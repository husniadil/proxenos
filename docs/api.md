# API contract

What proxenos exposes, and what callers may rely on.
[`proxy-behavior.md`](proxy-behavior.md) is the companion spec for how it
behaves internally.

Four surfaces: the HTTP ingress Claude Code talks to, the command line, the
control socket, and the configuration file. The ingress shape is fixed by the
Anthropic Messages API and is not ours to change. The other three are ours, and
the stability rules in §6 apply to them.

A bare `§N` in this file is a section of this file. A section of the behavior
spec is always written `proxy-behavior.md` §N.

---

## 1. Ingress

### Endpoints

| Endpoint | Purpose |
|---|---|
| `POST /v1/messages` | The only endpoint carrying real load. Answers with SSE where `stream` is true, and with one JSON body otherwise. |
| `POST /v1/messages/count_tokens` | Pre-flight sizing. Answers `{"input_tokens": n}`, an estimate. |
| `GET /v1/models` | The mapped models, in the Anthropic list shape: `{"data": [{"id", "display_name", "type": "model"}]}`. Ids are the upstream model ids the tiers map to. |
| `POST /control` | The §3 control vocabulary over HTTP. |
| anything else | `not_found_error`, 404. |

### Two doors, one daemon

The daemon always binds `127.0.0.1:<port>` and asks nothing of a caller reaching
it. Where `listen.address` (§4) names a reachable address, a **second** listener
opens there over the same state, and every request on it — ingress and
`POST /control` alike — must carry the token or is refused with
`authentication_error` (§1.1). A non-loopback `listen.address` with no token
refuses to start, naming both keys.

| Door | Address | Asks for the token |
|---|---|---|
| loopback | `127.0.0.1:<port>`, always bound | never |
| remote | `<listen.address>:<port>`, only where one is stated | on every request |

With `listen.address` at its loopback default there is one door.

#### Why

Every caller reaching loopback is already a local process running as the user.
The loopback door stays open when a token is configured because an `exec`
launch bakes `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>` into the client it
starts, and a local launch holds no token (§2.7). A daemon that moved to the
stated address would cut off every session already running on its own machine
with a 401 it could do nothing about.

### The token belongs to the door, not to the caller

Which listener a request arrived on decides whether it needs a token. Nothing
reads the peer address.

#### Why

- A guard keyed on the peer address cannot be tested from one machine, so the
  posture it implements is the one nobody exercises.
- Behind a reverse proxy or an overlay-network daemon every request arrives from
  loopback. A peer-keyed exemption would exempt the internet.

### The token is compared in constant time

A missing token and a wrong one get the same sentence.

#### Why

Short-circuiting on the first differing byte turns a comparison into an oracle
that recovers the secret one byte at a time. Saying which of the two failed
tells a caller where to look next.

### What `ANTHROPIC_AUTH_TOKEN` carries

It is the one header the client offers (sent as `Authorization: Bearer …`), so
both the token and the launch tag travel in it.

| Value | Meaning |
|---|---|
| anything (`unused`) | ignored, on the loopback door |
| `proxenos-account:<name>` | the launch tag `exec --account` (§2.3) travels as, naming the stored account that session's turns are made as |
| `proxenos-token:<secret>` | this daemon's token |
| `proxenos-token:<secret> proxenos-account:<name>` | both, whitespace-separated, in either order |

A value with no `proxenos-token:` part is read whole: the tag prefix is
stripped and the rest is the account name. Only a value that announces a token
is split into parts.

The tag is a name, and the credential it resolves to never leaves the daemon.
The token is a secret: it never appears in argv, a log line, or what `status`,
`env` or `settings` print (§2.2).

#### Why

An account name may hold a space. Splitting a value that was never multi-part
would silently truncate it.

### Relay or translate

`POST /v1/messages` either translates a turn or relays it. A relayed turn's body
is forwarded byte for byte and its reply streamed back byte for byte, with the
bearer replaced by that account's credential (`proxy-behavior.md` §9). The URL is
the same for both. The account that decides is, in order: the launch tag; the
account a relayed mapping entry claims for the model id; a tier's pinned
account; the serving account. The turn relays where that account is on the
second provider.

A launch tag naming no stored account is refused with `authentication_error`
before anything is spent.

This path is confirmed live against the second provider's real endpoint, plain
and streaming, generation and refusal (`roadmap.md` §L).

### `stream` decides the shape

Absent or `false` is not a stream and is answered with one `application/json`
message body: the frame sequence folded shut (`proxy-behavior.md` §5.5). Its
field set is held against a captured answer from the real endpoint; the one
field a real answer carries that this one does not is `stop_details`.

Claude Code always sets `stream`, so the harness only takes the streaming path.

### No request size limit

The ingress imposes no limit on a request body. The backend's limit is the real
one.

#### Why

A real turn — a full system prompt and a large tool set — runs past the
extractor's 2 MB default. A 413 from the door is not an Anthropic error shape,
so the client reads it as retryable and loops without the turn ever reaching
the backend.

### A held port fails the start

`run` fails immediately if the port is already bound, naming the conflict. It
does not retry or pick another port.

#### Why

A second daemon on a different port would be silently unused by a client
configured for the first.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/ingress.rs` | Router, doors, token guard, tag parsing, relay-or-translate branch |
| `crates/proxy/src/upstream/relay.rs` | The relay path and which account a turn relays as |
| `crates/core/src/anthropic/aggregate.rs` | Folding frames into the non-streaming body |
| `crates/proxy/src/commands/daemon.rs` | Binding both doors at startup |

### 1.1 Errors

Every failure returns an Anthropic-shaped body:

```json
{ "type": "error", "error": { "type": "...", "message": "..." } }
```

| Condition | Type | Status |
|---|---|---|
| Upstream 429 | `rate_limit_error` | 429 |
| Upstream 5xx | `overloaded_error` | 529 |
| Upstream unreachable, or the connection failed | `overloaded_error` | 529 |
| Upstream 400 | `invalid_request_error` | 400 |
| Upstream 401 or 403 | `authentication_error` | 401 |
| Upstream rejection, any other status | `api_error` | upstream status |
| Credentials invalid or absent | `authentication_error` | 401 |
| Credentials transiently unavailable | `overloaded_error` | 529 |
| Missing or wrong token on the remote door | `authentication_error` | 401 |
| Launch tag naming no stored account | `authentication_error` | 401 |
| Pinned tier naming an account not stored, or holding a credential the endpoint does not take | `invalid_request_error` | 400 |
| Request exceeds the model's window | `invalid_request_error` | 400 |
| Malformed request body | `invalid_request_error` | 400 |
| Unknown endpoint | `not_found_error` | 404 |

`retry-after` is forwarded when upstream supplies it.

### Transient is retryable, terminal is terminal

The proxy builds no retry loop of its own. Claude Code's backoff drives the
retryable types.

### Mid-stream failures

An error arising after the status was sent is an SSE `error` frame. On the
non-streaming path nothing is written until the turn is over, so the same
failure is a status and an error body. The status is rebuilt from the frame by
the same vocabulary, so the two never disagree; an `api_error` frame folds to
502.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/error.rs` | `ProxyError`, the constructors, upstream status mapping, frame-to-status |
| `crates/core/src/anthropic/stream.rs` | `ErrorKind` and the error body |

---

## 2. Command line

```
proxenos start [--port N]          start the daemon in the background
proxenos run [--port N]            start the daemon in the foreground
proxenos accounts [--json]         stored accounts, and which one serves
  accounts list [--json]           the same
  accounts login NAME --provider codex|anthropic [--path DIR]
                 [--device-auth] [--relogin]
  accounts add-key NAME --provider codex|anthropic   (key on stdin)
  accounts use NAME
  accounts rename OLD NEW
  accounts remove NAME
proxenos status [--json]           connection, tier mapping, catalog
proxenos models [--json] [--account NAME]
proxenos incidents [--json]        open incidents on the providers' status pages
proxenos env                       environment for Claude Code, as shell exports
proxenos settings                  the same, as one client settings document
proxenos exec [--account NAME] [--] PROGRAM [ARGS...]
proxenos reload                    re-read config.toml into the running daemon
proxenos stop                      ask the running daemon to stop
proxenos tiers [--json] [--account NAME]
  tiers set TIER MODEL [--account NAME] [--persist]
            [--as ACCOUNT [--allow-cross-account]] [--effort LEVEL]
  tiers cross-account on|off
proxenos effort [--json]           the effort ceiling in force
  effort set LEVEL|none [--account NAME] [--persist]
proxenos doctor [--live] [--probe NAME] [--fixtures DIR] [--relay-account NAME]
proxenos usage [--json] [--refresh]
proxenos statusline [-- COMMAND...]
proxenos inspect PID [--json]
proxenos record ingress [--port N]
proxenos record upstream [--port N]
proxenos record surface --account NAME [--only NAME] [--out DIR]
proxenos supervisor install|uninstall|status [--json]
```

`proxenos --help` and `proxenos <verb> --help` are the source of truth for flag
spelling. `--port` also reads `PROXENOS_PORT`.

### What reaches the daemon

Every verb goes through the control vocabulary (§3) against a running daemon,
except: `run`, `start` and `record` (bring a daemon up, or run their own),
`supervisor` (touches the machine), `doctor` (runs in the CLI), `inspect` (reads
a process), and `accounts login` and `accounts add-key` (need a terminal and the
store). The last two still call the socket afterwards where a daemon answers.

With `PROXENOS_DAEMON` set, the control vocabulary goes over HTTP to a daemon
elsewhere (§2.7).

### `--json` means one thing

On every verb that takes it, `--json` prints the control socket's payload for
that verb, unrendered. `supervisor status --json` and `inspect --json` print
their own documents, since neither has a socket payload.

#### Tried and dropped

`env --json` printed the settings document, a different verb's output. It is
gone with no alias: `settings` is that document's only name.

### One sub-verb per action, the account positional

Each thing an operator does to an account is its own sub-verb, and the account
it acts on is always the positional `NAME`.

#### Tried and dropped

Flags as actions (`--use`, `--forget`, `--rename` on `accounts`) and a top-level
`login` split by `--key` and `--profile`. What a command did depended on which
flags were present, and the account was spelled `--as` in one verb and `--use` in
another. The top-level `login` is gone with no alias.

### Adding an account never obtains a grant

Neither `accounts login` nor `accounts add-key` runs an authorization flow of
this daemon's own. There is no callback port and no `--setup-token`.

### `accounts login`

Signs in to a profile the daemon will borrow from (`proxy-behavior.md` §8.4). It
runs the owning program's own login — `claude auth login` or `codex login` —
against a directory, with the same environment variable the daemon later
resolves the grant from. Afterwards the profile is read, and only a directory
holding a grant is written into `[profiles]`.

- `--path DIR` says where. Absent, the profile goes under
  `<config dir>/profiles/<NAME>`.
- A directory already signed in is adopted: no client runs, and the entry is
  written.
- Where there is no terminal to answer the login's prompts, the command is
  printed instead of run, with the environment variable on it, together with
  the line that declares the profile afterwards.
- A declared profile reaches a running daemon at once: the verb calls
  `config.reload` after it writes and says whether the daemon took it. No socket
  is not a failure.

#### Why

Running the login against the same variable the daemon reads means what was
signed in and what is read cannot drift apart. Nothing here sees a token. A
client that wants a browser and a keyboard, started from something with neither,
hangs with nothing said, which is why the command is printed instead.

### `accounts login --relogin`

Signs a profile `[profiles]` already declares back in. The name must be
declared, `--provider` must match its declaration, and the directory is the one
declared: `--path` is refused, and a declaration with no path is the stock
profile, signed in with no variable set. The client runs whatever the profile
currently reads as. Nothing is written afterwards.

#### Why

A lapsed grant still reads as a grant. Without the flag the name is refused as
already declared, and adoption would change nothing.

### `accounts login --device-auth`

Puts `--device-auth` on the `codex login` that is run or printed, so the client
prints a URL and a code instead of opening a browser. It is on the printed way
back too. `--provider anthropic` refuses it.

#### Why

A machine may have a terminal and no browser. A re-run that dropped the flag
would start the client the way that hangs. `claude auth login` has no
equivalent, and passing it on would end in a usage error about a spelling rather
than about the choice.

### `accounts add-key`

Stores an API key read from **stdin**. At a terminal it says on stderr what it is
waiting for and reads from a hidden prompt; from a pipe it says nothing, so
`printf '%s' "$KEY" | proxenos accounts add-key NAME --provider P` writes to
stdout only the line naming what it stored.

- `NAME` and `--provider` are required. `--provider` has no default.
- A name already holding a key of the same provider is rotated in place.
- A name holding a key of the other provider is refused, naming the account, the
  provider it holds, and `accounts remove NAME`.
- A name that is a declared profile is refused.
- An `anthropic` key beginning `sk-ant-oat` gets a note on stderr, at a terminal
  only: that stem belongs both to the year-long token `claude setup-token` mints
  and to the harness's hours-long OAuth access token, nothing stored can tell
  them apart, and the second will stop authenticating (`proxy-behavior.md`
  §8.2). The key is stored either way, and the note names no part of the secret.
- Storing a key never moves the selection, except that a lone account is
  selected so the choice is written down before a second exists. Where it did
  select, the verb calls `accounts.select` so a running daemon follows.

#### Why

An argument is visible to every process on the machine and lands in shell
history. The two providers refuse each other's credentials, so a key that
claimed the wrong one would fail later as an authentication error naming the
credential rather than the choice. Storing a credential and choosing who pays
are two decisions; one command making both moved every turn onto a newly stored
account without saying so.

### `accounts` and `accounts list`

A table under `NAME PROVIDER KIND ACCOUNT SOURCE STATE`, declared profiles
first, then keys, with a `*` on the account serving turns.

| Column | Content |
|---|---|
| `PROVIDER` | `codex` or `anthropic`, on every row |
| `KIND` | `profile` or `key` |
| `ACCOUNT` | the address, else the id, else the subscription |
| `SOURCE` | the store it was read from, `$HOME` shortened to `~`; `keychain` for a keychain, `stored` for a key; `(found)` on a profile nobody declared. The only column ever cut |
| `STATE` | one phrase: `refused`, `identity changed`, the renewal countdown, else `ok` |

A grant left in `credentials.json` by an older version is not listed as an
account; it is named in a note under the table.

#### Why

With two providers stored, a row without its provider is a guess. A credential
that quietly stopped counting reads as one that vanished, so it is named.

### `accounts use NAME`

Serves every following turn as that account. The confirmation says how far the
switch moved: within one provider it reads `still on codex`; across providers it
names both sides, `codex to anthropic`.

#### Why

A name does not state a provider. A cross-provider switch changes which backend
answers, which path turns take, and which subscription is drawn down, and only
the daemon holds the answer.

### `accounts rename OLD NEW`

Works on a key. Refused for a borrowed profile, naming it.

#### Why

A profile's name is the key it is declared under in `[profiles]`, and changing it
is an edit to a file the operator can see.

### `accounts remove NAME`

- A key is dropped from this daemon's store.
- A **declared** profile loses its `[profiles]` line. The grant stays where it
  is, and the daemon re-reads the file.
- A profile the daemon **found** rather than was given is refused, saying it was
  found and that `[profiles]` is empty.

All of it goes through the socket.

#### Why

The daemon holds the selection. A CLI that edited the file directly would leave
a running daemon serving the account it read at startup. A found profile has no
line to delete, and writing the set down is what makes one removable.

### `status`

Renders the `status` payload.

- The `auth` line leads with the account's name, as `accounts` lists it —
  `auth       work-codex (husni@sayurbox.com, codex)` — then the address, kind
  and provider. A payload with no name renders without one.
- A `daemon` line names the build and pid serving the socket, and whether the
  §2.6 supervisor started it. Where `supervised` is null the clause is left out.
- The tier rows are a table under `TIER MODEL` in ladder order — `opus, sonnet,
  haiku, fable` — with a `STATE` column only where some row has a state:
  `inert while relaying`, or `as <account>` for a pinned tier.
- Where the serving account is on the second provider, every unpinned tier row
  is marked inert and a `routing` line names the provider the ids relay to. A
  pinned tier names its own account and stays live.
- Where the CLI's build differs from the daemon's, it says so.

#### Why

The name is the string every account verb takes. `supervised` null is silence
because "not supervised" is a claim a platform with no supervisor cannot make.
When the serving account relays, every id relays verbatim and the mapping decides
nothing (`proxy-behavior.md` §9.1); four unqualified rows would read as "your
turns go to these models", the one thing they do not mean. The payload sorts
`tiers` by name, an order nothing else uses. A version line printed on every run
is one nobody reads on the run that matters.

### `models`

A table under `MODEL WINDOW TIER`. `TIER` names the tiers pointing at each id, in
ladder order, read from the `tiers` method. Where `tiers` cannot be read the
column is left off. A model with no stated window reads `window unknown`.
`--account NAME` asks for that stored account's menu.

#### Why

A blank cell reads as "no tier maps to this model", a different statement from
"this side does not know".

### `incidents`

Asks the daemon for the incidents open on the status page of every provider a
stored account is on. A table under `PROVIDER IMPACT STATUS SINCE NAME URL`,
worst first, then one line per page that did not answer. Nothing open is a
sentence naming the providers asked.

### `tiers`

Reads the mapping as `TIER MODEL`, a tier the catalog cannot honour marked.
`--account NAME` reads that stored account's mapping (the `tiers` method's
`account` parameter); it cannot be combined with a sub-verb.

### `tiers set TIER MODEL`

Points one tier at a model through `tiers.set` (§3), with exactly what was typed.

| Flag | Effect |
|---|---|
| none | the shared table, until the daemon stops |
| `--account NAME` | that account's section instead |
| `--persist` | also written to `config.toml` |
| `--as ACCOUNT` | pins the tier to a stored account. Refused, naming the flag, without cross-account consent |
| `--allow-cross-account` | grants that consent first through `cross_account_tiers.set` (always written). Requires `--as` |
| `--effort LEVEL` | the effort the client starts the tier's model at, delivered in the launch settings (§2.2). Omitted, the tier carries none |

One tier per call. The answer says whether the change was persisted and where.

#### Why

A set is partial and replaces the tier's whole value, so an omitted `--effort`
clears it rather than keeping an old one.

### `tiers cross-account on|off`

Grants or revokes consent for pinned tiers. `off` is refused while any tier still
pins an account.

### `effort`

Reads the ceiling in force from `status`.

### `effort set LEVEL|none`

Sets the ceiling through `effort.set` (§3) to `minimal`, `low`, `medium`, `high`,
`xhigh`, `max` or `ultra`. `ultracode` is read as `xhigh`. `none` sends null,
removing the override. `--account` and `--persist` as for `tiers set`. The answer
reports the ceiling that results, not the one asked for.

### `doctor`

Runs the capability probes and prints a matrix. The default answers from the
fixture corpus, contacts nothing, and costs nothing. The matrix always states
which mode produced it.

| Flag | Effect |
|---|---|
| `--live` | answers from the real backend, one turn per probe, spending quota. Model ids are mapped through the configured tiers |
| `--probe NAME` | runs one probe. An unknown name lists the known ones |
| `--fixtures DIR` | answers from that directory only |
| `--relay-account NAME` | which account the live relay probe spends, where several are on the second provider |

#### Why

A matrix from replayed fixtures that reads like one from a live backend is the
plausible-looking output the probes exist to prevent.

### Which corpus answers

`--fixtures DIR` is used and nothing else; a fixture missing from it skips the
probe. With no `--fixtures`, a `fixtures/` directory in the working directory
wins if present, otherwise the corpus compiled into the binary.

#### Why

A recording just captured by `record` must be what a run against it sees, not a
compiled copy. An installed binary has no checkout, and a first run that skipped
every probe would establish nothing.

### Rows

- A probe that could not run is `skipped`, never a pass.
- A failed row prints the probe's rationale beneath it; passing rows stay one
  line.
- Under `--live`, `count-tokens` and `env-contract` are marked as answered by the
  proxy: sizing never leaves the proxy, and the launch surface is rendered, not
  sent.
- Checks that only mean something against a recording (an exact URL the corpus
  wrote) are marked in the probe table and skipped live.

### A live run resolves its credential first

A live run that cannot resolve a credential answers with that refusal alone. It
probes the endpoint the account's kind belongs to (`proxy-behavior.md` §8.2).
It also names, above the matrix, any tier whose stated model this account's
catalog does not carry; the catalog fetch is a model list, not a turn.

#### Why

Seven capabilities reported broken for want of a credential, under a header
saying the backend answered and was billed, is the failure the probes prevent,
printed the other way round.

### `env-contract`

Renders the §2.2 environment for two representative mappings and holds it to its
contract: `ENABLE_TOOL_SEARCH` on every launch, and
`CLAUDE_CODE_DISABLE_1M_CONTEXT` present where a tier translates and absent where
every tier relays (`proxy-behavior.md` §7.2). It replays nothing and runs in both
modes.

#### Why

Both variables were settled against a live client and both fail silently: without
the first the client disables deferred tool loading on a base URL it does not
recognize as first-party; without the second it appends `[1m]` to an
unrecognized id and assumes a window four times the model's. Either regression
presents as a broken client over a green matrix.

### The relay probe

Runs in both modes. Replayed, it drives the relay branch against a recording
whose marker sits inside a field the proxy does not model, and a stand-in backend
records the bytes sent, so both halves are checked. Live, it sends a turn to the
second provider's real endpoint and checks the answer half only, and the row says
so.

The live account is read from the store, not from the selection: exactly one
account on the second provider is used; several need `--relay-account`; none
skips the row, naming what the store holds. Authorizing by name neither reads
nor changes the selection.

#### Why

A body round-tripped through the proxy's own types fails the marker check. Live,
the outbound bytes leave on a socket this process cannot read, and checking them
against a stand-in would report a pass for a half nothing looked at.

### The coverage line

One line under the matrix, assembled from outcomes:

- `Exercised:` lists each path with a passing row, and the account it spent.
- `Not exercised:` lists each path nothing ran on, and always the WebSocket
  transport — a live run is HTTP only.
- A path whose probes all ran and all failed gets a clause of its own.
- A heading with nothing under it is not printed.

The relayed account is named separately from the account the translating probes
spent.

#### Why

Green rows say nothing about a path no probe drove, and a reader with nothing
saying so reads green as coverage of the whole proxy.

### `usage`

What quota is left, per account, as a table under
`NAME PROVIDER USED RESETS SOURCE AS OF`, the serving account's plan above it and
a `*` on the serving row.

- **One row per window.** Rows after an account's first repeat neither name nor
  freshness.
- `USED` is the percentage and the window it is of, plus the provider's own words
  about that window. A metered row carries the token tally
  (`proxy-behavior.md` §6.1) instead.
- `RESETS` counts down to the reset, or says the window has already reset.
- `SOURCE` is `last turn` or `asked`.
- `AS OF` is the figure's age, or the reason there is none: `no turn yet`,
  `no relayed turn yet`, `per token`, `not reported`. The explanation is one note
  under the table naming `usage --refresh`.
- A **credit balance** gets its own row: `credit: $205.75 / $210.52 · 98%`, the
  provider's severity word in parentheses where not `normal`, and `RESETS`,
  `SOURCE`, `AS OF` blank. Amounts are the minor units divided by the stated
  exponent; the percentage is the provider's, never recomputed; a currency with
  no known symbol is named by its code.
- A **subscription not active** gets its own row: `subscription canceled`, the
  provider's own word.
- A window whose reset has already passed is marked stale; a window with no reset
  stated never is.

A figure rides a turn already being made — the backend opens every stream with
one — so a bare `usage` costs nothing. Before any turn it says so rather than
printing zeroes. A figure survives a restart where its window has not reset
(`proxy-behavior.md` §6.1). A second-provider account earns its figure from the
`anthropic-ratelimit-unified-*` headers on a relayed turn (`proxy-behavior.md`
§9.4), with no plan name beside it. With one account, nothing is repeated under
its own name.

#### Why

Figures are per account because a pinned tier spends the account it names
(`proxy-behavior.md` §7.1), so a daemon can hold two live figures. "No turn yet"
is scoped to this daemon: a CLI process such as `doctor --live` can spend quota
the daemon never saw. A subscription that is not active is the one state the
percentages cannot show — quota reads untouched while every turn is refused.

### `usage --refresh`

Calls `usage.refresh` (§3), then prints the same document. One request per
askable account.

#### Why

Asking spends a request per account, so it is opted into. The `--json` shape is
the same either way.

### The quota headers do not reach the status line

The snapshot is also put on the response as `anthropic-ratelimit-unified-*`
headers, which feed the client's retry banner on a quota 429. They do not make
`rate_limits` appear in the client's status-line payload: that field is gated on
a flag its schema documents as false for the API-key path, and a proxied client
is on that path by definition. `statusline` (§2.1) is the only route.

### `record`

| Mode | Captures | Needs | Spends |
|---|---|---|---|
| `ingress` | what the client sends, before translation | a client | nothing |
| `upstream` | the client's untranslated request paired with the backend's stream, for every turn | credentials | quota, one turn per turn |
| `surface` | a fixed list of exchanges against the second provider's Messages endpoint | an account on that provider | one turn per exchange |

`ingress` and `upstream` run a daemon and take `--port` / `PROXENOS_PORT`.
Captures go to `captures/` in the configuration directory, `0600`, and the most
recent twenty are kept. They hold conversation content.

- Ingress keeps request headers, with `authorization`, `x-api-key`, `cookie` and
  `proxy-authorization` redacted by name. A relayed turn's request is held as the
  exact bytes relayed.
- `upstream` warns at start that every turn spends quota.
- `surface` needs no daemon and goes out through the relay code. `--account` is
  required and must be on the second provider. `--only NAME` captures one
  exchange. `--out DIR` defaults to `fixtures/surface`. Response headers are
  scrubbed by name before writing: `authorization`, `x-api-key`, `cookie`,
  `proxy-authorization`, `set-cookie`, and the organization and workspace ids.

Ingress and upstream share one fixture format. Surface captures hold a status, a
scrubbed header set, and a body or a list of SSE payloads.

#### Why

A request cannot be inferred from a stream, and a translated request could not be
replayed through the translation it has already been through. A relayed body
re-encoded through this proxy's types would drop every field they do not model.
Spending the wrong subscription is not recoverable, so `surface` names its
account; a capture on disk is quota already spent, so `--only` exists. Organization
and workspace ids say whose account paid, and fixtures are committed.

### Logging

Controlled by `RUST_LOG`, written to stderr. Credentials never appear at any
level.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/cli.rs` | The verb and flag set |
| `crates/proxy/src/main.rs` | Dispatch, logging setup |
| `crates/proxy/src/commands/accounts.rs` | `accounts` sub-verbs |
| `crates/proxy/src/auth/profile_login.rs` | `accounts login`, adoption, relogin, printed commands |
| `crates/proxy/src/auth/key_login.rs` | `accounts add-key`, stdin reading, the `sk-ant-oat` note |
| `crates/proxy/src/commands/inspect.rs` | `status`, `models`, `incidents`, `usage`, `statusline` |
| `crates/proxy/src/commands/policy.rs` | `tiers`, `effort` |
| `crates/proxy/src/commands/doctor.rs`, `crates/proxy/src/doctor.rs`, `crates/proxy/src/probe.rs` | `doctor`, probes, coverage line |
| `crates/proxy/src/commands/record.rs`, `crates/proxy/src/recorder.rs`, `crates/proxy/src/surface.rs` | `record` and capture files |
| `crates/proxy/src/render/` | Every table and line the verbs print |

### 2.1 `statusline`

Wraps a status-line script: reads the payload the client hands it on stdin,
merges in the quota, and passes it on.

```json
{ "statusLine": { "type": "command",
                  "command": "proxenos statusline -- ~/.claude/my-statusline.sh" } }
```

The merged payload gains:

- `rate_limits.five_hour` and `rate_limits.seven_day`, only where a window's
  duration genuinely is one of those;
- `rate_limits.windows`, every window the backend reported with its real length;
- the `serving` block from `usage` — name, provider, address, plan, account id —
  whether or not a figure is known.

With no command after `--`, the merged payload is printed. The wrapped command's
exit status becomes this command's.

#### Why

A script written against the client's own shape keeps working and gains a figure
it could not otherwise have. The backend's windows are not fixed, so a window
matching neither slot is left to `windows` rather than announced as one it is
not. On a daemon that has served no turn, who is paying is the only thing worth
rendering.

### It never breaks the status line

A daemon not running, a socket not answering, a payload that will not parse:
each passes through unchanged.

#### Why

A status line renders constantly, and one that breaks is worse than one missing
a figure.

### It never merges another session's quota

`usage` reports the ids this daemon serves: the configured tiers plus every id a
turn has been made against. A payload naming another model passes through
untouched, `serving` included. Where either side names no models there is
nothing to compare, and the figure is merged.

#### Why

One status line renders for every session, including ones pointed at their own
provider. Withholding on an unanswerable question would take the figure from
every session that has it to prevent a case that may not be happening.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/statusline.rs` | The merge and the session check |
| `crates/proxy/src/commands/inspect.rs` | The verb |

### 2.2 `env` and `settings`

The configuration Claude Code needs, in two renderings. `env` is shell exports;
`settings` is one client settings document.

### `env`

```
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
export ANTHROPIC_AUTH_TOKEN=unused
export ANTHROPIC_DEFAULT_OPUS_MODEL=<mapped>
export ANTHROPIC_DEFAULT_SONNET_MODEL=<mapped>
export ANTHROPIC_DEFAULT_HAIKU_MODEL=<mapped>
export ANTHROPIC_DEFAULT_FABLE_MODEL=<mapped>
export CLAUDE_CODE_MAX_CONTEXT_TOKENS=<effective window>
export CLAUDE_CODE_AUTO_COMPACT_WINDOW=<effective window>
export CLAUDE_CODE_DISABLE_1M_CONTEXT=1
export ENABLE_TOOL_SEARCH=true
export ENABLE_CLAUDEAI_MCP_SERVERS=false
```

### Tier variables

`ANTHROPIC_DEFAULT_<TIER>_MODEL` is emitted for every tier, except a tier the
serving account relays (`proxy-behavior.md` §9.1) whose model was not stated for
that account — pinned in `[tiers]` or named under `[accounts.<name>.tiers]`.

#### Why

`WebFetch` and `WebSearch` run on the haiku tier, so an unmapped haiku breaks them
in a way that looks unrelated to tier mapping. For a relayed tier the shared
table's id is the first provider's, and the client's own id is the one the second
provider accepts (`proxy-behavior.md` §7.2).

### Window variables

`CLAUDE_CODE_MAX_CONTEXT_TOKENS` and `CLAUDE_CODE_AUTO_COMPACT_WINDOW` appear only
where the catalog knows the window, carrying the smallest across the mapped
tiers. `CLAUDE_CODE_AUTO_COMPACT_WINDOW` is further limited to 100,000–1,000,000
tokens; outside that range it is left out and the reason logged. A mapping with
any tier on the second provider states no window, and one served entirely by
that provider also omits `CLAUDE_CODE_DISABLE_1M_CONTEXT`.

The client will warn that its 200,000 limit is not enforced. That is expected.

#### Why

Outside the range the client's parser answers `Expected 'auto' or 100k–1M tokens`
and the settings key of the same meaning discards the value silently. The client
recognizes second-provider ids by itself, and both variables would replace what
it knows with a figure this catalog cannot supply (`proxy-behavior.md` §7.2).

### `CLAUDE_CODE_DISABLE_1M_CONTEXT`

Load-bearing. Without it the client appends `[1m]` to an unrecognized id and
assumes a million tokens. With it, the client also strips `context-1m-2025-08-07`
from the beta list it sends (`proxy-behavior.md` §7.2).

### `ENABLE_TOOL_SEARCH=true`

On every launch.

#### Why

The client disables deferred tool loading whenever its base URL is not a
first-party host, and this is its own override. Both paths carry what deferral
needs: the relay forwards `defer_loading` and `tool_reference` verbatim, and the
translating path carries client-driven discovery (`proxy-behavior.md` §2.5).
Measured live on both: an MCP set costing ~101k tokens up front defers to zero and
turns succeed.

### `ENABLE_CLAUDEAI_MCP_SERVERS=false`

Emitted when `client.disable_connectors` is on. It is the only piece of client
policy (`proxy-behavior.md` §7.3) with an environment variable. Where there is
policy the exports cannot carry, a comment above them says so and names
`settings` and `exec`.

### Whose environment

The `env` method takes an optional `{"account": name}`, and `exec --account`
passes it. The mapping, window and client policy are then resolved for that
account; without it, for the selection. A name the store does not hold is refused
by name. A named account's mapping is resolved from `config.toml` the way
`accounts.select` would resolve it; a `tiers.set` never persisted does not carry
across to it.

### `settings`

```json
{
  "env": { "ANTHROPIC_BASE_URL": "http://127.0.0.1:8787", "...": "..." },
  "permissions": { "deny": ["Skill(claude-api)"] },
  "disableClaudeAiConnectors": true,
  "remoteControlAtStartup": false,
  "attribution": { "commit": "" },
  "modelSettings": { "<mapped>": { "effortLevel": "high" } }
}
```

- The document is complete on its own. Measured: a client with no `ANTHROPIC_*`
  in its environment, reading only this `env` block from a settings file, reached
  the proxy.
- `permissions`, `disableClaudeAiConnectors`, `remoteControlAtStartup`,
  `attribution` and `modelSettings` are absent when nothing is configured.
- The proxy publishes this document and never installs it.

#### Why

An empty deny list merged over a real one is how a rule disappears.

### `modelSettings`

A tier that states an effort (§4) becomes one entry keyed by the tier's upstream
model, with the effort as `effortLevel`. Two tiers on one model must agree, and
are refused by name where they do not. Resolved for the same account as the rest
of the document.

Measured against Claude Code 2.1.259 through a stand-in endpoint: the client sends
the stated effort for a second-provider id, `--effort` on a session overrides it,
and an id with no entry sends the client's default. The daemon's ceiling (§4)
still caps what arrives.

#### Why

Keyed by model, not alias: the `ANTHROPIC_DEFAULT_<TIER>_MODEL` line resolves the
alias before the client looks the effort up, and the client keeps one effort per
model.

### The payload's `settings` is always present

The `env` method's `settings` field is an empty object where there is no policy.
Absence means a daemon that predates client policy. `settings` and `exec` refuse
against such a daemon; `env` continues with a comment saying the policy is
missing.

#### Why

One file is both the daemon and the CLI, and replacing it does not restart a
running daemon, so a newer CLI against an older daemon is an ordinary upgrade
state. If "no policy" and "cannot answer" looked alike, a document lacking a
permission rule would look complete.

### Redirecting into a settings file overwrites it

`>` truncates. `.claude/settings.local.json` is where the client records the
permissions a user accepted, so an existing file with content is the common case.
Merge, or write somewhere nothing else owns. `jq -s '.[0] * .[1]'` is wrong: it
takes arrays from the right-hand side, so `permissions.deny` is replaced rather
than extended.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/control/handler.rs` | The `env` method: variables, policy, `modelSettings`, named account |
| `crates/proxy/src/launch.rs` | The launch environment's variables and window rules |
| `crates/proxy/src/render/launch.rs` | Shell exports and the settings document |
| `crates/proxy/src/commands/launch.rs` | `env`, `settings`, `exec` |
| `crates/proxy/src/config.rs` | `ClientConfig` and `model_settings` |

### 2.3 `exec`

Runs a command with the §2.2 configuration applied.

```
proxenos exec claude --resume abc
proxenos exec -- claude --help
proxenos exec --account personal claude
```

The environment is set on the child. The settings document rides on the client's
own `--settings` flag, inline: nothing is written to disk. Everything from the
program name onward is forwarded in order; `--` separates a program whose first
argument would read as this verb's. On Unix the child is `exec`d, so signals, job
control, the terminal and the exit status pass through.

A program that does not read `--settings` is given the environment only, and
stderr says the policy was left out.

#### Why

No file means none to go stale and none to clean up. The document holds no
secret — outside client mode the auth-token value is ignored — so argv is a fine
place for it.

### `--account NAME`

Serves this session as the named account without moving the selection. Consumed
before the program name and never forwarded; it travels as
`ANTHROPIC_AUTH_TOKEN=proxenos-account:<name>` (§1). The daemon reads the tag per
turn, and it outranks a tier's pinned account. The environment is rendered for
that account (§2.2), and stderr prints the account and the tier ids the launch
carries, saying where a tier carries none. A name the store does not hold is
refused at launch and again at the turn.

#### Why

`accounts use` is the standing switch; this is the per-session one. A session
served by one provider and handed the other's tier ids sends them: seen live as a
launch tagged onto a second-provider account given `gpt-5.6-luna` from the shared
table and refused as an unrecognized model. Refusing at the turn too means an
account removed mid-session fails loudly instead of falling back to the selection.

### `--model` is upgraded where the session relays

Where the session's account relays, a plain `--model` id whose `[1m]` variant is
on that account's curated list (§3, `models`) is rewritten to the variant, and
stderr names the rewrite. An id already carrying the marker, an alias the list
does not name, and another program's `--model` are forwarded as typed. A session
that translates rewrites nothing.

#### Why

`[1m]` is the client's own long-context selector, so the session starts on the
million-token window. Translating to the first provider, the marker would make
the client assume a window it does not have. The list is asked for the session's
account because a menu is one account's (`proxy-behavior.md` §7.0).

### Refusals before anything starts

- The daemon is not answering.
- The daemon predates client policy (§2.2).
- The forwarded arguments already carry `--settings`. The refusal names the
  collision; `proxenos settings` prints this proxy's half to merge.

#### Why

A client launched against no daemon reports a connection refused it cannot
explain. Measured: given two `--settings` flags, the client keeps the last, drops
the first, exits 0 and says nothing, so either order silently loses one policy.

### The policy does not reach a grandchild

A child inherits the environment into anything it spawns, not its argv. A client
started from inside the session carries routing and not policy. Anything that
spawns a client composes its own `--settings`.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/commands/launch.rs` | `exec`: refusals, `--account`, `[1m]` upgrade, client-mode token |
| `crates/proxy/src/ingress.rs` | `auth_token_value`, the tag's one spelling |

### 2.4 `stop`

Asks the running daemon to stop, then reports what it observed afterwards.

```
$ proxenos stop
stopped 0.2.0+ab12cd3; launchd started it again as 0.3.0+cd34ef5
```

- The daemon answers before it goes; the run loop is released only once the
  response is written.
- An in-flight turn is cut.
- The CLI watches `instance` on `status`: a different id is a different process.
  It waits three seconds for the daemon to go and twelve for anything to bring it
  back, returning as soon as it sees the answer.
- Where the departing daemon's `supervised` was `true`, the sentence names
  `launchd`; otherwise it says `something`. With nothing back it says nothing
  started it again.
- Builds are named unless the strings are identical, which with a build id (§3)
  means the same build.
- A daemon predating the `shutdown` method cannot be stopped this way, and the
  CLI says that is the situation instead of surfacing `unknown method`.

#### Why

Under a supervisor, `stop` is how a running daemon is replaced by the build on
disk. Whether anything restarts it belongs to the supervisor, so this reports
what it saw. A socket falling quiet is about timing, not the daemon: a quick
supervisor leaves no gap, a throttled one leaves a long one. Twelve seconds
because launchd holds a respawn ten seconds after the last start. A closed
connection with no reply cannot be told from a crash. A dropped connection is
something the client's own retry already handles.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/commands/daemon.rs` | `stop` and its sentences |
| `crates/proxy/src/commands/mod.rs` | `STOP_WINDOW`, `RESTART_WINDOW`, `watch` |
| `crates/proxy/src/control/handler.rs` | The `shutdown` method |

### 2.5 `start`

Starts the daemon in the background and returns once it answers.

```
$ proxenos start
daemon running (pid 4711), logging to ~/.config/proxenos/daemon.log
stop it with `proxenos stop`
```

- The child is `run` of the same binary in its own process group, stdout and
  stderr appended to `daemon.log` in the configuration directory. `--port` as for
  `run`.
- Exit 0 only once the daemon answers the control socket. A child that dies first
  is reported with the tail of what it wrote this start, exit nonzero. Ten seconds
  without either is the same, and the child is ended.
- A daemon already answering is named and left alone, exit 0:
  `already running: 0.12.0+ab12cd3 (pid 4711), supervised`. `pid` and
  `supervised` are said only where the daemon reports them; `false` reads
  `not supervised`, null says nothing.

#### Why

Backgrounding is what an operator asks for and holding the terminal is what a
supervisor asks for, so they are two verbs. A backgrounded process's terminal is
gone once the command returns. A second daemon would take over the first's socket
file while the first held the port. The state the verb was asked to produce is
the state that holds, so an already-running daemon is not a failure.

#### Tried and dropped

`run --detach`. Gone with no alias.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/commands/daemon.rs` | `start`, `run`, the readiness wait |

### 2.6 `supervisor`

Installs, removes, and reports the supervisor that brings the daemon back when it
dies.

```
$ proxenos supervisor install
supervising proxenos.daemon, from ~/Library/LaunchAgents/proxenos.daemon.plist
  runs /Users/someone/.local/bin/proxenos run
  logs to ~/.config/proxenos/daemon.log
  control socket /var/folders/j2/…/T/proxenos.sock
stop it for good with `proxenos supervisor uninstall`
```

```
$ proxenos supervisor install          # on Linux
supervising proxenos.service, from /home/someone/.config/systemd/user/proxenos.service
  runs /home/someone/.local/bin/proxenos run
  logs to /home/someone/.config/proxenos/daemon.log
  the unit's own failures go to the journal: journalctl --user -u proxenos.service
  control socket /tmp/proxenos.sock
stop it for good with `proxenos supervisor uninstall`
```

| Action | Effect |
|---|---|
| `install` | writes the unit for this user and hands it to the supervisor |
| `uninstall` | removes both; the daemon it supervised stops |
| `status` | whether it is installed and what the supervisor makes of it |

The verb and its three actions are semver-bound (§6). The name is the role, not
launchd, so a second implementation could join it.

### Platforms

| Platform | Unit | Handed to |
|---|---|---|
| macOS | `~/Library/LaunchAgents/proxenos.daemon.plist`, label `proxenos.daemon` | `launchctl bootstrap` |
| Linux | `$XDG_CONFIG_HOME/systemd/user/proxenos.service` (`~/.config` where unset) | `systemctl --user` |
| anything else | refused, naming both supervisors and `proxenos start` | — |

`PROXENOS_HOME` does not move the Linux unit; systemd reads `XDG_CONFIG_HOME`
only.

#### Why

A unit installed but never run reports success and supervises nothing, so nothing
writes a file it cannot hand to a supervisor.

### Linux is a user unit only

Never `sudo`, never a system unit. Where no per-user systemd is reachable — no
login session, no `XDG_RUNTIME_DIR`, no session bus, as inside a container or over
bare `ssh <host> <command>` — every action including `status` refuses before
writing anything, quotes what `systemctl --user` said, names both variables and
their values, and names `loginctl enable-linger $USER` where logind exists.

#### Why

A system unit runs as another user and binds a control socket in a home this
operator does not own: a healthy daemon the CLI never reaches.

### The job runs `run` in the foreground

`Type=simple` on systemd, no fork on launchd. `KeepAlive` restarts on macOS;
`Restart=always` with `RestartSec=5` on Linux. The daemon logs to the same
`daemon.log` on both (`StandardOutput`/`StandardError` are `append:` it on Linux);
`install` on Linux names `journalctl --user -u proxenos.service` for failures of
the unit itself.

#### Why

A process that forks away leaves the supervisor watching something already
exited. systemd's 100ms default puts a unit in `failed` after five restarts in ten
seconds, and `run` against a held port is exactly that crash loop.

### The unit carries no credential, and two variables

The job's environment is `TMPDIR`, always, and `PROXENOS_HOME` where the
installing shell names one. Nothing writes `EnvironmentFile=`. `TMPDIR` records
the value the socket derivation used, the `/tmp` fallback included. A socket path
too long for the platform is refused when the unit is planned.

#### Why

A unit file in the home is readable, and the store holds credentials. The socket
path is derived from those two variables (§3). launchd supplies a `TMPDIR` of its
own and `systemd --user` supplies none, so a unit that omitted it would bind a
different socket than the operator's shell dials: a daemon healthy on its port
while every verb reports connection refused.

### `status`

`status --json` is one document on both platforms: `installed` (`absent`,
`current`, `divergent`), `program`, `log`, `socket`, `state`, `pid`, and the file
under `plist` on macOS or `unit` on Linux. `state` and `pid` come from
`launchctl print`, or `systemctl --user show -p
LoadState,ActiveState,SubState,MainPID` with the state phrased `active (running)`;
both are null where the supervisor said nothing, and `not-found` reports no state.
`divergent` means the installed unit differs from the one this environment would
write.

#### Why

A systemd unit named `plist` sends an operator looking for a file nobody has.
`show` answers for an unknown unit in the shape of an installed stopped one, which
is why `LoadState` is asked. An environment moved since install leaves a daemon
binding one socket while the shell dials another, which reads as a dead daemon.

### `install` names a daemon that will still hold the port

Where a daemon other than the supervised job answers after the reinstall's own
stop (`launchctl bootout`, `systemctl --user stop`), `install` names it by
version, says the job cannot take the port yet, and names `proxenos stop`. It
never stops that daemon. A reinstall over the supervisor's own daemon, or an
install with nothing answering, prints nothing extra.

#### Why

`run` refuses a held port, so the job would respawn into the same refusal. The
observation is taken after the reinstall's stop because the daemon answering
before it is one this verb ends itself.

#### Tried and dropped

Observing before the stop: every reinstall printed a notice false in both halves.

### Nothing is left half-installed

The unit is written through a rename over a temporary carrying this pid. If the
supervisor refuses it — `launchctl bootstrap`, `daemon-reload`, `enable --now` —
the file is removed, and on Linux the enable is undone first.

#### Why

A `daemon-reload` from any other cause must never read a truncated file.
`enable --now` can leave a `default.target.wants` symlink after a failed start,
and a symlink to a removed file is a complaint on every later reload.

### What a supervisor changes elsewhere

- `status.supervised` (§3) can be true.
- `stop` (§2.4) becomes the way to replace the daemon with the build on disk.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/supervisor.rs` | Unit rendering, labels, `RESTART_SEC`, the supervised reading |
| `crates/proxy/src/commands/supervisor.rs` | The verb, install ordering, refusals, `status --json` |

### 2.7 Client mode

A second machine runs only this CLI and is served by a daemon on the first. It
holds no accounts, credentials or configuration: the control vocabulary goes over
§3's HTTP transport, and `exec` points the client it starts at that daemon.

| Variable | Meaning |
|---|---|
| `PROXENOS_DAEMON` | the daemon's base URL, e.g. `https://macbook.tailnet:8787`. Set and non-empty, this CLI is a client. A URL carrying a user name or password is refused |
| `PROXENOS_TOKEN` | the token that daemon's `listen.token` names |
| `PROXENOS_TOKEN_FILE` | a file holding it. Read only where `PROXENOS_TOKEN` is unset or empty |

### A URL, not a host

The scheme decides whether the hop is encrypted. This project terminates no TLS.

### No configuration key and no flag

Client mode is only these variables. There is no `--token` on any verb.

#### Why

`config.toml` on the client machine is a daemon's configuration shape, and a
client reading `port` and `[tiers]` from it would read settings nothing there
applies. An environment variable is the form a per-shell or per-pane choice can
take. argv is visible in `ps`.

### `status.daemon_at`

In client mode `status` carries `daemon_at`, the URL this CLI dialed, added by
the CLI. Absent for a local daemon.

### Refused in client mode

Each with a sentence naming the URL and saying to run it on that host.

| Verb | Why |
|---|---|
| `run`, `start` | bind a port on this machine; aimed elsewhere they would start a second daemon here |
| `accounts login` | runs the owning program's login and reads the profile here; the daemon would never see it |
| `accounts add-key` | writes a credential file the remote daemon never reads |
| `supervisor` | writes and reports on a unit on this machine |

### `stop` is allowed

The daemon acts on it itself, and an operator who can move its serving account
over the same transport can stop it. What client mode cannot do is start it again,
and `stop`'s sentence says so.

### Everything else

| Verb | In client mode |
|---|---|
| `status`, `accounts`, `accounts use`, `accounts rename`, `accounts remove`, `models`, `incidents`, `tiers`, `tiers set`, `tiers cross-account`, `effort`, `effort set`, `usage`, `reload`, `env`, `settings`, `exec`, `statusline` | go to the remote daemon |
| `doctor` | runs in the CLI against the fixture corpus; never needed a daemon |
| `inspect` | reads a process on this machine. The `daemon` it reports is that process's `ANTHROPIC_BASE_URL` |
| `record ingress`, `record upstream` | not refused: they start a daemon on this machine, from this machine's configuration |
| `record surface` | runs here, against this machine's store |

`--persist` writes the configuration on the daemon's machine.

### What `exec` sets

`ANTHROPIC_BASE_URL` is the URL this CLI dialed. `ANTHROPIC_AUTH_TOKEN` carries
the token, beside the `--account` tag where given:
`proxenos-token:<secret> proxenos-account:<name>` (§1). Both are set on the child
and never printed.

### What `env` and `settings` do

`env` rewrites the base URL. With a token, it leaves the `ANTHROPIC_AUTH_TOKEN`
export out and prints comments instead, including the line to set it from the
variable this process read:

```sh
# ANTHROPIC_AUTH_TOKEN is left out: it would carry this daemon's token. Set it with
#   export ANTHROPIC_AUTH_TOKEN="proxenos-token:$PROXENOS_TOKEN"
# or start the client with `proxenos exec`, which sets it without printing it.
```

`settings` is refused in client mode with a token.

#### Why

Exports are pasted into a shell and its history. The settings document is one
blob a client reads whole: it would either carry the secret on stdout or not
work. `exec` sets both halves without printing either.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/control/mod.rs` | `Endpoint`, the three variables, `refuse_remote`, `call_http` |
| `crates/proxy/src/commands/launch.rs` | Client-mode `env`, `settings`, `exec` |
| `crates/proxy/src/commands/inspect.rs` | `daemon_at` |

---

### 2.8 `inspect`

```
proxenos inspect PID [--json]
```

Says which account another process's turns go as, read from that process's
environment. It needs no daemon and parses the auth-token value with the same
function the daemon reads a request's header with (§1).

| Platform | Where the environment is read |
|---|---|
| Linux | `/proc/<pid>/environ`, NUL-separated |
| macOS | `ps -Eww -o command= -p <pid>`: the command, then the environment. `ps` shows only the caller's own processes |

```json
{ "pid": 4242, "through": true, "account": "work-codex", "daemon": "http://127.0.0.1:8787" }
```

- `through` is true when `ANTHROPIC_AUTH_TOKEN` carries the account tag or a
  token (§1), or holds `unused` beside an `ANTHROPIC_BASE_URL`.
- `account` is the tagged name, null where the launch tagged none.
- `account` and `daemon` are null where `through` is false.

```
pid 4242: through proxenos as work-codex (http://127.0.0.1:8787)
pid 4242: through proxenos as the serving account (http://127.0.0.1:8787)
pid 4242: not through proxenos
```

- No token is ever printed; the parsed token is dropped where it is read.
- `no process <pid> is running` and `the environment of process <pid> could not
  be read` are refusals, not `not through proxenos`. On macOS a command line with
  no assignment after it is the second.

#### Why

Every program that wanted the answer used to match `proxenos-account:` itself,
somewhere it could not be kept in step with the daemon. Needing no daemon is what
makes it usable in a client-mode pane, on a machine whose daemon stopped, or in a
sweep. `unused` alone points at nothing and is not a launch. An untagged launch
goes as whoever is serving, which is `status`'s question. The two refusals call
for different next steps.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/commands/process.rs` | Reading the environment per platform, refusals, rendering |
| `crates/proxy/src/process.rs` | Deciding `through`, `account`, `daemon` |

---


## 3. Control socket

JSON-RPC 2.0. One request per line on a Unix domain socket, or a named pipe on
Windows. The same vocabulary is served over HTTP at `POST /control` (below).

### Where the socket lives

`$PROXENOS_HOME/proxenos.sock` when that variable is set, else
`$TMPDIR/proxenos.sock`, with `/tmp` where no temporary directory is named. The
daemon's bind and every CLI call use one derivation. The socket is created `0600`.

#### Why

`PROXENOS_HOME` isolates a daemon from the operator's own. While the socket
ignored it, an isolated CLI sharing a `TMPDIR` reached the real daemon, and every
login path ends in `accounts.select`.

### An unaddressable path is refused by name

Both bind and dial check the path against `sun_path` — 104 bytes on macOS, 108 on
Linux, one of them the terminator — and refuse naming the path and the cap.

#### Why

A too-long path fails the bind while the HTTP port comes up fine: a daemon that
serves turns and answers no verb.

### Errors

| Code | Meaning |
|---|---|
| `-32700` | malformed request |
| `-32600` | not JSON-RPC 2.0 |
| `-32601` | unknown method |
| `-32000` | the method refused; `message` says why |

An unknown method reaches the caller as an unknown method on both transports.

#### Why

"This daemon does not have that method" is answered by replacing the daemon;
"that method refused" is not.

### Over HTTP

`POST /control`, one JSON-RPC request per body, one response object back. Same
dispatch, same result, same code. A JSON-RPC failure is a 200 carrying `error`;
the HTTP status is only about reaching the endpoint. Served on both doors, and on
the remote door every request carries the token (§1). The handler reads nothing
about the caller.

#### Why

No method may behave one way over the socket and another over HTTP. This endpoint
holds `accounts.remove` and `accounts.select`, so an unguarded one on a reachable
address would be worse than an unguarded ingress. The loopback door is open
because a local CLI holds no token.

#### Tried and dropped

A peer-address check in the handler. With two doors it answered the wrong
question — a tokened daemon's loopback door is legitimately open — and a peer
check beside a door check invites reasoning from the wrong one.

### Methods

The bound set is `METHODS` in `control/protocol.rs`, and `status.methods` lists it
at runtime. A method's section says what it takes and returns. "Added after vX" is
a capability a caller checks for (§6), not a version to compare.

### `status`

No parameters.

| Field | Meaning |
|---|---|
| `port`, `base_url` | the daemon's port and loopback URL |
| `auth.connected` | there is a credential to spend, of either kind |
| `auth.dead` | the credential cannot be spent as it stands: unreadable, or lapsed and waiting on the program that owns the profile (`proxy-behavior.md` §8.4) |
| `auth.refused` | the backend's own words where it turned the credential away; null otherwise |
| `auth.account` | what this daemon calls the serving account; what selects it |
| `auth.account_id` | what the backend calls it |
| `auth.kind` | `grant` or `key` |
| `auth.provider` | `codex` or `anthropic` |
| `auth.key_flavour` | on a key only, where recorded: `subscription_token` or `api_key` (`proxy-behavior.md` §8.2). Absent otherwise |
| `auth.source`, `auth.identity_changed`, `auth.email`, `auth.expires_at` | where the account was read from, whether it became a different account, its address, its expiry |
| `auth.plan`, `auth.plan_source` | plan, and `backend` or `grant`; null where neither said |
| `auth.login_expires_at` | when the operator must sign in again; null where no such date exists |
| `auth.accounts` | every account, as `accounts` lists them. Present and empty rather than absent |
| `tiers` | the mapping in force |
| `effort_ceiling` | null for no ceiling |
| `unlisted_tiers` | mapped models the catalog knows but withholds |
| `missing_tiers` | tiers whose stated model this account's catalog does not carry. Present and empty |
| `cross_account_tiers` | whether a tier may pin another account |
| `catalog_authoritative`, `catalog_curated`, `catalog_stale`, `catalog_account` | whether the catalog is the backend's, the curated relay list, not this account's, and whose it is |
| `methods` | every method this build answers |
| `version` | the build id (below) |
| `instance` | an id minted when the process started |
| `pid` | this process |
| `supervised` | `true`, `false`, or null (below) |
| `client` | the client policy in effect: `deny_skills` as a launch would apply it, `disable_connectors`, `disable_remote_control` |
| `recording` | whether a capture is running |

`auth` holds only `connected` and `accounts` where nothing is selected. No tokens
appear anywhere.

#### Why

A dead grant leaves `connected` true while every turn fails; without `dead` a
front-end shows a healthy provider. `key_flavour` absence is reported, not
resolved into the likelier value, and `subscription_token` is the shape's answer,
not the credential's: `sk-ant-oat` is worn by a setup token and by the harness's
OAuth access token alike. `methods` lets a front-end establish a method exists
instead of comparing versions.

### `status.supervised`

- **macOS**: read from the job label launchd hands the process. `proxenos.daemon`
  is true, no label is false, any other label is null.
- **Linux**: no `INVOCATION_ID` is false. With one, the manager is asked once at
  startup whether `MainPID` of `proxenos.service` is this pid. Unreachable manager
  is null.
- Everywhere else: null.

The reading is taken once at startup and carried for the life of the process.

#### Why

"Not supervised by `proxenos.daemon`" and "nothing supervises this" are different
statements. A terminal emulator is itself a user unit, so `INVOCATION_ID` alone
says only that some unit started the process; the unit is `Type=simple` running
this binary, so `MainPID` equality is identity. Nothing adopts a running daemon
into a unit, so there is no later answer to re-read.

### `status.version`

The version number, `+`, the short commit, and `-dirty` where the tree had
uncommitted changes: `0.12.0+ab12cd3`, `0.12.0+ab12cd3-dirty`, `0.12.0+unknown`
where there was no git. Compare for equality or not at all; never parse it. The
same string is what `--version`, the `daemon` line of `status`, `stop` and
`supervisor install` print. It is not `[upstream].client_version`.

#### Why

Two builds of one version number are different strings, which makes "the binary
is new and nothing changed" answerable. §6 already forbids comparing versions.

### `accounts`

No parameters. Returns `selected`, `accounts`, `discovered`, and
`ignored_grants`.

- Each account: its name, kind, provider, `declared` (true only for a profile in
  `[profiles]`), whether it serves turns, and, for a borrowed row, the profile it
  was read from and `login_expires_at` for a Claude profile.
- A borrowed row whose store could not be read carries `unreadable`, the refusal's
  words. The key is **absent** otherwise, so absent means readable.
- `discovered` is whether the rows are the operator's `[profiles]` or the stock
  profile of each program, read because none were declared.
- `ignored_grants` names grants an older version left in `credentials.json`.

No tokens.

### `accounts.select`

`{"account": name}`. Returns `selected`, `provider`, `previous_provider` (absent
where nothing was selected), `catalog_refreshed`, and `tiers`, the mapping now in
force. Selecting the account already serving returns
`{"selected", "provider", "previous_provider", "unchanged": true}` and does
nothing else.

- The account's own tiers and ceiling (§4) are resolved and validated against the
  catalog fetched for it before anything moves. A model that catalog lacks refuses
  the switch, naming whose menu refused and how to give that account its own
  mapping; the daemon keeps serving what it was, catalog included.
- Validation is skipped where the catalog cannot speak for the account: the
  fallback list, or a failed refetch. `catalog_stale` then says so.
- The ingress authenticates through the same store, so the next turn is made as
  the account named.
- Live conversations are dropped, each paying a full upload on its next turn.
- Quota figures stay under the accounts that earned them.

#### Why

A conduit fixes its account when it dials and keeps the connection for the
conversation's life (`proxy-behavior.md` §4.1), so a session left alone would go
on billing the account the operator moved off. A full send is the direction §4.3
of the behavior spec resolves every ambiguity toward. A fetch that did not answer
is not evidence a model went away. A re-selection is already where a switch pays
to arrive.

### `accounts.rename`

`{"account": from, "name": to}`. Returns `renamed`, `name`, and
`moved_configuration`. The grant and account id are untouched.

- An account section in `config.toml` moves with the name; only the table headers
  change, everything under them byte for byte. The file is written before the
  store.
- An account with no section is renamed without touching the file.
- A rename onto a name whose section is still in the file is refused.

#### Why

A section keyed by the old name would detach silently, since a section naming
nobody is not an error. The file-first order can at worst leave an orphan
section; the other order can leave an account with no mapping. Removing an account
leaves its section, and moving onto it would define one table twice, which TOML
refuses at the next start.

### `accounts.remove`

The selected account, or `{"account": name}`. Returns `removed`, `serving` (who
serves afterwards), `remaining`, and `catalog_refreshed`.

- A key is dropped from the store.
- A declared profile loses its `[profiles]` entry, then the file is re-read. The
  grant belongs to the program that owns the directory.
- A found profile is refused, saying so and that `[profiles]` is empty.
- The removed account's quota figure is dropped. An idle account's removal leaves
  the serving account's figure alone.
- Where removal hands over to another account, the catalog is fetched again.

#### Tried and dropped

`disconnect`, then `accounts.forget`. The store's verb and every refusal say
"remove"; a third spelling was a name to translate.

### `models`

Optional `{"account": name}` (added after v0.16.0). Returns `models` (each `id`,
`context_window`, `effective_window`, null where unknown), `authoritative`, and
`stale`.

- For an account on the second provider — named, or the selection — the answer is
  a list built into the binary with windows, and carries `curated: true` and
  `provider`. No mapping is ever validated against it.
- Otherwise it is that account's catalog, fetched as that account where the list
  in force was not fetched for it or is the fallback. Nothing is put in force by
  it; a failed fetch answers with the list in force, `stale: true`.
- A name the store does not hold is refused by name.

`exec --account` measures `--model` against this.

#### Why

The fetched catalog was never the second provider's menu (`proxy-behavior.md`
§9.1), and that provider's list endpoint states no windows.

### `tiers`

Optional `{"account": name}`. Returns `tiers`, `missing_tiers`, and
`cross_account_tiers` (added after v0.19.0; `missing_tiers` after v0.17.0).

With a name other than the serving account, the answer is that account's section
as the file holds it now, resolved as a switch would, with `account` naming it and
no `missing_tiers`. The serving account's name answers as without the parameter.
An unknown name is refused.

#### Tried and dropped

`tiers.get`. Every other read is a bare noun beside namespaced writers.

### `tiers.set`

`{"tiers": {<tier>: value, ...}, "account"?: name, "persist"?: bool}`. Returns
`tiers`, `persisted`, `account` (null for the shared table), and `detail`.

A value is a model id, or `{"model", "account"?, "effort"?}`: `account` pins the
tier, `effort` is the level the client starts the model at (§2.2).

- **Partial**: naming one tier changes that tier.
- Each model is validated against the catalog and **refused** where the catalog
  lacks it. A pinned model is excluded from validation.
- The pinned form needs `cross_account_tiers` and is refused by name without it.
- An unrecognized effort is refused naming the tier; two tiers on one model with
  different efforts are refused naming both.

#### Why

A caller that knows one tier must not unset the three it did not mention. This
daemon owns the mapping because it holds the catalog. A set is feedback on
something typed a moment ago, so it refuses where a start or reload only marks.
The catalog is the serving account's menu and cannot speak for a pinned one.

### `effort.set`

`{"effort": level | null, "account"?: name, "persist"?: bool}`. Returns `effort`,
`persisted`, `account`, and `detail`. `null` removes an override: under an account
it clears that account's line and the shared ceiling applies again. The answer
reports the ceiling that results.

#### Why

Reporting no ceiling after clearing an account's line would be a figure that
lasted until the next start.

### Setters: persistence and scope

These rules hold for `tiers.set` and `effort.set`.

- **In effect until the daemon stops**, unless `persist` is true. Every answer
  says which.
- **Written before applied.** A failed write leaves the daemon unchanged.
- **Written where the value is read from.** With no `account`, a tier goes to the
  serving account's section where that section already names it, else the shared
  table; the ceiling follows the same rule. With `account`, that section.
- **Aimed at a non-serving account: written, not applied**, and not validated
  against the serving catalog. Without `persist` such a call is refused. `detail`
  distinguishes written-and-applied from written-only.
- **A text edit, not a re-serialization.** One value on one line changes; the file
  is read fresh at write time.
- **Account tables are re-read from disk when needed.** A file that no longer
  parses keeps the startup snapshot.

#### Why

Trying a mapping is not changing what the daemon is, and only the caller knows
which it is doing. Applying before writing would leave a policy nobody chose,
reported as a failure. An account section shadows the shared table, so a write to
the shared table would be in force now and gone at restart. The file's comments
explain why keys are what they are; re-serializing would silently discard them. A
daemon that resolved its own writes from a startup snapshot could not see them.

### `cross_account_tiers.set`

`{"enabled": bool}`. Always persisted. Granting applies to the next call.
Revoking is refused by name while any tier pins an account.

#### Why

Consent changes what the daemon is, and a grant that evaporated at restart would
leave the file refusing a mapping the operator permitted. Revoking under a pin
would write a file the daemon refuses to start from.

### `config.reload`

No parameters. Re-reads `config.toml` and returns `reloaded`, `serving` (null
where the file took the serving profile away), `remaining`, and `needs_restart`.

- Applies `[profiles]`, the tier mapping and the effort ceiling. The mapping goes
  through the checked path a switch takes, except a model the catalog lacks
  **marks** its tier instead of refusing.
- `needs_restart` is always `instructions`, `client`, `transport`, `upstream`,
  `port`.
- Nothing is fetched. A conversation in flight keeps what it started with.
- A file that does not parse is refused with the parse error; the daemon keeps what
  it was running.

#### Why

A reload is the move left after a daemon came up with a tier marked, so it cannot
refuse on the same grounds. A key that did nothing must be named rather than
discovered.

### `usage`

No parameters. Returns the serving account's quota as of its last turn, or that no
turn has been made, plus:

- `models`, the ids this daemon serves (§2.1);
- `serving`: name, provider, address, plan, account id;
- `incidents`, the same list the `incidents` method returns;
- `accounts`, one entry per stored account with its figure, its freshness, and
  `unavailable` where it has none. Each entry carries `served_tokens`
  (`proxy-behavior.md` §6.1). An entry with no figure carries `reason` beside
  `detail`: `no_turn`, `no_relayed_turn`, `metered`, `unknown_key_kind`,
  `not_reported`.

Each window carries `used_percent`, `window_minutes`, `resets_at`, and where the
provider stated them `status`, `surpassed_threshold`, `representative`, and
`label` for a window no duration identifies. An entry with a credit balance
carries `credit`: `used_minor`, `limit_minor`, `exponent`, `currency`, `percent`,
`severity`. An entry whose subscription is not active carries
`subscription_status`, the provider's word; absent where active.

#### Why

`reason` is the fact in a word, so a renderer never matches on prose. Staleness is
per window, since one snapshot can hold a turned-over five-hour window beside a
current seven-day one.

### `usage.refresh`

No parameters. Asks the backend for a figure now, per stored account whose
credential can hold one, each on its own credential and recorded under its own
name as asked-for. Returns the serving account's outcome plus `accounts`, each
with its figure or the sentence why not. Nothing about the selection moves.

- A failure belongs to its own entry.
- A key, or a credential whose provider states quota only on turns, is not asked.
- A non-serving account's expired grant is never refreshed; its row says so and
  what to do. The serving account is the exception.
- Where a borrowed profile's grant has lapsed, the owning client runs once first,
  and the call waits. **The budget is one client run for the whole call.** An
  account past the budget is asked without the refresh, and its row says so. Runs
  per profile are serialised by a lock. A profile whose refresh token has also
  lapsed is refused.

#### Why

The stream snapshot is free and primary. This exists for a daemon that has served
no turn and for an idle account whose headroom is the question before switching
to it. A refresh rotates a token family, and a second holder would be left with a
retired token. Neither the socket nor the CLI times out, so four lapsed profiles
at one run each would look hung. Running the client over a lapsed refresh token
would blank what is left of the grant.

### `incidents`

No parameters. Returns:

- `incidents`, worst first: each `id`, `provider`, `name`, `status`
  (`investigating`, `identified`, `monitoring`; resolved rows are dropped),
  `impact` (`none`, `minor`, `major`, `critical`), `url`, `since`, and `updates`
  (added after v0.27.0) — newest first, each `status`, `body`, `at`, present and
  empty where nothing was posted;
- `providers`, the ones asked;
- `errors`, per provider whose page did not answer, with its last list kept;
- `checked_at`, epoch seconds of the last round, null before the first.

The daemon asks each page once a minute (`upstream.status`,
`upstream.anthropic.status`, §4). Nothing on the turn path waits on it. The CLI
table omits `updates`.

### `env`

Optional `{"account": name}` (added after v0.15.1). Returns `variables` and
`settings` (§2.2).

#### Why the name stayed

The payload carries more than an environment, but its halves are named inside it,
and renaming the method would cost a caller a shim for no capability. `settings`
is the CLI name for the document.

### `shutdown`

No parameters. Returns `{"stopping": true, "version": ...}`, then the process goes
once the answer is written.

### `record.start` and `record.stop`

`record.start` takes `{"mode": "ingress" | "upstream"}`, `ingress` by default, and
returns `{"recording": true, "mode"}`. `record.stop` returns
`{"recording": false}`.

#### Why

`upstream` bills every turn that follows, so it has to be named.

### `doctor`

Reserved and not implemented; it answers that it is not implemented rather than
unknown. `doctor` runs in the CLI, where `--live` can resolve credentials without a
daemon.

### Front-ends

The daemon holds authoritative state and every front-end is a client of this
interface. The CLI has no privileged path.

### Operator-facing rows name the provider id

Rendered output names a provider by its stored id, `codex` or `anthropic`: the
`routing`, `catalog` and `auth` lines of `status`, the curated note on `models`,
and every per-account reason in `usage`.

#### Why

With two providers stored, a row that leaves the provider out is the one an
operator has to guess about.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/control/protocol.rs` | Request and response shapes, codes, `METHODS` |
| `crates/proxy/src/control/handler.rs` | Every method |
| `crates/proxy/src/control/mod.rs` | Socket path, bind, dial, HTTP client |
| `crates/proxy/src/ingress.rs` | `POST /control` |
| `crates/proxy/src/config/edit.rs` | Text edits for persisted changes |
| `crates/proxy/src/usage.rs` | Quota figures, refresh sweep |
| `crates/proxy/src/incidents.rs` | Status-page polling |
| `crates/proxy/src/catalog.rs` | Catalog, fallback, curated relay list |
| `crates/proxy/src/version.rs` | The build id |

---

## 4. Configuration

TOML at `config.toml` in `$PROXENOS_HOME`, else `$XDG_CONFIG_HOME/proxenos`, else
`~/.config/proxenos`. The file is optional and every key has a default.

```toml
port = 8787
cross_account_tiers = false

# Optional. A ceiling on reasoning effort, whatever the client asks for.
effort = "low"

# Optional. The Claude and Codex CLIs this daemon runs on its own behalf.
claude_program = "/opt/homebrew/bin/claude"
codex_program  = "/opt/homebrew/bin/codex"

[tiers]
opus   = "gpt-5.6-terra"
sonnet = "gpt-5.6-luna"
haiku  = "gpt-5.6-luna"
fable  = "gpt-5.6-sol"

# Optional, one per account, keyed by the name `accounts` lists it under.
[accounts.spare]
effort = "low"

[accounts.spare.tiers]
opus = "..."

# Optional. A second door beside the loopback one, and the token it demands.
[listen]
address    = "100.64.0.2"
token_file = "/Users/me/.config/proxenos/token"
# token    = "a-long-random-string"

[transport]
websocket   = true
compression = true

[instructions]
identity       = true
working_budget = true
append         = "..."

[client]
deny_skills                = ["claude-api"]
disable_connectors         = true
disable_remote_control     = true
disable_commit_attribution = true

[upstream]
client_version           = "2.0.0"
effective_window_percent = 95.0
endpoint                 = "https://chatgpt.com/backend-api/codex/responses"
websocket                = "wss://chatgpt.com/backend-api/codex/responses"
catalog                  = "https://chatgpt.com/backend-api/codex/models"
usage                    = "https://chatgpt.com/backend-api/wham/usage"
status                   = "https://status.openai.com/api/v2/summary.json"

[upstream.key]
endpoint = "https://api.openai.com/v1/responses"
catalog  = "https://api.openai.com/v1/models"

[upstream.anthropic]
endpoint = "https://api.anthropic.com/v1/messages"
usage    = "https://api.anthropic.com/api/oauth/usage"
profile  = "https://api.anthropic.com/api/oauth/profile"
status   = "https://status.claude.com/api/v2/incidents/unresolved.json"

# Optional. The profile directories grants are borrowed from, keyed by the
# name the account is filed under.
[profiles.work]
provider = "codex"
path     = "/Users/me/Library/Application Support/Agent Profiles/codex/p/997619b5"

[profiles.personal]
provider = "anthropic"
```

The `[tiers]` and `[upstream*]` values shown are the defaults.

### A missing file is a first run

The daemon logs where the file would go and starts on the defaults. A file present
but unparseable is an error.

#### Why

Falling back there would run a daemon that ignores what the operator wrote.

### An unrecognized key is refused

Every table refuses unknown keys, and the error says top-level keys must sit above
`[tiers]` and `[transport]`.

#### Why

In TOML a bare key after a table header belongs to that table: `effort` below
`[tiers]` is `tiers.effort`. Ignored, the operator believes they capped spending
while every request runs at the backend's default.

### When the file is read

At startup, on `config.reload` (§3 lists what that moves), and, for account
tables, whenever a setter needs them. Nothing watches it. Outside the file, `--port`
or `PROXENOS_PORT` overrides `port`, and `PROXENOS_HOME` moves the directory. The
token estimator is a compile-time feature (`--features tokenizer`), not a key
(`proxy-behavior.md` §6.3).

### `port`

The port both doors bind. Default 8787.

### `[tiers]`

The four tiers, each a model id or a table (below). An omitted tier takes its
default; a tier written blank is refused.

Each mapped model is checked against the catalog at startup, on
`accounts.select`, and on `config.reload`. A model the catalog lacks does not stop
the daemon:

- A **defaulted** model is replaced with one the account has.
- A **stated** model is never replaced; it marks its tier. A marked tier refuses
  its own turns, naming the tier, the model, and what the catalog has. The other
  tiers serve. The startup log says it once at WARN, `status` and `models` name
  the tier, and `reload` clears the mark once the file is fixed. Where every tier
  is marked the daemon still starts (`proxy-behavior.md` §7.1).

#### Why

An omission accepts the shipped answer; a blank is a mistake. A defaulted model is
this proxy's guess, a stated one is the operator's decision. A daemon that exited
could not be reloaded.

### Tier table: `account`

`haiku = { account = "spare", model = "..." }` serves that tier's turns as
`spare`. Gated by top-level `cross_account_tiers = true`; without it the daemon
refuses to start. A pin naming an account not stored refuses the turn with
`invalid_request_error`, naming it and listing what is stored; a pinned account
holding a credential the endpoint does not take is refused the same way.
`proxy-behavior.md` §7.1 has the rest.

#### Why

A pin routes one client's traffic across accounts' quotas, which the operator
owns. Falling back to the serving account would spend the wrong quota invisibly.

### Tier table: `effort`

`opus = { model = "…", effort = "high" }`, with or without `account`. One of the
client's own levels: `low`, `medium`, `high`, `xhigh`, `max`. Anything else refuses
the daemon naming the tier. Two tiers on one model must agree. Delivered in the
launch settings as that model's effort (§2.2). It is what a session asks for when
it names none; the ceiling still caps it, so `high` under a `medium` ceiling is
served at `medium`.

#### Why

A backend level the client lacks (`none`, `minimal`, `ultra`) would reach the
client as a setting it refuses. The client keeps one effort per model.

### `cross_account_tiers`

Top-level. Consent for pinned tiers. Default false. `cross_account_tiers.set`
writes it.

### `effort`

Top-level ceiling on reasoning effort for every request: `none`, `minimal`, `low`,
`medium`, `high`, `xhigh`, `max`, `ultra`. `ultracode` is read as `xhigh`. Capped
again by what the model accepts, and raised to the model's lowest listed effort
where below it (`proxy-behavior.md` §2.7). Omitted means the backend's default,
not zero. An unrecognized value is refused.

#### Why

`ultracode` is Claude Code's session mode that runs at xhigh with workflows on top,
not a level.

### `[accounts.<name>]`

What differs for one account. `[accounts.<name>.tiers]` replaces the tiers it
names and no others. `effort` replaces the shared ceiling rather than being capped
by it. Keyed by the name `accounts` lists. An account with no section, and a daemon
with nothing selected, take the shared tables.

#### Why

A catalog is one account's menu (`proxy-behavior.md` §7.0): two plans offer
different models, and a key beside a subscription need not overlap at all. A key
account has no id to be keyed by.

### `claude_program` and `codex_program`

The Claude and Codex CLIs this daemon runs on its own behalf, never to serve a
turn. `claude` is run to refresh a borrowed Anthropic grant and to read the version
its quota request is made as; `codex` runs a cheap `codex exec` turn to refresh a
borrowed Codex grant (`proxy-behavior.md` §8.4). Unset, the bare name resolves
through the daemon's `PATH`.

#### Why

The daemon's `PATH` is not the shell's. A daemon launchd started inherits a minimal
one where the bare name does not resolve, and `usage --refresh` then refuses with
`could not run \`claude\``.

### `[profiles.<name>]`

Where another program keeps a grant this daemon spends (`proxy-behavior.md` §8.4).
`provider` is `codex` or `anthropic`. `path` is the profile directory — a
`CODEX_HOME` or `CLAUDE_CONFIG_DIR`. No credential is written here or read from
here.

- **No `path` means the stock profile**, which differs from naming the stock
  directory explicitly.
- A path must be absolute; a leading `~` is refused, not expanded.
- An empty name is refused.
- Two entries on one provider and one directory are refused, naming both. The same
  directory under different providers is two profiles.

#### Why

On macOS the Claude client files its grant under a keychain item chosen by whether
`CLAUDE_CONFIG_DIR` was set at all, so writing the path out selects a different
item. The daemon's working directory is not the operator's, and for a Claude
profile the spelling of the path is part of the identity. One directory holds one
grant, so it is one account.

### `[listen]`

| Key | Meaning |
|---|---|
| `address` | a door to add beside loopback (§1). Default `127.0.0.1` |
| `token_file` | a file holding the token. Preferred |
| `token` | the token inline |

- `address` names the door to **add**, not the address to move to.
- A non-loopback `address` with no token is refused at startup, naming both keys.
- A wildcard (`0.0.0.0`, `::`) is refused by name.
- `token_file` is refused when group- or world-readable, naming the mode and the
  `chmod`.
- Both `token` and `token_file` is refused.
- A token beside a loopback `address` logs a WARN at startup and is not an error.
- `port` stays top-level.

#### Why

`listen.token` is the one secret this file can hold. It is not a credential for
anything upstream: it gates this daemon, is minted by the operator, and decides
whether the daemon answers at all, so it has to be readable first. A token file can
be `0600`, rotated without editing configuration, and excluded from backups. A
readable token file works while every account on the machine has it. Two stated
tokens leave no way to tell which the daemon took. A wildcard covers
`127.0.0.1`, so the two doors cannot both be bound, and the result is
platform-dependent: measured on macOS 15, the BSDs bind both and hand loopback to
the more specific socket, while Linux refuses the second bind. A loopback address
for an afternoon should not mean deleting the token. `port` shipped top-level and
§6 forbids moving a key.

### `[transport]`

| Key | Default | Meaning |
|---|---|---|
| `websocket` | true | use the WebSocket transport where the account supports it; false is HTTP only |
| `compression` | true | zstd on HTTP bodies, `permessage-deflate` on the socket |

`proxy-behavior.md` §4 has the transports.

### `[instructions]`

What the proxy puts around the client's system prompt (`proxy-behavior.md` §2.1).
All three must stay constant for a conversation.

| Key | Default | Meaning |
|---|---|---|
| `identity` | true | lead with one line naming the model actually answering |
| `working_budget` | true | a short block, after the client's prompt and before `append`, asking the model to read the smallest slice that answers and act once a read is enough |
| `append` | unset | operator text after the system prompt |

#### Why

A model told it is a different product is given a false premise every turn. The
conversation is replayed upstream every turn and echoed back three times, so broad
reading spends the window fast. `append` outranks `working_budget` because an
operator wrote it on purpose. Text changing between turns costs every delta and
every cache hit.

### `[client]`

Policy the client applies to itself (`proxy-behavior.md` §7.3).

| Key | Default | Delivers |
|---|---|---|
| `deny_skills` | per launch | `permissions.deny` with `Skill(...)` rules. Unset: `claude-api` denied for a launch whose turns translate, nothing for one wholly relayed. A written list applies on either path; empty allows everything |
| `disable_connectors` | true | `disableClaudeAiConnectors` in settings, and `ENABLE_CLAUDEAI_MCP_SERVERS=false` in the environment |
| `disable_remote_control` | true | `remoteControlAtStartup: false` |
| `disable_commit_attribution` | true | `attribution.commit: ""` |

`status` reports the deny list a launch would apply.

#### Why

A rule built by hand and built wrong denies nothing and says nothing. The skill
documents the second provider's API: wrong for a translated session, right for a
relayed one. The connector setting suppresses the notice the client prints whenever
an auth token is set, which here is always; the export still reaches a client
launched from `env` alone. A session started through a local proxy is a local
decision, so remote control is off. Which model served a turn is not a commit
message's business.

### `[upstream]`

Where the first provider is reached, so a pinned binary can be repointed rather
than rebuilt.

| Key | Meaning |
|---|---|
| `client_version` | the client version reported when fetching the model list |
| `effective_window_percent` | share of a context window left usable, where the catalog states none. Must be in `(0, 100]`, refused otherwise |
| `endpoint`, `websocket`, `catalog` | the subscription backend's HTTP, socket and model-list URLs |
| `usage` | where a quota figure is asked for rather than waited for |
| `status` | the provider's status page, polled for `incidents` |

#### Why

The backend filters the catalog by `client_version`, and a version below every
model's minimum returns an empty list, not an error, which reads like an account
with no models; startup says so by name. `effective_window_percent` is the figure
the client is told, so it decides when compaction fires: lower wastes window,
higher risks a turn refused for length.

### `[upstream.key]`

Where an API key is spent (`proxy-behavior.md` §8.2): `endpoint` and `catalog`. No
socket: a key account uses HTTP. Sending either credential kind to the other's
endpoint is refused before anything leaves.

### `[upstream.anthropic]`

The second provider.

| Key | Meaning |
|---|---|
| `endpoint` | where a relayed turn goes (`proxy-behavior.md` §9) |
| `usage` | where that provider states quota for a borrowed grant. Only a grant can ask |
| `profile` | where it states the plan with its multiplier (`max 20x`); asked beside a quota refresh, at most hourly. A declined answer leaves the plan absent |
| `status` | the status page, polled for `incidents` |

#### Why

A key has no subscription behind it, and the long-lived setup token wearing the
same stem is refused there for want of a scope. The relay speaks the surface this
proxy already exposes, so there is no catalog to translate.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/config.rs` | Every key, defaults, validation, directory resolution |
| `crates/proxy/src/config/edit.rs` | Persisted text edits |
| `crates/proxy/src/commands/daemon.rs` | Startup checks: listen, tiers, marks |
| `crates/proxy/src/incidents.rs` | Default status-page URLs |

## 5. Limitations

Each is permanent under the current design.

- **The context percentage Claude Code displays is wrong.** It is computed
  client-side against an assumed window. Token counts are exact; the percentage is
  not.
- **Sessions compact earlier than necessary**, for the same reason. The assumed
  window sits below the real one, which is the safe direction.
- **`cache_creation_input_tokens` is always zero.** No upstream write event exists.
- **`count_tokens` is an estimate**, from the conversation's own estimator:
  uncalibrated before its first completed request, never exact, since there is no
  upstream counting endpoint.
- **`cache_control` and `thinking` blocks are dropped** on the request path.
  Reasoning is reconstructed on responses from summary events.
- **Image URLs are not prefetched** and resolve only if the backend can reach them.
- **The catalog fallback list is fixed.** Its entries carry no window, so the window
  guard does not fire for a model it named.
- **The credential directory must be on a filesystem that locks.** A write that
  cannot take its lock fails, naming `PROXENOS_HOME`.
- **A key account's catalog carries no windows or efforts.** The window guard never
  fires for it and the model half of the effort cap has nothing to cap against. The
  configured ceiling still applies.
- **Claude Code never reaches the `input_file` path.** It rasterises PDFs into
  image blocks. The `document` translation is for a client that sends one, and the
  backend accepts it — measured by posting a `document` block that returned a code
  existing only inside the PDF.
- **Compression saves bytes and no tokens.** Roughly two thirds off in both
  directions; the inbound half is larger because the backend echoes the request
  three times per turn. A key request is never compressed.
- **A web search with no citations reports the pages the model opened**, with a URL
  and no title. Better than an empty result, which the client reads as "nothing
  found".
- **What a matrix proves depends on what answered it.** Replayed establishes the
  proxy's half; only `--live` establishes the backend's. `roadmap.md` §L records
  what is settled live.

---

## 6. Stability

The CLI verb set, the control-socket method names, the configuration keys, and
the error-type vocabulary are semver-bound. A shipped
name is never repurposed or removed within a major version; only new ones are
added.

### The bound names

- **Methods** (nineteen, `METHODS` in `control/protocol.rs`): `status`,
  `shutdown`, `accounts`, `accounts.select`, `accounts.rename`, `accounts.remove`,
  `models`, `tiers`, `tiers.set`, `effort.set`, `cross_account_tiers.set`,
  `usage`, `incidents`, `usage.refresh`, `env`, `doctor`, `record.start`,
  `record.stop`, `config.reload`. `doctor` is bound though unimplemented.
- **Verbs**: `start`, `run`, `accounts` (`list`, `login`, `add-key`, `use`,
  `rename`, `remove`), `status`, `models`, `incidents`, `env`, `settings`,
  `reload`, `stop`, `tiers` (`set`, `cross-account`), `effort` (`set`), `exec`,
  `doctor`, `usage`, `statusline`, `record` (`ingress`, `upstream`, `surface`),
  `supervisor` (`install`, `uninstall`, `status`), `inspect`.
- **Environment**: `PROXENOS_DAEMON`, `PROXENOS_TOKEN`, `PROXENOS_TOKEN_FILE`.
- **Ingress**: the auth-token tag `proxenos-token:` and `POST /control`.
- **Socket names** in §3 are frozen.

#### Why

A reserved name that appears later must mean what its name said all along. The
method list is a constant in the daemon, so removing one is a visible code change.

### Before 1.0, a wrong name may be renamed

A name that turns out wrong is renamed on a minor bump, said in the changelog, and
removed rather than kept beside its replacement. The exception ends when a second
caller of the socket exists, whether or not 1.0 has been reached.

#### Why

Semantic versioning does not bind a zero major, and so far only this project's own
CLI speaks the socket — one binary, so a rename lands on both halves at once. Once
anything else has to be upgraded in step, only additions are safe.

#### Tried and dropped

`accounts.forget`, renamed to `accounts.remove`. The project's former name
`codex-cc-proxy` and `CODEX_CC_PROXY_*`, renamed to `proxenos` and `PROXENOS_*`
with no aliases; a store or variable under the old name refuses loudly.

### An added field is a capability

Adding a response field is not a breaking change. An older caller ignores what it
does not know and must not be made strict. A newer caller that requires a field
checks for it rather than inferring it from a version string. Where absence would
be ambiguous, the field is emitted empty, so absence means only "this daemon
predates it". `status.daemon_at` follows this rule: absent means a local daemon.

#### Why

A strict older caller makes every upgrade simultaneous. Comparing versions forces a
policy about which differences matter and gets it wrong for a patched build or a
forgotten bump.

### The ingress shape is not ours

It tracks the Anthropic Messages API, and changes there are not breaking changes in
this project's versioning.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/control/protocol.rs` | `METHODS` |
| `crates/proxy/src/cli.rs` | The verb set |
| `crates/proxy/src/config.rs` | The key set, `renamed_home_refusal` |

---

## 7. Posture

The upstream endpoint is not a published or supported API. It may change or be
withdrawn without notice, and using a subscription this way is a decision each
operator makes for themselves.

This project is not affiliated with, endorsed by, or sponsored by Anthropic or
OpenAI. All trademarks belong to their owners.

### No telemetry

Nothing is collected or transmitted. Credentials never appear in process arguments
or logs. Configuration and credential files are created with restrictive
permissions.

### Two doors, one daemon

`127.0.0.1` is always bound and authenticates nothing; every caller reaching it is
already a local process running as the user. A reachable `listen.address` opens a
second listener whose every request must carry the token, and a non-loopback
address with no token is refused at startup (§1, §4). The token decides who may
ask, not what is served.

### The token belongs to the door, not the peer

Nothing reads a request's source address. §1 gives the reasons.

### What the token is and is not

It gates this daemon. It is not a credential for any upstream and authorizes no
spending of its own. A holder can do exactly what a local caller can — serve turns
on the accounts this daemon holds, and move its settings — and is, for every
purpose here, the operator. Rotate it by editing `listen.token_file` and restarting.

### No TLS

A daemon reachable beyond loopback should sit behind a private overlay network or a
reverse proxy that terminates TLS. Over plain `http://` the token crosses the wire
in a header, and so does every turn.
