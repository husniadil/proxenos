# API contract

What proxenos exposes, and what callers may rely on.
[`proxy-behavior.md`](proxy-behavior.md) is the companion spec for how it
behaves internally.

Four surfaces: the HTTP ingress Claude Code talks to, the command line, the
control socket, and the configuration file. The ingress shape is fixed by the
Anthropic Messages API and is not ours to change. The other three are ours, and
the stability rules in §6 apply to them.

---

## 1. Ingress

The daemon binds `127.0.0.1` and refuses any other address. It performs no
authentication: every caller reaching the socket is already a local process
running as the user.

`ANTHROPIC_AUTH_TOKEN` must be set for Claude Code's own sake. Its value is
ignored, with one carve-out: a value of `proxenos-account:<name>` is a launch
tag (`exec --account`, §2.3) naming the stored account that session's turns
are made as. The tag is a name, never a secret, and the credential it resolves
to never leaves the daemon.

| Endpoint | Purpose |
|---|---|
| `POST /v1/messages` | The only endpoint carrying real load. Answers with SSE where `stream` is true, and with one JSON body otherwise. |
| `POST /v1/messages/count_tokens` | Pre-flight sizing. Returns an estimate. |
| `GET /v1/models` | The mapped models, in the Anthropic list shape — `{"data": [{"id", "display_name", "type": "model"}]}`. Ids are the upstream model ids the tiers map to. |

A request to `POST /v1/messages` whose model id belongs to an account on the
second provider is **relayed** rather than translated: the body is forwarded
byte for byte and the reply is streamed back byte for byte, with the bearer
replaced by that account's credential. `proxy-behavior.md` §9 states the rule
and the header delta. Nothing about the endpoint changes — the same URL serves
both paths, and which one a turn takes is decided from the model id it carries.
This path is **confirmed live**: relayed turns round-trip against the real
endpoint of the second provider — plain and streaming, generation and refusal —
with a subscription bearer the relay substituted (`proxy-behavior.md` §9,
`roadmap.md` §L).

`stream` decides the shape of the answer, and its default is the endpoint's:
absent or `false` is **not** a stream, and is answered with a single
`application/json` message body. Claude Code always sets it, so the harness only
ever takes the streaming path; every other local caller gets what it asked for.
The non-streaming body is the frame sequence folded shut — `proxy-behavior.md`
§5.5 — and its field set is held against a captured answer from the real
endpoint. The one field a real answer carries that this one does not is
`stop_details`.

`run` fails immediately if the port is already bound, naming the conflict, rather
than retrying or selecting another port. A second daemon on a different port
would be silently unused by a client already configured for the first.

The ingress imposes **no size limit** on a request body. A real turn — a full
system prompt and a large tool set — runs past the extractor's 2 MB default, and
the backend's own limit is the real one. A 413 from the door is worse than a
large body: it is not an Anthropic error shape, so the client reads it as
retryable and loops on it, the turn never reaching the backend. The daemon is
loopback-only, so the only sender is the user's own client.

### 1.1 Errors

Every failure — including transport and credential failures — returns an
Anthropic-shaped body:

```json
{ "type": "error", "error": { "type": "...", "message": "..." } }
```

| Condition | Type | Status |
|---|---|---|
| Quota exhausted | `rate_limit_error` | 429 |
| Upstream overload or 5xx | `overloaded_error` | 529 |
| Upstream judged the request invalid | `invalid_request_error` | 400 |
| Upstream rejection, otherwise | `api_error` | upstream status |
| Credentials invalid or absent | `authentication_error` | 401 |
| Credentials transiently unavailable | `overloaded_error` | 529 |
| Request exceeds the model's window | `invalid_request_error` | 400 |
| Malformed request body | `invalid_request_error` | 400 |
| Unknown endpoint | `not_found_error` | 404 |

`retry-after` is forwarded when upstream supplies it.

Transient conditions surface as retryable so Claude Code's own backoff drives
them. Terminal conditions surface as terminal. The proxy does not build a second
retry loop on top of the client's.

An error arising mid-stream is emitted as an SSE `error` frame rather than
changing an already-sent status.

On the non-streaming path nothing is written until the turn is over, so the same
failure is a status and an error body rather than a 200 carrying an error frame.
A frame and a status describing one failure never disagree: both are built from
the vocabulary above.

---

## 2. Command line

```
proxenos start      start the daemon in the background, returning once it
                    answers; says what is there and does nothing where one is
                    already running
proxenos run        start the daemon in the foreground
proxenos accounts   stored accounts, and which one serves turns (also
                    `accounts list`; --json prints the socket's own payload)
                    a header table, one row per account: `NAME PROVIDER KIND
                    ACCOUNT SOURCE STATE`, with a `*` on the one serving
                    turns. KIND is `profile` or `key`; ACCOUNT is the address,
                    else the id, else the subscription; SOURCE abbreviates
                    `$HOME` to `~`, reads `keychain` for a keychain and
                    `stored` for a key, and marks a profile nobody declared
                    `(found)`; STATE is one phrase — `refused`, `identity
                    changed`, the renewal countdown, else `ok`. The provider
                    is on every row, since with two stored an unnamed one is
                    a guess
  accounts login   NAME --provider codex|anthropic [--path DIR]
                    [--device-auth] [--relogin]
                    sign in to a new profile of the owning program and
                    declare it; --path says where, absent it goes under this
                    daemon's own directory; --device-auth has the client print
                    a URL and a code instead of opening a browser, and is
                    `codex` only; --relogin signs a profile `[profiles]`
                    already declares back in, against the directory it names,
                    and declares nothing
  accounts add-key NAME --provider codex|anthropic
                    store an API key, read from stdin
  accounts use     NAME   serve every following turn as this account
  accounts rename  OLD NEW   change what this daemon calls an account
  accounts remove  NAME   drop this account, leaving the rest usable
proxenos status     connection, tier mapping, model catalog (--json prints the
                    socket's own payload)
proxenos models     available models, as `MODEL WINDOW TIER` under a header;
                    TIER names the tiers mapping to each id, read from the
                    `tiers` method (--json prints the socket's own payload)
proxenos env        environment for Claude Code, as shell exports
proxenos settings   the same configuration, as one settings document
proxenos exec       run a command with that configuration applied
proxenos reload     re-read config.toml into the running daemon
proxenos stop       ask the running daemon to stop
proxenos doctor     probe backend capabilities (--live answers from the real one)
proxenos usage      what quota is left (--refresh asks, per account), as a
                    header table: `NAME PROVIDER USED RESETS SOURCE AS OF`,
                    with a `*` on the account serving turns and one row per
                    window (--json prints the socket's own payload)
proxenos statusline wrap a status-line script, adding that quota
proxenos record     capture exchanges as fixtures
proxenos supervisor install|uninstall|status
                    the supervisor that keeps the daemon alive; `install`
                    writes the unit for this user and hands it over,
                    `uninstall` removes it and the daemon it was supervising
                    stops with it, `status` says whether it is installed and
                    what the supervisor makes of it. Named for what supervises
                    rather than for launchd, because a verb named after an
                    implementation cannot grow a second one
```

Every verb except `run`, `start`, `record`, `supervisor`, `doctor`, and the two
`accounts` verbs that add an account operates through the control socket (§3)
against a running daemon. Those bring a daemon up, run one of their own, touch
the machine, or need credentials rather than a socket.

**One sub-verb per thing an operator does, each naming its account
positionally.** The surface before this used flags as actions — `--use`,
`--forget` and `--rename` on `accounts`, and a top-level `login` whose two
unrelated halves were told apart by `--key` and `--profile` — so what a command
did was decided by which flags were present, and the account it did it to was
spelled `--as` in one verb and `--use` in another. The top-level `login` verb
is gone with no alias, and there is one word for the account: `NAME`.

**Neither verb that adds an account obtains a subscription grant of this
daemon's own**; there is no authorization flow here, no callback port, and no
`--setup-token`.

`accounts add-key` stores an API key, and only that: a key belongs to nobody
and has to be kept somewhere.

`accounts login` signs in to a profile the daemon will then borrow from
(`proxy-behavior.md` §8.4). It runs that program's own login — `claude auth
login` or `codex login` — against a directory, with the same environment
variable the daemon later resolves the grant from, so what was signed in and
what is read cannot drift apart. Nothing here sees a token. Afterwards the
profile is read, and only a directory that holds a grant is written into
`[profiles]`.

A directory that is **already** signed in is adopted rather than signed in
again: no client is run, and the entry is written. That is how a profile
another tool made is taken on, and how a second run finishes the job after the
operator ran the printed line themselves.

`--relogin` is the case that adoption cannot serve: a declared profile whose
grant has lapsed. An expired grant still reads as one, so without the flag the
name is refused as already declared and with it the profile would be adopted
and nothing would change. Given it, the name has to be declared already, the
provider has to be the one it is declared as, the directory is the one
`[profiles]` names — so `--path` is refused, and a declaration naming no path
is the stock profile and is signed in with no variable set — and the client is
run whatever the profile currently reads as. Nothing is written afterwards:
the entry is already there, and the file is not opened at all.

Where there is no terminal to answer a login's prompts, the command is printed
instead of run — with the environment variable already on it — along with the
line that declares the profile afterwards. A client that wants a browser and a
keyboard, started from something with neither, hangs with nothing said.

`--device-auth` is the other half of that machine: a terminal there may be, but
no browser to open. It puts `--device-auth` on the `codex login` that is run or
printed, so the client prints a URL and a code to carry elsewhere, and it is on
the printed way back too — a re-run that dropped it would start the client the
way that hangs. `--provider anthropic` refuses it rather than passing it on,
because `claude auth login` has no equivalent and would end in its own usage
error about a spelling rather than about the choice.

A declared profile reaches a running daemon at once: the verb calls
`config.reload` (§3) after it writes, and says whether the daemon took it.
Best effort — no socket is no daemon, which is an ordinary state for a login
and not a failure of one.

Storing a key never moves which account pays. A lone account serves turns
without anything recorded, so the choice is written down before a second
account exists; every account stored after that leaves the selection alone, and
`accounts use NAME` is the verb that moves it. Storing a credential and
choosing what serves turns are two decisions, and one command making both moved
every turn onto a newly stored account without saying so.

The key arrives on **stdin**, never in a command line: an argument is visible to
every process on the machine and lands in shell history. Where stdin is a
terminal it says on **stderr** what it is waiting for and reads from a hidden
prompt; where stdin is a pipe it says nothing, so
`printf '%s' "$KEY" | proxenos accounts add-key NAME --provider P` writes to
stdout only the line
naming what it stored. One thing more is said, on stderr and only at a terminal:
an `anthropic` key beginning `sk-ant-oat` gets a note that the stem belongs to
two credentials — the year-long token `claude setup-token` mints and the
harness's own hours-long OAuth access token — that nothing stored can tell them
apart, and that the second will simply stop authenticating
(`proxy-behavior.md` §8.2). The key is stored either way; the note names the
stem and no part of the secret.

`NAME` is positional and required, because a key carries no id to be named by.
`--provider` is required too and has no default: it states which provider's
endpoints the key is spent against — `anthropic` for a key that serves turns
through the relay (`proxy-behavior.md` §9) — and the two providers refuse each
other's credentials, so a key that silently claimed the wrong one fails later
as an authentication error naming the credential rather than the choice.
Storing a key under a name that already holds a key of the *same* provider
rotates it in place, silently, which is what a replaced secret needs. A name
that holds a key of a *different* provider is refused instead, naming the
account, the provider it currently holds, and the `accounts remove NAME` that
clears the way. A name that is already a declared profile is refused too: one
name, one account.

`accounts` lists what this daemon can serve — the declared profiles first, then
the keys — as a table under a header, marking the one serving turns, naming the
store each borrowed row was read from, and saying in its last column whether
the account needs anything doing to it: a credential the backend refused, a
profile that has become a different account since it was chosen, a login about
to expire, else `ok`. Only the source column is ever cut. A grant left in
`credentials.json` by an older version is **not** listed as an account, because
nothing reads one any more; it is named in a note under the listing instead,
since a credential that quietly stopped counting reads as one that vanished.
Each row also says what kind of thing it is and whether the operator wrote it
down — `declared` is true only for a profile named in `[profiles]`, and it is
what separates the account `accounts remove` can drop by deleting a line from
the one where there is no line to delete.
`--json` prints the socket's payload instead of the table, under either
spelling of the listing.

`accounts use NAME` switches to another, and its confirmation says how far the
switch moved: a switch within one provider changes whose quota is spent and
reads `still on codex`, while one across providers changes which backend
answers, which path the turn takes and which subscription is drawn down, and
names both sides — `codex to anthropic`.

`accounts rename OLD NEW` works on a key; it is **refused for a borrowed
profile**, naming it, because a profile's name is the key it is declared under
and changing it is an edit to a file the operator can see.

`accounts remove NAME` works on both, and each kind loses a different thing. A
key is this daemon's own and is dropped from its store. A **declared** profile
is a line in `[profiles]` naming a directory another program owns: the line
goes, the grant stays exactly where it is, and the daemon re-reads the file so
it stops answering for an account it has just reported gone. A profile this
daemon *found* rather than one it was given is refused instead, saying both
that it was found and that `[profiles]` is empty — there is no line to delete,
and writing the set down is what makes it something an entry can be taken out
of. All of these go through the socket, because the daemon holds the selection:
a CLI that edited the file directly would leave a running daemon serving the
account it read at startup.

**The rendered `models` is a table under `MODEL  WINDOW  TIER`.** The `TIER`
column names the tiers pointing at each id, in the ladder's own order — `opus,
sonnet, haiku` — and a pinned tier (§7.1) points at its model like any other.
The mapping is read from the `tiers` method rather than carried a second time
on this one's payload, and where it cannot be read the column is left off
rather than printed empty: a blank cell there reads as "no tier maps to this
model", which is a different statement from "this side does not know". A
catalog that states no window still says `window unknown`.

**The rendered `status` names the account by the name `accounts` lists it
under.** The `auth` line leads with it — `auth       work-codex
(husni@sayurbox.com, codex)` — because that string is what every account verb
takes, and the word it led with before, `connected`, was the one thing already
established by there being a line at all. The address, the kind and the
provider follow it, and a daemon whose payload carries no name renders exactly
what it did before rather than inventing one.

**A `daemon` line names what is serving the socket**: the build, the process,
and — where the daemon can tell — whether the supervisor of §2.6 is what
started it. `stop`, `supervisor` and `start` all talk about that
process, and nothing in the report named it. The supervision clause is silence
rather than `not supervised` where the answer is not established, since a
platform with no supervisor here has no standing to make that claim.

**The four tier rows are a table under `TIER  MODEL`**, with a `STATE` column
where a row has one — `inert while relaying`, or `as <account>` for a tier
pinned to another account. The state used to trail off the end of the model as
a parenthesis, and the rows had no header at all. The column appears only where
some row has a state: a header over four blank cells is one the reader learns
nothing from.

**The rendered `status` says what the next turn does, not only what is
configured.** Where the account serving turns is on the second provider, every
model id it authenticates relays verbatim and the tier mapping decides nothing
(`proxy-behavior.md` §9.1). The four tier rows printed unqualified read as
"your turns go to these models", which is the one thing they do not mean in
that state, so each unpinned row is marked inert and a `routing` line names the
provider the ids relay to. A pinned tier (§7.1) names its own account and stays
live either way, marked or not by the provider the selection happens to be on —
a split mapping renders accurately row by row. This is a rendering rule: the
`status` payload already carries `auth.provider` and the pins, and no field
changed.

A live run **resolves its credential before it probes anything**, and answers
with that refusal alone when it cannot. A matrix reporting seven capabilities
as broken because there is no credential — under a header saying the backend
answered and was billed, when nothing was sent — is the same failure the probes
exist to prevent, printed the other way round. It probes the endpoint the
account's kind belongs to (`proxy-behavior.md` §8.2), so a key is answered for
rather than reported as a subscription that failed everything.

`doctor` runs the capability probes and prints a matrix. Against the fixture
corpus — the default — it contacts nothing and costs nothing. `--live` answers
the same probes from the real backend instead, one turn each, and spends real
inference quota; it maps the corpus's model ids through the configured tiers,
so what it reports is the mapping in the configuration file rather than a
notional one.

A live run applies every check except the ones that only mean something against
a recording. The corpus can assert the exact URL a search returned because the
corpus wrote it; a backend answers with whatever it answers, and failing a
working capability on that basis teaches whoever reads the matrix to discount
it. Those checks are marked in the probe table and skipped live.

The matrix always states which it was. One built from replayed fixtures that
reads like one built from a live backend is exactly the plausible-looking
output the probes exist to prevent. A probe that could not run is reported as
skipped and never counted as a pass: a probe that established nothing while
reporting success is the same failure in miniature.

A failed row prints the probe's rationale beneath it — what breaks silently
without that probe. Passing rows stay one line, because a rationale on every
row is a page of prose over a matrix nobody would then read.

Under `--live` the `count-tokens` and `env-contract` rows are marked as answered
by the proxy. The live header says the backend answered and was billed, and that
is true of every other row and false of these two: pre-flight sizing never leaves
the proxy by design, and the launch surface is rendered rather than sent. The
rows are marked rather than dropped, because a list whose job is to be complete
cannot quietly omit a surface it cannot vouch for.

The launch surface has a probe of its own, `env-contract`, and it replays
nothing: it renders the environment of §2.2 for two representative mappings and
holds it to its contract. `ENABLE_TOOL_SEARCH` must be there on every launch, and
`CLAUDE_CODE_DISABLE_1M_CONTEXT` must be there where a tier translates and absent
where every tier is relayed (`proxy-behavior.md` §7.2). Both variables were
settled against a live client and both fail silently: without the first the
client disables deferred tool loading on a base URL it does not recognize as
first-party, and without the second it appends `[1m]` to an unrecognized model id
and assumes a window four times the model's. Either regression presents as a
broken-looking client over a fully green matrix, which is why the assertion is on
the rendered environment rather than on the configuration behind it. It costs
nothing on either mode and is not skipped under `--live`.

One line under the matrix names what the run exercised and what it did not: the
account whose credential was spent, and — always — that the WebSocket transport
was not among it. A live run is HTTP only, since a probe is one turn with no
continuation and the socket's value is entirely in the incremental path. The
same line says whether the relay path (§9) was exercised, and as which
account when it answered live. Green rows say
nothing about a path no probe drove, and a reader with no line to tell them
otherwise reads green as coverage of the whole proxy.

That line is assembled from the outcomes, so a partial run states a partial
result. `--probe` is one way to get one, and a skipped or failed row is another.
Every path is named exactly once, under the heading that is true of it. A path
with a passing row is listed under `Exercised:`, and only there is the account
it spent named. A path nothing ran on — no probe, or every probe skipped — is
listed under `Not exercised:`, alongside the WebSocket transport, which is
always in it. A path whose probes all ran and all failed belongs under neither:
it was reached and established nothing, and gets a clause of its own saying so.
A heading with nothing under it is not printed, so a run that exercised nothing
prints no `Exercised:` at all.

The relay path has a probe of its own, and it runs on both modes. Replayed, it
drives the §9 branch against a recording whose marker sits inside a field the
proxy does not model, so a body round-tripped through the proxy's own types
fails it, and a stand-in backend records the bytes it was sent so both halves
are checked. Live, it sends a turn of its own to the second provider's real
endpoint.

The live arm establishes the answer half only, and the row says so. Forwarding
is the whole behaviour of this path, so the outbound bytes leave on a socket
this process cannot read; the request-half checks do not run, and the replay arm
is what covers them. Running them over a stand-in value would report a pass for
a half nothing looked at.

Which account the live arm relays as is read from the store rather than taken
from whichever account is serving turns. Exactly one account on the second
provider is used; several need `--relay-account <name>`; none skips the row
naming what the store holds. The account is pinned by name, and an authorization
by name neither reads nor changes the selection — `accounts` reports the same
serving account before and after a run. The coverage line names the relayed
account separately from the account the translating probes spent, because they
are different accounts by construction.

`--probe <name>` runs one at a time, and naming an unknown one lists the
known ones.

The corpus resolves in one of three ways, and the matrix names which. `--fixtures
<dir>` is answered from that directory and never from anywhere else — a
recording just captured by `record` must be what a run against it sees, not a
copy compiled in months earlier, so a named directory missing a fixture skips
the probe rather than falling back. With no `--fixtures`, a `fixtures/`
directory in the working directory wins if there is one, and otherwise the
corpus compiled into the binary answers. That last case is the one an installed
binary is in: `doctor` has to establish something on a first run, and a run that
skipped every probe for want of a checkout would establish nothing at the
moment it is most likely to be run.

`usage` reports every account's quota as a table, the serving account's plan
above it. It costs nothing to ask: the backend opens every stream with a snapshot, before it says
anything about the response, so the figure rides along with a turn already being
made and is never polled. Before any turn has been made it says so rather than
answering with zeroes. A figure survives a restart where its window has not
reset since (`proxy-behavior.md` §6.1), and comes back with the moment it was
taken, so the row reads `2h ago` rather than standing empty. `--json` emits the
snapshot as it stands, for a status line.

`--refresh` **asks** before reporting: it calls `usage.refresh` (§3), which
sweeps every stored account that can hold a figure, each on its own credential
and each recorded under its own name, and then reports the `usage` document as
it now stands. It spends one request per askable account, which is why it is
opted into: a bare `usage` asks for nothing and stays the cheap read. Nothing
about which account serves turns moves, a failure belongs to the row it
happened on, and an unselected account's expired grant is never refreshed. The
document `--json` emits is the same one either way — asking changes the figures
in it, never its shape.

**One row per window, and the long sentences under the table rather than on
it.** An account can hold a five-hour window beside a seven-day one, each with
its own reset, so the window rows after the first repeat neither the name nor
the freshness — those belong to the account. USED carries the percentage and
the window it is of, plus the provider's own words about that window where it
stated any. RESETS counts down to the reset the provider gave, or says the
window has already reset. SOURCE is `last turn` or `asked`. AS OF is the age of
the figure, and on a row that has none it is the reason in a cell — `no turn
yet`, `no relayed turn yet`, `per token` for a metered account, `not reported`
for a provider that states none. A metered row's USED cell carries the tally of
§6.1 instead of a percentage, because it has no ceiling to state one against.
The explanation of an empty row is one note under the table, said once however
many rows are empty, and it names `usage --refresh` as the way to fill them.

**A credit balance gets a row of its own.** Where the provider states one
(`proxy-behavior.md` §8.4), the account's windows are followed by a row whose
USED cell reads `credit: $205.75 / $210.52 · 98%`, with the provider's severity
word in parentheses where it said anything other than `normal`. It is money
rather than a percentage of an entitlement and has no reset, so the RESETS,
SOURCE and AS OF cells stay blank — those belong to the account, and are said
once on its first row. The amounts are the minor units the provider stated
divided by the exponent it stated, and the percentage is the provider's own,
never recomputed from them. A currency this proxy has no symbol for is named by
its code rather than dressed as dollars.

**A subscription that is not active gets a row too.** Where the provider states
a subscription status other than `active` (`proxy-behavior.md` §8.4), the
account's windows are followed by a row whose USED cell reads `subscription `
and the provider's own word — `subscription canceled`. It is the one state the
figures cannot show: quota keeps reading as untouched while every turn is
refused. An active subscription, and an account whose provider states nothing,
say nothing.

**A figure per account, not per daemon.** A pinned tier's turns spend the
account it names (`proxy-behavior.md` §7.1), so a daemon can hold two live
figures at once. Each is held under the account that earned it and reported
beside the serving one, with how it was come by — riding a turn, or asked for
over the socket — and the moment it was taken. An account with no figure says
so, and says why: no turn as it recorded by this daemon yet, a key holding no
subscription entitlement, or a provider that does not report a quota to this
proxy at all. That first reason is scoped to the daemon on purpose
(`proxy-behavior.md` §6.1): a turn relayed by a CLI process — `doctor --live`
makes one — reads the same quota headers and exits with them, so the account
has spent something the daemon never saw, and a line claiming none had been
spent would be false.

**A second-provider account earns its figure the same way**, from the
`anthropic-ratelimit-unified-*` headers on the response to a relayed turn
(`proxy-behavior.md` §9.4) — the only place that provider states a quota for a
subscription credential. It is reported as having ridden a turn, because it did.
No plan name appears beside it: no header states one, and one is not deducible
from headroom.
Where there is one account, the block above is the whole answer and nothing is
repeated under its own name.

The same snapshot is also put on the response as `anthropic-ratelimit-unified-*`
headers, which are the names this client's own code parses a quota from.
**Measured: that is not enough to make it appear in the status line.** A stub
endpoint setting those headers, with nothing else changed, left `rate_limits`
absent from the status-line payload.

The reason is now known rather than inferred. The client does parse those header
names, but the status-line payload is gated on a separate flag, which its own
schema describes as false "when plan rate limits do not apply (API key, Bedrock,
Vertex, or missing profile scope)". Pointing the client at a proxy means setting
`ANTHROPIC_AUTH_TOKEN`, which is the API-key path by definition, so `rate_limits`
is null there no matter what any header says. §2.1 is the only route, and the
headers are emitted because they are the accurate wire form of a figure the
response really carries — they do still feed the client's retry banner on a
quota 429.

### 2.1 `statusline`

The status line is a script the user supplies, and the client hands it a JSON
payload on stdin. `statusline` wraps that script: it reads the payload, merges
in the quota, and passes it on. A script written against the client's own shape
keeps working unchanged and gains a figure it could not otherwise have.

```json
{ "statusLine": { "type": "command",
                  "command": "proxenos statusline -- ~/.claude/my-statusline.sh" } }
```

The merged payload gains `rate_limits.five_hour` and `rate_limits.seven_day`
where a window genuinely is one of those, in the fields a script already reads —
plus `rate_limits.windows`, which carries every window the backend reported with
its real length. A script wanting a window the client has no name for reads that.

Omit the command to print the merged payload instead, for a script that would
rather pipe it. The wrapped command's exit status becomes this command's.

**It never breaks the status line.** A daemon that is not running, a socket that
does not answer, a payload that will not parse: each passes through unchanged. A
status line renders constantly, and one that breaks is worse than one missing a
figure.

**And it never merges another session's quota.** A status line is configured
once and renders for every session the client runs, including sessions pointed
at their own provider rather than at this proxy — and the daemon answers `usage`
whenever it is up. So the merge is conditional on the model: `usage` reports
the ids this daemon serves, and a payload naming something else is passed
through untouched. That is what makes the wrapper safe to leave configured
permanently while switching back and forth.

The ids are the configured tiers plus every id a turn has actually been made
against, because a client that names a model itself passes that id straight
through and no tier would recognize it. **An unanswerable question merges**: a
snapshot that names no models, or a payload that names none, leaves nothing to
compare, and withholding the figure there would take it from every session that
has it today to prevent a case that may not be happening.

Where headers do apply, only a window that genuinely matches one gets one. Those
headers name two fixed windows, five hours and seven days, and the backend's
windows are not fixed: it has reported a five-hour window in the past, does not
currently, and may again. Windows are matched to header slots by duration, and
one matching neither is reported by `usage` — where it can state its real length
— rather than announced as a window it is not.

`record` has two modes, and the distinction matters because only one of them
costs anything:

- **ingress** captures what Claude Code sends to the proxy. It needs a working
  client and no upstream credentials at all, since the exchange is recorded
  before translation. The capture carries the request headers as they arrived —
  they are half of any question about what a client actually sends — with
  credential-bearing values (`authorization`, `x-api-key`, `cookie`,
  `proxy-authorization`) redacted by name: the header's presence is the datum,
  its value is a secret in a file that is not the credential store. A turn that
  is relayed rather than translated (`proxy-behavior.md` §9) is captured too,
  and its request is held as the exact bytes that were relayed — that path
  forwards the body verbatim, and a capture re-encoded through this proxy's own
  types would silently drop every field they do not model.
- **upstream** captures the whole exchange: the client's request, untranslated,
  paired with the stream the backend answered it with. It needs credentials and
  spends quota, because the turn it records is a real one. Every turn through a
  daemon started this way is captured, not only the failing ones — a fixture is
  made from an exchange that worked.

- **surface** captures the second provider's Messages endpoint itself: a short
  fixed list of exchanges — a plain generation, a streaming text turn, a
  streaming tool call, a refusal, and a sizing call — made against the real
  endpoint and written as conformance fixtures under `fixtures/surface/`. It
  makes the calls rather than waiting for a client to make them, because what
  is wanted is a handful of known shapes rather than whatever a session happens
  to send, and it needs no daemon at all. It goes out through the same relay
  code a §9 turn takes, so what is captured is what the shipping path would
  receive. `--account` is required and must name an account on the second
  provider: spending the wrong subscription is not recoverable, and the
  selected account is usually the other one. `--only <name>` captures one
  exchange, because a capture on disk is quota already spent. Response headers
  are scrubbed by name before anything is written — `authorization`,
  `x-api-key`, `cookie`, `proxy-authorization`, `set-cookie`, and the
  organization and workspace ids, the last two because a fixture is committed
  and they say whose account paid for it.

Both halves are needed to replay one. The request cannot be inferred from the
stream, which is why the capture holds the client's request rather than the
translated one: a capture of the translated request could not be replayed
through the translation it had already been through.

Ingress and upstream write to the same fixture format, so a test replays either
without knowing which mode produced it. Surface captures are a format of their
own: they hold a status, a scrubbed header set, and either a body or a list of
SSE payloads, because what they record is an endpoint's answer rather than an
exchange to be replayed through translation.

Either mode runs a daemon, so both take the daemon's port control: `--port`, or
`PROXENOS_PORT`, overriding the configured value — the same pair `run`
documents.

Captures are written beside the configuration, `0600`, and the most recent
twenty are kept. They hold conversation content — the system prompt, the
messages, and whatever the tools read.

Logging is controlled by `RUST_LOG`. Credentials never appear at any level.

### 2.2 `env` and `settings`

The configuration Claude Code needs, in two renderings. Neither is a degraded
version of the other; they carry different amounts because the client has two
configuration surfaces and only one of them is the environment.

`env` emits shell exports, for a shell:

```
ANTHROPIC_BASE_URL=http://127.0.0.1:8787
ANTHROPIC_AUTH_TOKEN=unused
ANTHROPIC_DEFAULT_OPUS_MODEL=<mapped>
ANTHROPIC_DEFAULT_SONNET_MODEL=<mapped>
ANTHROPIC_DEFAULT_HAIKU_MODEL=<mapped>
ANTHROPIC_DEFAULT_FABLE_MODEL=<mapped>
CLAUDE_CODE_MAX_CONTEXT_TOKENS=<effective window>
CLAUDE_CODE_AUTO_COMPACT_WINDOW=<effective window>
CLAUDE_CODE_DISABLE_1M_CONTEXT=1
```

A tier the serving account relays (`proxy-behavior.md` §9.1) has no
`ANTHROPIC_DEFAULT_<TIER>_MODEL` line unless its model was stated for that
account — pinned in `[tiers]`, or named under `[accounts.<name>.tiers]`. The
shared table's id is the first provider's, and the client's own id for the
tier is the one the second provider accepts (`proxy-behavior.md` §7.2).

**Whose environment this is.** The socket method takes an optional
`{"account": name}`, and `exec --account` (§2.3) passes it: the flag decides
who serves every turn of the session it starts, so the mapping, the window,
and the client policy are all resolved for that account rather than for the
selection. Without it the answer is the selection's, unchanged. A name the
store does not hold is refused by name rather than answered about somebody
else. The mapping in force is the selection's, so an account the call names
instead is resolved from `config.toml` the way `accounts.select` would resolve
it — the shared table with `[accounts.<name>.tiers]` over it — and a
`tiers.set` that was never persisted is not carried across to it.

The two window variables appear only when the catalog knows the window, and
carry the smallest across the mapped tiers. `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
carries one further condition: it is emitted only where that figure falls
within 100,000–1,000,000 tokens, the range the client will accept. Outside it
the client's own parser answers `Expected 'auto' or 100k–1M tokens` and the
settings key of the same meaning discards the value silently, so a figure out
of range is no setting at all; the variable is left out and the reason is
logged instead (`proxy-behavior.md` §7.2). The client will warn that its
200,000 limit is not enforced; that is expected, because the real window is
larger and using it is the point.

**A mapping with any tier on the second provider states no window at all**, and
one served entirely by that provider omits `CLAUDE_CODE_DISABLE_1M_CONTEXT` too
— the client recognizes those ids by itself, and both variables would replace
what it knows with a figure this catalog cannot supply (`proxy-behavior.md`
§7.2). The tier variables are unchanged: they still carry the final ids.

**Every launch adds `ENABLE_TOOL_SEARCH=true`.** The client disables deferred
tool loading the moment its base URL is not a first-party host — it cannot
know what stands behind the proxy — and that variable is the client's own
documented override. Both paths carry the contract it needs: the relay
forwards `defer_loading` and `tool_reference` verbatim to a backend that runs
the search itself, and the translating path carries client-driven discovery
(`proxy-behavior.md` §2.5). Measured on both, live: an MCP set costing ~101k
tokens loaded up front defers to zero and the turns succeed.

All four tier variables are always emitted. `WebFetch` runs on the haiku tier, so
an unmapped haiku breaks it in a way that looks unrelated to tier mapping.

`CLAUDE_CODE_DISABLE_1M_CONTEXT` is not inert: without it this client appends
`[1m]` to an unrecognized id and assumes a million tokens, and it also strips
`context-1m-2025-08-07` from the beta list the client sends — see
`proxy-behavior.md` §7.2.

**Shell exports carry routing, plus the connector switch.** When
`client.disable_connectors` is on, the exports include
`ENABLE_CLAUDEAI_MCP_SERVERS=false` — the client's documented opt-out for the
claude.ai-hosted servers, and the one piece of client policy (§7.3 of
`proxy-behavior.md`) that has an environment variable. The rest — the denied
skill, the connector notice — lives in the client's settings file and has no
environment variable of any kind, so this rendering cannot deliver it. It says
so in a comment, which `eval` steps over, and the comment appears only when
there is a policy being left out.

`settings` emits one complete client settings document, and is the only name for
it. `env --json` printed it too and is **gone, with no alias**: `--json` means
one thing on every verb that takes it — the control socket's payload for that
verb, unrendered, which is what it already meant on `accounts` and `usage` and
now means on `status` and `models` as well. On `env` it meant a different verb's
document, so the one flag an operator could read off the surface was the one
they had to learn twice. `env` renders shell exports and nothing else.

```json
{
  "env": { "ANTHROPIC_BASE_URL": "http://127.0.0.1:8787", "...": "..." },
  "permissions": { "deny": ["Skill(claude-api)"] },
  "disableClaudeAiConnectors": true,
  "remoteControlAtStartup": false,
  "attribution": { "commit": "" }
}
```

**This document is complete on its own.** Measured: a client started with no
`ANTHROPIC_*` in its environment, reading only a settings file holding this
document's `env` block, still reached the proxy. It needs no `eval`.

The `permissions`, `disableClaudeAiConnectors`, `remoteControlAtStartup`, and
`attribution` keys are absent from the *document* when nothing is configured, rather than present and empty. An empty
deny list merged over a real one is how a rule disappears.

**The payload behind it is the other way round.** The `env` method's `settings`
field is always present, an empty object when there is no policy, because
absence there is reserved for one thing only: a daemon that predates client
policy. One file is both the daemon and the CLI, and replacing it on disk does
not restart what is already running, so a newer CLI against an older daemon is
what an ordinary upgrade leaves behind. If "no policy" and "cannot answer" looked
alike, nothing could tell the operator which one they had.

`settings` and `exec` **refuse** against such a daemon rather than producing a
document that looks complete and lacks a permission rule. `env` continues,
because routing is all it ever carried and an older daemon has all of it — with
a comment naming the daemon it is talking to. `status` (§3) names the version
actually running.

**Redirecting this into a settings file overwrites that file.** `>` truncates;
it does not merge. `.claude/settings.local.json` in particular is where the
client itself records the permissions a user has accepted, so an existing file
with real content in it is the common case, not the corner case. Merge, or write
somewhere nothing else owns. Deep-merging with `jq -s '.[0] * .[1]'` is the
obvious one-liner and is wrong: it recurses into objects but takes arrays from
the right-hand side, so the existing `permissions.deny` is replaced rather than
extended.

The proxy publishes this document and never installs it. Applying it is the job
of whoever starts the client.

### 2.3 `exec`

Runs a command with the configuration of §2.2 applied, so starting a client is
one step rather than two.

```
proxenos exec claude --resume abc
proxenos exec -- claude --help
```

The environment half is set on the child. The policy half rides on the client's
own settings flag, passed inline: **nothing is written to disk**, so there is no
file to go stale and none to clean up. The document holds no secret — the auth
token's value is ignored by design — so a command line is a fine place for it.

Everything from the program name onward is opaque and forwarded in order, so the
client's own flags keep working unchanged. `--` is accepted for a command whose
first argument would otherwise be read as this verb's.

**`--account <name>` serves this session as the named account, without moving
the selection.** The flag is this verb's, consumed before the program name and
never forwarded: the child's argv gains nothing, and the name travels as the
`ANTHROPIC_AUTH_TOKEN` value — otherwise ignored by design (§1) — as
`proxenos-account:<name>`. The daemon reads the tag per turn and it outranks a
tier's pinned account: relay when the named account is on the second provider,
translate as it otherwise, exactly the fork the selection would have decided.
A name the store does not hold is refused twice, each time naming it: at
launch, before anything starts, and at the turn, so an account removed
mid-session fails loudly rather than falling back to whoever is selected.

**The environment is rendered for that account too** (§2.2), and the mapping it
produced is printed on stderr beside the account. The flag decides which
provider serves the session, and a session served by one provider and handed
the other's tier ids sends them: seen live as a launch tagged onto an account
on the second provider being given `gpt-5.6-luna` from the shared table and
refused by the backend as an unrecognized model, with an explicit `--model` the
only way past it. The line names the ids the launch carries, and says where a
tier carries none — the client's own id relays, which is the one known to work
there.
`accounts use` is the standing switch; this is the per-session one, the way a
`kubectl` command can name a context without touching the current one. On Unix the child is
`exec`d, so signals, job control, the terminal, and the exit status pass through
untouched.

**One argument is rewritten, and only where the session's own account
relays**: a plain `--model` id whose `[1m]` variant the curated list offers
(§3) is upgraded to that variant, and the rewrite is named on stderr. The list
is asked for the account the session is served as, the same one the
environment is rendered for — a menu is one account's (§7.0), so the
selection's answers whether *its* ids have a long-context variant rather than
whether this session's do. The suffix is the client's
own long-context selector, so the session starts on the million-token window
instead of silently assuming the standard one. An id already carrying the
marker, an alias the list does not name, and another program's `--model` are
forwarded as typed — and a daemon translating to the first provider rewrites
nothing, because there the marker makes the client assume a window it does not
have.

**It refuses, before starting anything, in three cases.**

When the daemon is not answering: launching anyway hands the operator a
connection refused from a client that cannot explain it.

When the daemon predates client policy (§2.2): the session would start with a
permission rule missing and nothing about it would ever say so.

When the forwarded arguments already carry `--settings`. Measured: given two
settings flags on one argument list, the client keeps the last, drops the first,
exits 0, and writes nothing to stderr. So leading with this proxy's document
loses the policy and trailing loses the caller's, both without a word. The
refusal names the collision and the way out; `proxenos settings` prints
this proxy's half to merge. A program that does not read the flag is never given
one, so its own `--settings` is not a collision — and because that launch drops
a rule the operator configured, it is named on stderr rather than left silent:
the launch carries the environment only.

**The policy half does not reach a grandchild.** A session started this way
inherits the environment into anything it spawns, but not the argument list, so
a client started from inside it carries the routing and not the policy. Anything
that spawns a client composes its own `--settings`.

### 2.4 `stop`

Asks the running daemon to stop, then reports what it observed afterwards.

```
$ proxenos stop
stopped 0.2.0; launchd started it again as 0.3.0
```

The observation is the useful half. Under a supervisor a stop is how a running
daemon is replaced by the build on disk, which is the answer to "the binary is
new and nothing changed" — one file is both the daemon and the CLI, and
replacing it does not restart what is already running (§2.2). Whether anything
restarts it belongs to the supervisor, so this reports what it saw rather than
claiming to have done it.

**The supervisor is named where the daemon said it had one.** `supervised` on
the `status` payload (§3) is read from the daemon that is about to go — the
only process that can answer it, since what comes back is a different one and
often not answering yet — and where it said `true` the sentence names
`launchd` rather than `something`. Under a supervisor that replacement is the
mechanism the operator installed, and calling it "something" describes it as a
coincidence. Where supervision was not established — a platform with no
supervisor here, or a process launchd started under some other label — the
wording is unchanged, because naming a supervisor nothing checked for would be
a claim rather than an observation.

The build is named unless the string is identical, and with a build id on it
(§3) identical means the same build rather than merely the same version
number.

**It watches the `instance`, not the silence.** A socket falling quiet is a
statement about timing rather than about the daemon: a supervisor quick enough
leaves no gap to observe, and one that throttles a respawn leaves a gap longer
than any sensible wait. `status` therefore carries an id minted when the process
started, and a different id is a different process however the two overlapped.

The windows are three seconds for the daemon to go and twelve for anything to
bring it back, and it returns as soon as it sees the answer. Twelve because
launchd holds a respawn for ten seconds after the last start, and a shorter
window would report "nothing started it again" moments before something did,
sending the reader to `run` straight into the port the supervisor is about to
take.

**The answer arrives before the process goes.** A caller reading a closed
connection with no reply cannot tell a clean stop from a crash, and learning what
happened is the reason to ask over the socket rather than send a signal. The run
loop is released only once the response has been written.

**An in-flight turn is cut.** Someone typing `stop` means it, and a dropped
connection is something the client's own retry already handles.

**It cannot stop a daemon older than itself.** The verb exists to replace a
running daemon with the build on disk, and a daemon that predates the verb has
no method to ask — so the first upgrade past this version still has to be ended
by whatever supervises it. Nothing here can fix that; what it does is say which
situation it is rather than surface `unknown method` and leave the reader to
work out that a protocol error is really an upgrade problem.

### 2.5 `start`

Starts the daemon in the background and returns once it answers.

```
$ proxenos start
daemon running (pid 4711), logging to ~/.config/proxenos/daemon.log
stop it with `proxenos stop`
```

A verb of its own rather than a flag on `run`. Backgrounding is what an
operator asks for and holding the terminal is what a supervisor asks for, and
while both lived under one name `stop` (§2.4) was the counterpart of neither.
`run` still starts the daemon in the foreground and takes the same `--port`;
the flag that used to spell this is **gone, with no alias**.

The child is a plain `run` of the same binary in its own process group, with
stdout and stderr appended to `daemon.log` in the configuration directory —
a backgrounded process's terminal is gone the moment the command returns, so
its output needs somewhere durable to go. `stop` (§2.4) is the counterpart.

**Success is observed, not assumed.** The command exits 0 only once the daemon
answers the control socket. A child that dies first — a held port, a broken
configuration — is reported with the tail of what it wrote this start quoted,
and the command exits nonzero. Ten seconds without either is reported the same
way, and the child is ended rather than left to finish coming up after the
command has already called it a failure.

**A daemon already answering is named, not replaced.** The control socket is
one per socket path, and a second daemon would take over the socket file of the
first, leaving the CLI answering for one daemon while another holds the port.
So nothing is started, and the line says what is there —
`already running: 0.12.0+ab12cd3 (pid 4711), supervised` — from the `pid` and
`supervised` of the `status` payload (§3). It exits **0**: the state the verb
was asked to produce is the state that holds, and a failure would be a report
of something being wrong when nothing is. Each half is said only where the
daemon reports it: a build predating `pid` gets no invented number, and
`supervised` unanswered is silence rather than "not supervised", which is a
claim (§2.6).

### 2.6 `supervisor`

Installs, removes, and reports the thing that brings the daemon back when it
dies.

```
$ proxenos supervisor install
supervising proxenos.daemon, from ~/Library/LaunchAgents/proxenos.daemon.plist
  runs /Users/someone/.local/bin/proxenos run
  logs to ~/.config/proxenos/daemon.log
  control socket /var/folders/j2/…/T/proxenos.sock
stop it for good with `proxenos supervisor uninstall`
```

`install` writes a per-user LaunchAgent and hands it to launchd; `uninstall`
removes both, stopping the daemon with it; `status` says whether it is installed
and what the supervisor makes of it. The verb and its three actions are
semver-bound like the rest of §6.

**macOS is the only platform implemented, and every other one refuses by name.**
The refusal says what supervising that platform would take — a systemd user
unit with `Restart=always` — and names `proxenos start` as the way to start the
daemon meanwhile. Nothing writes a file it cannot hand to a supervisor: a unit
that is installed but never runs reports success and supervises nothing, which
is worse than having no verb at all.

**The job runs `run` in the foreground, and logs where the daemon already
logs.** Not `start`: a process that forks away leaves launchd supervising
something that has already exited, and its respawn then fights the daemon it
cannot see. `KeepAlive` is what brings it back.

**It carries no credential.** A plist in the user's home is a world-readable
file, and the store is what holds credentials. The job's environment is a closed
set of two — `TMPDIR`, and `PROXENOS_HOME` when the installing shell names one —
so adding to it is a deliberate edit rather than a filter that widened.

**Those two are carried for one reason: the socket path.** It is derived from
`PROXENOS_HOME` when set and from `TMPDIR` otherwise (§3), and a process launchd
starts does not necessarily see the `TMPDIR` a login shell does. If the two
disagree the daemon comes up healthy on its port while every CLI verb in the
operator's terminal reports connection refused, because it is dialing a
different path. Naming both in the unit makes the daemon's bind and the CLI's
dial the same derivation over the same inputs. A path too long for the
platform's socket address is refused when the unit is planned, rather than at a
bind that happens after the HTTP listener is already up.

**`TMPDIR` is carried whether or not the installing shell names one**, and that
is the subtle half. launchd does not hand a job an empty environment — it
supplies a `TMPDIR` of its own. So omitting it would not mean "no `TMPDIR`" to
the supervised daemon; it would mean launchd's, while the path planned at
install time fell back to `/tmp` and the operator's CLI went on dialing whatever
its own shell says. The unit therefore records the value the derivation actually
used, including the fallback, which is what leaves the two ends unable to drift.

**`status` compares the installed unit against the one this environment would
write, and says so when they differ.** That is the same hazard seen from the
other side: an environment that has moved since install leaves a unit whose
daemon binds one socket while the shell dials another, and the symptom reads as
a dead daemon when it is not.

**`install` says when a daemon is already answering, and does not stop it.** The
supervised job runs `run`, and `run` refuses a port another daemon holds, so
installing while a hand-started daemon is up installs a job that cannot start
yet — launchd respawns it into the same refusal until the port is free. The
install itself is real and is not undone by that, so `install` names what is
answering, by version, says the supervised job cannot take the port yet, and
names `proxenos stop` as the way to hand over. It never ends that daemon on its
own: this verb installs a supervisor, and stopping a process the operator
started by hand is not what it was asked for.

**What it reports is what will still hold the port, not what was answering when
the verb was typed.** A reinstall — the ordinary case, a new build or a moved
binary — boots out the unit it had already installed, so the daemon answering a
moment earlier is one this verb itself ends. Naming that one would tell the
operator to hand over a port already theirs, for a job that then starts fine.
The observation is therefore taken between the bootout and the bootstrap: before
it, a reinstall reports a daemon on its way out; after it, the supervised job
reports itself. A reinstall over the supervisor's own daemon prints nothing, and
so does an install with nothing answering.

**What a supervisor changes about `stop` (§2.4):** it is how a running daemon is
replaced by the build on disk. `stop` asks the daemon to go and reports what it
saw afterwards; under a supervisor what it sees is the new build answering.
Without one, nothing comes back and `stop` says that too.

---

---


## 3. Control socket

A Unix domain socket, or a named pipe on Windows, carrying JSON-RPC:

| Method | Returns | v0.1 |
|---|---|---|
| `status` | connection state, the process serving the socket (`pid`) and whether the supervisor of §2.6 started it (`supervised` — `true`, `false`, or **null** where this side cannot tell, which is every platform with no supervisor here and any process launchd started under some other label), whether the grant has been **refused** — `dead` where this side cannot spend it and `refused` carrying the backend's own words where it was sent and turned away — when the serving account's login has to be renewed (`login_expires_at`, absent where no such date exists), plan and which source reported it, the tier mapping and the effort ceiling, any mapped model the catalog withholds, whether the catalog was authoritative, the client policy in effect, and the build and `instance` serving the socket | yes |
| `accounts.select` (re-selection) | selecting the account already serving answers `{"selected", "provider", "previous_provider", "unchanged": true}` and does nothing else: no catalog fetch, no conversation ended, no figure dropped. A switch pays all three to arrive somewhere; this one is already there | no — `unchanged` added after v0.12 |
| `accounts.remove` | removes one account — the selected one, or `{"account": name}` — and answers with the name it cleared and the one serving turns afterwards; the rest stay usable, and an idle account's removal leaves the serving grant's quota alone. A key is dropped from this daemon's store; a **declared** profile loses its `[profiles]` entry and nothing else — the grant belongs to the program that owns the directory — after which the file is re-read so the daemon stops answering for it. A profile that was found rather than declared is refused, saying so and that `[profiles]` is empty | no — was `disconnect`, then `accounts.forget` |
| `accounts` | every stored account, what kind of credential each holds, whether the operator wrote it down (`declared`, true only for a profile named in `[profiles]`), and which one serves turns, plus `discovered` — whether these are the operator's own `[profiles]` entries or the stock profile of each program, read because none were declared; each borrowed row also carries the profile it was read from and, for a Claude profile, `login_expires_at` — the date the operator has to sign in again. No tokens | no — v0.3 |
| `accounts.select` | `{"account": name}`, the account every following turn is made as, the provider now serving and the one serving a moment ago (absent where nothing was) — one select moves every unpinned turn onto that provider's subscription — whether the catalog was refetched for it, and the tier mapping now in force; refuses, and moves nothing, where that account's mapping names a model its catalog does not have, naming whose menu refused and how to give that account its own mapping | no — v0.3 |
| `accounts.rename` | `{"account": from, "name": to}`, the name this daemon calls an account by, and whether an account section moved with it; the grant and the account id are untouched | no — v0.3 |
| `models` | catalog, whether it is the fallback list, and whether it was fetched for an account other than the one it was asked about. `{"account": name}` answers for that account's menu rather than the selection's — which is the curated list where that account relays (§9.1), and is what `exec --account` measures a `--model` id against; a name the store does not hold is refused by name | yes — `{"account": name}` added after v0.16.0 |
| `tiers` | tier mapping | no — was `tiers.get` |
| `usage` | the serving account's quota as of its last turn, or that no turn has been made, plus `models` — the ids this daemon serves — and `accounts`, one entry per stored account with its own figure, its freshness, and `unavailable` where it has none. Each account entry also carries `served_tokens`, the §6.1 tally, and an entry with no figure carries `reason` beside its `detail` — `no_turn`, `no_relayed_turn`, `metered`, `unknown_key_kind`, `not_reported` — the same fact in a word, so a renderer never matches on prose. Each window carries `used_percent`, `window_minutes`, `resets_at`, and — where the provider stated them — `status`, `surpassed_threshold`, `representative`, and `label` for a window no duration identifies. An entry whose provider states a credit balance also carries `credit` — `used_minor`, `limit_minor`, `exponent`, `currency`, `percent`, `severity` — money in the units the provider stated it in, present only where there is a balance to state. An entry whose provider states a subscription it no longer calls active also carries `subscription_status`, that provider's own word verbatim — absent where the subscription is active, which is silence | yes |
| `usage.refresh` | asks the backend for a figure now, **per account** — every stored account whose credential can hold one, each on its own credential and each recorded under its own name. The answer is the serving account's outcome plus `accounts`, one entry per stored account carrying either its figure or the sentence saying why it has none. Nothing about which account serves turns is read or changed | yes |
| `env` | the §2.2 block: `variables`, and `settings` always present. `{"account": name}` answers for a session served as that account rather than as the selection — the mapping, the window, and the client policy all resolved for it, which is what `exec --account` launches with; a name the store does not hold is refused by name | yes — `{"account": name}` added after v0.15.1 |
| `shutdown` | `{"stopping": true, "version": ...}`, then the process goes once the answer is written | yes |
| `record.start` / `record.stop` | fixture capture | yes — `{"mode": "ingress"}` by default, `"upstream"` must be named because it bills every turn that follows |
| `tiers.set` | tier mapping, validated against the catalog and in effect until the daemon stops; `{"account": name}` writes that account's section instead of the shared table. A tier's value takes the same two forms the file does — a model id, or `{"account": …, "model": …}` pinning the tier to another account. The pinned form needs `cross_account_tiers = true` and is refused by name without it, and its model is excluded from catalog validation: the catalog is the serving account's menu and cannot speak for the pinned one | yes |
| `effort.set` | the effort ceiling, or `null` to remove it; in effect until the daemon stops; `{"account": name}` as for `tiers.set` | yes |
| `cross_account_tiers.set` | `{"enabled": bool}` — consent for pinned tiers. **Always persisted**, unlike the setters above: consent is the operator changing what the daemon is, and a grant that evaporated at restart would leave the file refusing a mapping the operator permitted. Granting applies to the next call, not the next restart; revoking is refused by name while any tier still pins an account, because the write would produce a file the daemon refuses to start from | yes |
| `config.reload` | re-reads config.toml into the running daemon and answers `{"reloaded": [...], "needs_restart": [...]}`. It applies `[profiles]`, the tier mapping and the effort ceiling — the mapping through the same validated path a switch takes — and names what it did not: `instructions`, `client`, `transport`, `upstream`, `port`. Nothing is fetched. A file that does not parse is refused with the parse error and the daemon keeps what it was running on It also carries `serving` — who serves turns afterwards, `null` where the file took the serving profile away — and `remaining`, how many accounts are left, so that case is reported here rather than found out from a refused turn | no — added after v0.12 |
| `doctor` | probe results | no — `doctor` runs in the CLI, which is where `--live` can be given credentials without a daemon already holding them |

**Where the socket lives.** `$PROXENOS_HOME/proxenos.sock` when that variable
is set, else `$TMPDIR/proxenos.sock`. The home is what isolates a daemon from
the operator's own, and the socket is part of what has to move with it: while
the path ignored the home, a CLI isolated into a temporary home still reached
the real daemon whenever the two shared a `TMPDIR`, and every login path ends in
`accounts.select` over that socket. One derivation answers for the daemon's bind
and for every CLI call, so the pair cannot split.

**An unaddressable path is refused by name, at bind and at dial.** A unix socket
address is capped at `sun_path` — 104 bytes on macOS, 108 on Linux — and a
longer path fails the bind while the HTTP port comes up fine, leaving a daemon
that serves turns, looks healthy, and answers no verb. Both ends check first and
say the path and the cap.

**`tiers.get` is gone and `tiers` replaces it.** Every other read on this
socket is a bare noun — `status`, `models`, `accounts`, `usage`, `env` — and each
of them coexists with namespaced writers under the same noun, `accounts.select`
and `usage.refresh` among them. A lone `.get` was one name a caller had to
remember separately for no capability it bought. Renamed rather than aliased,
for the reason below.

**`disconnect` is gone and `accounts.remove` replaces it**, and the answer's
`disconnected` field is `removed`. `accounts.forget` was the first replacement
and is gone in turn: the store's own verb is `remove`, every refusal it can
answer with says "remove", and one method spelling the operation a third way
was a name an operator had to translate. The old name shipped in v0.1, when there
was one account and disconnecting from it was the whole idea; with a store of
several, forgetting one is an account operation and every other account
operation is `accounts.<verb>`. Keeping it would have left one method outside
the pattern, and adding the new name beside it would have left two methods
doing one thing for as long as the other had to stay. Renamed rather than
either, because nothing but this project's own CLI has ever called the socket
— see §6 on what that permits and when it stops.

`auth.dead` is the one that is easy to miss: a grant that cannot be spent leaves
`connected` true, because the account is still there and still readable, while
every turn after it fails with an authentication error. Without that field a
front-end shows a healthy provider and no reason to look. It is `true` when the
credential cannot be spent as it stands — unreadable, or lapsed and waiting on
the program that owns the profile (`proxy-behavior.md` §8.4).

**A persisted change is written before it is applied.** A write that fails
leaves the daemon exactly as it was, so the error the caller receives is the
whole story — applying first would leave it running a policy nobody chose,
reported as a failure, and gone at the next restart.

**`tiers.set` and `effort.set` write the configuration file only when asked** —
`{"persist": true}`. A front-end changing a mapping to try something is not the
same as an operator changing what this daemon is, and only the caller knows
which it is doing. Without it the change lasts until the daemon stops, and every
answer says which it was rather than leaving it to be discovered.

**The account tables are re-read from disk when they are needed.** They are the
one part of the configuration this daemon writes — `tiers.set` and `effort.set`
persist into them, and a rename moves them — so resolving them from the snapshot
taken at startup means a daemon that cannot see its own writes. A file that
no longer parses keeps the snapshot: the daemon is already running on it.

**Everything else is read at startup and on `config.reload`, and only some of
it can move.** `[profiles]`, the tier mapping and the effort ceiling are what a
running daemon can be handed: the profile set is swapped into the store every
turn authenticates through, and the mapping goes through the same validated
path a switch takes, so a file that would refuse a switch refuses a reload too.
The rest — `instructions`, `client`, `transport`, `upstream`, `port` — is read
once and handed to something that keeps it, and the answer names those rather
than leaving an operator to discover from a key that did nothing. A reload
fetches nothing: it is an edit taking effect, not a reason to spend a request.
A conversation in flight keeps what it started with, exactly as a `tiers.set`
mid-turn leaves it.

**A persisted change is written where the value is read from.** An account
section shadows the shared table for the tiers it names and for the ceiling it
states (§4), so a change written to the shared table while such a section exists
would be in force on this daemon and gone at the next start — written, and left
looking applied. With no `account` named, each tier goes to the serving
account's section if that section already names it and to the shared table
otherwise; the ceiling follows the same rule. `{"account": name}` writes that
account's section regardless.

A change aimed at an account that is not the one serving turns is **written and
not applied**: the mapping in force belongs to the account being served, and it
is not validated against that account's catalog either, since a list fetched for
one account makes no claim about another. Without `persist` such a call would
change nothing anywhere, and is refused rather than answered as though it had
done something. Both answers carry `account` — null for the shared table — and a
`detail` that distinguishes written-and-applied from written-only.

**`effort.set` with `null` removes an override, not every ceiling.** Under an
account it clears that account's line, and the shared ceiling applies again; the
answer and the running daemon both report the ceiling that results rather than
the `null` that was asked for, because reporting no ceiling would be a figure
that lasted until the next start.

**A rename onto a name whose section is still in the file is refused.**
Forgetting an account leaves its section behind, so a name can be free in the
store and taken in the file; moving onto it would define one table twice, which
TOML refuses, and the daemon would fail to start on a file the operator never
edited. The store is renamed first and the file second, because the store is the
half that can refuse — and a write that fails puts the name back rather than
leaving an account and its section apart.

A persisted change is a **text edit**, not a re-serialization. The file is a
document whose comments explain why each key is what it is, and most of them
exist because the obvious value is wrong in a way that does not fail loudly;
rewriting it from the parsed configuration would discard all of that, and the
loss would be invisible — the file would still parse, still work, and never again
explain itself. One value on one line changes; everything else survives byte for
byte. The file is read fresh at write time, so an edit the operator made since
startup is not overwritten to persist an unrelated one.

`tiers.set` is **partial**: naming one tier changes that tier. Treating the
argument as the whole mapping would let a caller that knows about one tier
silently unset the three it did not mention. Every set is validated against the
catalog exactly as startup validates it — that check is why this daemon owns the
mapping rather than a front-end, since it is the side holding the catalog.

**A rename takes the account's configuration with it.** An account section is
keyed by the name (§4), so a rename that left it behind would detach a mapping
from the account it was written for — and a section naming nobody is not an
error, so nothing would say so. Only the table headers change; every key and
comment under them survives byte for byte. The file is written before the store,
because the other order can leave an account with no mapping, and this one can
only leave an orphan section. An account with no section is renamed without the
file being touched at all.

**A selection re-resolves the mapping, and can be refused.** The account's own
tiers and ceiling (§4) are resolved and validated against the catalog fetched
for it, before anything else moves; a mapping naming a model that account's
catalog does not have refuses the switch and leaves the daemon serving what it
was, catalog included. The answer carries the mapping now in force, because
after a switch it is not necessarily the one that was routing turns a moment
ago. Validation is skipped where the catalog cannot speak for this account — a
fallback list, or a refetch that failed and left the previous account's list in
force — for the reason startup skips it: a fetch that did not answer is not
evidence that a model went away. There `catalog_stale` says the list is not
this account's.

**A selection moves what routes turns.** `accounts.select` writes to the store
the ingress authenticates through, so the next turn is made as the account
named rather than the one this socket merely reports. Quota does not move with
it and does not need to: every figure is held under the account that earned it
(`proxy-behavior.md` §8.3), so what `usage` reports at the top follows the
selection by itself, and each account's own figure stays valid. `accounts.remove`
drops the figure of the account it removes, serving or idle, because that
entitlement belongs to an account this daemon can no longer spend.

Live conversations are dropped with it. A conduit fixes its account on the
connection when it dials and reuses that connection for the conversation's life
(`proxy-behavior.md` §4.1), so a session left alone would go on being billed to
the account the operator has just moved off. Each dropped session pays a full
upload on its next turn, which is the direction §4.3 resolves every ambiguity
toward anyway.

`auth.dead` needs nothing of the sort: it is read from the profile every time it
is asked, so it clears by itself the moment the program that owns the grant
refreshes it.

**An account on the second provider answers from a curated list.** The fetched
catalog was never these models' menu (`proxy-behavior.md` §9.1), and the second
provider's own list endpoint names ids but states no windows — so `models`
answers from a list built into the binary, windows included, and says
`curated: true` rather than presenting it as a fetch. Which account decides
that is the one the call named, or the selection where it named none, so a
launch onto an account on the second provider is measured against that
account's menu rather than the selection's. The same answer carries
`provider`, that account's stored id, so a renderer can name whose list it is
instead of describing it by role. It is
a menu for reading, never a list to refuse by: no mapping is validated against
it, and `status` reports the catalog as curated instead of unvalidated.

**Operator-facing rows name a provider by its stored id** — `codex`,
`anthropic` — the same ids `accounts` prints. That covers the `routing` and
`catalog` lines of `status`, the curated note on `models`, and every
per-account reason in `usage`. The `auth` line names it on every connected row,
including an oauth account on the provider this proxy started with: with two
providers in the store, the row that leaves it out is the one an operator has
to guess about.

**A catalog belongs to the account it was fetched for** (`proxy-behavior.md`
§7.0). `accounts.select` and an `accounts.remove` that hands over to another account
fetch it again as whoever serves now, and their answers carry
`catalog_refreshed` — a fetch that failed keeps the previous list in force, and
everything downstream of it still describes that account. A CLI `accounts add-key` calls
`accounts.select` when it lands **and only when it selected** — storing a key
that left the selection where it was moved nothing for the catalog to follow.
Where nothing refetched — a key stored while no daemon was running, or a profile
signed into elsewhere after this daemon started — `status.catalog_stale` and
`models.stale` say the list is not this account's and `status.catalog_account`
names whose it is.

**A status line is told who is paying, before it is told anything about
quota.** The `usage` answer carries a `serving` block — name, provider, address,
plan, and account id — and `statusline` copies it into the payload whether or
not a figure is known: on a daemon that has served no turn it is the only thing
worth rendering, and a borrowed account is what makes it worth rendering at all
(`proxy-behavior.md` §8.4). It is subject to the same session check as the
figure: a session this daemon does not serve is told neither.

**`status` says which process is serving, and what supervises it.** `pid` is
this process, and `supervised` is read from the job label launchd hands a job
it starts: the label §2.6 installs is a positive answer and no label at all is
a negative one. Any other label answers **null**, because "not supervised by
`proxenos.daemon`" and "nothing supervises this" are different statements and
only the first would be established. Both fields are additive: a caller that
needs them checks for them (§6), and a daemon predating them omits `pid` and
answers no `supervised` at all.

**`status` names the account.** `auth.account` is what this daemon calls the one
serving turns and is what selects it; `auth.account_id` is what the backend
calls it and is what appears on a request; `auth.kind` is `grant` or `key`,
which decides which endpoint it is spent against and what it can be asked for;
`auth.provider` is the other half of that decision — which provider's endpoint,
`codex` unless the account says otherwise, and each entry of `auth.accounts`
carries the same field. Stored entries without one are the first provider's:
every credential file written before the field existed reads unchanged, and the
CLI renderings name a provider only where it is not `codex`.
`auth.key_flavour` is present only on a key, and only where the store recorded
which meter it is on: `subscription_token` or `api_key` (`proxy-behavior.md`
§8.2). It is a classification of the credential's shape, never any part of the
secret. It is **absent** on every entry written before the field existed and on
any key whose shape matched neither, and absence is reported rather than
resolved into whichever is likelier — a `usage` row for such an account says
this daemon does not know which meter it is on, and claims nothing about
whether a figure will arrive.
`subscription_token` is the shape's answer and not the credential's: the
`sk-ant-oat` stem is worn both by a setup token and by the harness's own OAuth
access token, and no field here separates them (`proxy-behavior.md` §8.2).
`auth.connected` means there is a credential to spend, of either kind — a key
has no grant behind it, and reading only the grant reported a daemon that could
serve every turn as not connected. `auth.accounts` lists every account this
daemon can serve — present and empty rather than absent — carrying names, ids,
addresses, plans as last read, and expiries. It carries no tokens: this is the one
credential-shaped answer that leaves the process.

**A window states more than a percentage where the provider stated more.** Each
window carries its reset epoch, and — where the provider gives them — its own
status, the threshold behind that status, whether the provider named it the
representative window, and a label for a window duration cannot identify (an
overage window has no length). `usage` renders each of those beside the figure,
and marks a window whose reset has already passed: the figure is real but
describes a window that has since turned over, and nothing else in the answer
would say so. Staleness is per window — one snapshot can hold a five-hour window
that has turned over beside a seven-day one that has not — and a window with no
reset stated is never marked.

**`accounts use` says which provider now serves.** One select moves every
unpinned turn onto that account's provider and spends that provider's
subscription. The operator asked for it, but a name does not state a provider,
and only the daemon holds the answer.

**`usage.refresh` can block while the owning client runs.** Where the account
it is asked about is a borrowed profile of either provider whose grant has
lapsed, the owning client is run once before the figure is asked for, and the
answer waits for it
(`proxy-behavior.md` §8.4) — the figure the caller wants is the one after the
refresh. **The bound is one client run for the whole call**, not one per
account: a sweep over four lapsed profiles would otherwise be four minutes of a
caller that looks hung, and neither this socket nor the CLI times out. An
account the budget ran out before is still asked for its figure, without the
refresh, and its row says it was not asked and what to do about it. One run per
profile is serialised by a lock, and the one case that cannot be helped refuses
instead: a profile whose refresh token has lapsed too, where running the client
would blank what is left of the grant.

**`usage.refresh` is not the primary path and does not replace it.** The backend
volunteers a snapshot at the head of every stream; that one is free, rides a turn
already being made, and is what `usage` reports. This exists for the cases that
path cannot cover — a front-end with a figure to show on a daemon that has served
no turn yet, and an account that is held but not serving, whose headroom is the
question asked *before* switching to it. Each account's answer is recorded where
the stream path records its own, under that account's own name, so everything
reading a quota reads one value. It is recorded as asked for rather than as
volunteered: both are true figures and they go stale differently, so `usage`
states which one each account's figure is.

**Asking is per account, and so is failing.** One account's refusal, expiry, or
dead endpoint is reported on that account's own entry and stands in for no
other's. A row whose credential cannot hold a subscription figure at all — a
key, or a credential of the provider that states quota only on turns — is not
asked; it keeps the sentence it already had rather than gaining a failed
request, and no figure is invented for it. And asking never refreshes the grant
of an account the operator did not select: a refresh rotates a token family, and
a second holder of the same grant would be left holding a token retired by a
sweep it never asked for. Such a row says its grant has expired and what to do
about it. The serving account is the exception, because every turn already
refreshes it.

**The tier table is printed in ladder order.** `tiers` is an unordered object
and arrives sorted by name, which is `fable, haiku, opus, sonnet` — an order
nothing else uses. `status` prints `opus, sonnet, haiku, fable`, which is the
order `models` already lists the same four tiers in.

**`status` reports the version of the build serving the socket.** It is not
necessarily the build that asked: one file is both, and replacing it does not
restart a running daemon. The CLI says so only when the two differ, because a
line printed on every run is one nobody reads on the run that matters.

**`version` carries a build id, and is compared for equality or not at all.**
The string is the version number, a `+`, the short commit the binary was built
from — `unknown` where there was no git to ask, a tarball build being the case
— and `-dirty` where the tree had uncommitted changes: `0.12.0+ab12cd3`,
`0.12.0+ab12cd3-dirty`, `0.12.0+unknown`. Two builds of one version number are
therefore different strings, which is what makes "the binary is new and nothing
changed" answerable by `status` and `stop` (§2.4) rather than only by a release
bump. **A caller must not parse it**: §6 already says not to compare versions,
and the suffix is exactly the part a comparison would get wrong. The same
string is what `--version`, the `daemon` line of `status`, both halves of
`stop`, and the notice from `supervisor install` (§2.6) print. It is not what
goes upstream — `[upstream].client_version` is a claim about a client the
backend filters its catalog by, and a build id there would describe a different
program.

**`env` keeps its name although its payload now carries more than an
environment.** The two halves are named inside it — `variables` and `settings` —
and a caller reading only the first is untouched by the second. Renaming the
method would cost a shim in a caller that already speaks it and buy no
capability, so the honesty went into the field names and the CLI verb: `settings`
is the name for the document, and `env` stays the name for the exports.

The names are reserved whether or not v0.1 answers them: they are semver-bound
(§6), and a method that appears later must mean what its name said all along. A
reserved method reports that it is unimplemented rather than failing as though
it were unknown.

The daemon holds authoritative state and any front-end is a client of this
interface. The CLI has no privileged path of its own; a second front-end needs no
new daemon work.

---

## 4. Configuration

TOML in the platform configuration directory — `$PROXENOS_HOME`, else
`$XDG_CONFIG_HOME/proxenos`, else `~/.config/proxenos`. Credentials
are never stored here.

```toml
port = 8787

# Optional. A ceiling on reasoning effort, whatever the client asks for.
effort = "low"

# Optional. Where the Claude CLI is, for the two things this daemon runs it
# for itself. Unset, the bare name `claude` is resolved through the daemon's
# PATH, which is not the shell's — a daemon started by launchd inherits a
# minimal one and the name does not resolve there.
claude_program = "/opt/homebrew/bin/claude"

# Optional. Where the Codex CLI is, for the cheap turn this daemon runs to
# refresh a borrowed Codex grant it owns. Unset, the bare name `codex` is
# resolved through the daemon's PATH, with the same launchd caveat.
codex_program = "/opt/homebrew/bin/codex"

[tiers]
opus   = "..."
sonnet = "..."
haiku  = "..."
fable  = "..."

# Optional, one per account, keyed by the name `accounts` lists it under.
[accounts.spare]
effort = "low"

[accounts.spare.tiers]
opus = "..."

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
endpoint                 = "https://..."
websocket                = "wss://..."
catalog                  = "https://..."
usage                    = "https://chatgpt.com/backend-api/wham/usage"

[upstream.key]
endpoint = "https://..."
catalog  = "https://..."

[upstream.anthropic]
endpoint = "https://..."
usage    = "https://..."
profile  = "https://..."

# Optional. The profile directories grants are borrowed from, keyed by the
# name the account is filed under.
[profiles.work]
provider = "codex"
path     = "/Users/me/Library/Application Support/Agent Profiles/codex/p/997619b5"

[profiles.personal]
provider = "anthropic"
```

`[profiles]` says where another program keeps a grant this daemon spends
(`proxy-behavior.md` §8.4). Paths only: no credential is written into this file,
and none is read out of it.

`provider` names which program owns the profile, and therefore which endpoint
its grant is spent against. `path` is the profile directory — a `CODEX_HOME`, or
a `CLAUDE_CONFIG_DIR`.

`claude_program` is the Claude CLI this daemon runs **on its own behalf**, and
never to serve a turn: once to ask the program that owns a borrowed Anthropic
profile to refresh its own grant (`proxy-behavior.md` §8.4), and once to read
the version the quota request for that grant is made as. Unset, it is the bare
name `claude`, resolved through the daemon's `PATH` — which is not the shell's.
A daemon started by launchd inherits a minimal one and resolves nothing, so
write the path out where that is how it starts; `usage --refresh` otherwise
refuses with `could not run \`claude\``.

`codex_program` is the same thing for the Codex CLI: a cheap `codex exec` turn
this daemon runs on its own behalf to refresh a borrowed Codex grant it owns.
Unset, it is the bare name `codex`, resolved through the daemon's `PATH`, with
the same launchd caveat.

**Leaving `path` out means the stock profile**, the one that program uses when
no variable designates a directory. That is a *different* profile from one
naming the stock directory explicitly: on macOS the client files its grant under
a keychain item chosen by whether `CLAUDE_CONFIG_DIR` was set at all, so writing
the path out selects a different item (`proxy-behavior.md` §8.4). Writing it out
is not a way of saying "the default".

A path must be absolute, and a leading `~` is refused rather than expanded: the
daemon's working directory is not the operator's, and for a Claude profile the
spelling of the path is part of the identity. Two entries naming one provider
and one directory are refused naming both, because one directory holds one grant
and is therefore one account. Two entries naming one directory under *different*
providers are two profiles, which is what a directory holding both programs'
state looks like.

`[upstream.key]` is where an API key is spent, which is not where a grant is
(`proxy-behavior.md` §8.2). There is no socket in it: the WebSocket protocol
belongs to the subscription backend, so a key account uses HTTP. Sending either
credential to the other's endpoint is refused before anything leaves.

`[upstream.anthropic]` is where a relayed turn goes (`proxy-behavior.md` §9),
and `usage` is where that provider states quota for a borrowed grant
(`proxy-behavior.md` §8.4). Only a grant can ask there: a key has no
subscription behind it, and the long-lived subscription token wearing the same
stem is refused for want of a scope. `profile` is where the same provider
states the account's plan with its multiplier (`max 20x`); it is asked beside
a quota refresh, at most hourly, and an answer it declines to give leaves the
plan absent rather than invented. Otherwise the relay does one thing: it speaks the surface this proxy
already exposes, so there is no catalog to translate and no socket protocol to
speak.

`[upstream]` is entirely optional; every key defaults to what ships. It exists so
a pinned binary can be repointed rather than rebuilt, and because two of the keys
fail in ways nothing else can diagnose.

`client_version` is what the proxy reports when asking for the model list — not
this crate's version. The backend filters the list by it, and a version below
every model's minimum returns an **empty list rather than an error**, which reads
exactly like an account with no models. Startup says so by name when the catalog
comes back empty.

`usage` is where a quota figure can be asked for rather than waited for: the
backend volunteers a snapshot at the head of every stream (`proxy-behavior.md`
§6), and this is the route for a front-end that has to show a figure before any
turn has been made.

`effective_window_percent` is the share of a context window left usable once
instructions, tool overhead, and output are accounted for, applied where the
catalog states no share of its own. It is the figure the client is told, so it
decides when compaction fires: lower compacts sooner and wastes window, higher
risks a turn refused for length. A value outside `(0, 100]` is refused at
startup rather than clamped.

**Every key has a default, and the file itself is optional.** A missing
configuration is a first run, not a failure: the daemon logs where the file would
go and starts on the defaults. A file that is present but unparseable is still an
error — falling back there would run a daemon that ignores what the operator
wrote.

**An account section states what differs for one account.** A catalog is one
account's menu (`proxy-behavior.md` §7.0), so a mapping is only ever right for
the models every account has: two subscriptions on different plans are offered
different models, and a key account beside a subscription need not overlap at
all. `[accounts.<name>.tiers]` replaces the tiers it names and no others, and
`effort` under `[accounts.<name>]` replaces the shared ceiling rather than being
capped by it — an operator who writes a different one for an account means that
one. The key is the name `accounts` lists, because that is the string every
account verb takes and a key account has no id to be named by. An account with
no section takes the shared tables, which is also what a daemon with nothing
selected uses.

The four tiers default to the mapping above. An omitted tier takes its default; a
tier written blank is refused, because an omission accepts the shipped answer
while a blank is a mistake. Each mapped model is validated against the live
catalog when one is reachable. That validation happens once, at startup: the
catalog is not refetched, so a mapping cannot go stale while the daemon runs.

A tier entry is a model id, or a table pinning one to another account:
`haiku = { account = "spare", model = "..." }` serves that tier's turns as
`spare` whatever account serves the rest of the session. The table form is
gated by the top-level `cross_account_tiers = true` — it routes one client's
traffic across accounts' quotas, which is a decision the operator owns, so its
absence refuses the daemon at startup rather than falling back to the serving
account. Falling back would spend the wrong account's quota invisibly. The
bare-string form is ungated and keeps the meaning it has always had.

The pin decides which credential authenticates: every upstream request that tier
produces goes up as the pinned account, and unpinned tiers are unchanged. A pin
naming an account the store does not hold refuses the turn with
`invalid_request_error`, naming the account and listing what is stored, and
nothing reaches the backend as somebody else. A pinned account holding a
credential the endpoint does not take is refused the same way, naming the pinned
account. `proxy-behavior.md` §7.1 carries the rest, including what a refresh on
a pinned grant does.

`[client]` is policy the client applies to itself, which settings mostly carry
and environment variables mostly cannot — see `proxy-behavior.md` §7.3 for why
each default is what it is. `deny_skills` names skills refused for a session
served here; the proxy writes the `Skill(...)` rule the client understands,
because a rule built by hand and built wrong denies nothing and reports nothing.
An empty list allows everything. **Left unset, the default is resolved per
launch**: `claude-api` is denied for a launch whose turns translate, and
nothing is denied for one whose turns are all relayed — the skill documents
the second provider's API, the wrong reference for a translated session and
the right one for a relayed session. A written list is the operator's rule and
applies on either path. `status` reports the list a launch would actually
apply. `disable_connectors` does two things through
one intent: the settings key (`disableClaudeAiConnectors`) suppresses the
connector notice the client prints whenever an auth token is set, which here is
always, and the export (`ENABLE_CLAUDEAI_MCP_SERVERS=false`) is the client's
documented opt-out for the claude.ai-hosted servers themselves — the half that
still reaches a client launched from `proxenos env` alone.
`disable_remote_control` writes `remoteControlAtStartup: false`, keeping the
client from starting its remote-control session at launch: a session started
through a local proxy is a local decision. `disable_commit_attribution` writes
`attribution.commit: ""` — an empty template, which is the client's own way of
appending no trailer to a commit a launched session makes. Which model served a
turn is not a fact a commit message is the place to record, so it ships on every
launch, translate or relay.

`effort` caps reasoning effort on every request, whatever the client asks for —
one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra`
(`ultracode` is the client's name for `ultra` and is accepted as one). It is a
ceiling, not a fixed value, and is capped again by what the model accepts — and
raised by it: an effort below the lowest one the model lists is snapped up to
that floor, this ceiling included, because there is nothing cheaper the model
would take (`proxy-behavior.md` §2.7). Omit it for no ceiling; omitting it does not mean zero effort, it means the backend's
own default. An unrecognized value is refused at startup rather than ignored.

It is a top-level key and must sit above the tables — see the note on
misplacement below.

`[instructions]` is what the proxy puts around the client's system prompt
(`proxy-behavior.md` §2.1). `identity` leads with one line naming the model that
is actually answering, and is **on by default** — a model told it is a different
product is being given a false premise on every turn, which is not a neutral
default to pick on an operator's behalf. `append` is operator text placed after
the system prompt, where an instruction has to be to take precedence over it.

`working_budget` is a short block asking the model to read the smallest slice
that answers the question rather than whole files, and to act once a read is
enough. **On by default**, deliberately: the conversation is replayed upstream on
every turn and echoed back three times, so broad reading spends the window fast.
It sits after the client's prompt, which it exists to overrule on this point, and
before `append`, which an operator wrote on purpose and which therefore outranks
it.

All three must be constant for a conversation. Text that changes between turns
changes `instructions`, and that costs every delta and every cache hit.

The file is read at startup and again on `config.reload` — **nothing watches
it**, so a change takes effect when a reload asks for it or on the next `run`.
What a reload can move and what it cannot is §3's list. `--port` is the only
override outside the file.
The estimator backend is likewise not a configuration key in v0.1 — the
tokenizer is a compile-time feature (`--features tokenizer`), because which
estimator wins is a measurement rather than an operator's choice
(`proxy-behavior.md` §6.3).

**An unrecognized key is refused, not ignored.** Tolerating one looks forgiving
and is not: in TOML a bare key written after a table header belongs to that
table, so `effort` placed below `[tiers]` is `tiers.effort`. Ignored quietly,
the operator believes they capped their spending while every request runs at
the backend's default. Top-level keys therefore sit above the tables, and the
error says so when they do not.

---

## 5. Limitations

Stated because each is permanent under the current design, not because they are
pending work.

- **The context percentage Claude Code displays is wrong.** It is computed
  client-side against an assumed window and cannot be corrected from the proxy.
  Token counts are exact; the percentage is not.
- **Sessions compact earlier than necessary**, for the same reason. The assumed
  window sits below the real one, which is the safe direction.
- **`cache_creation_input_tokens` is always zero.** No upstream write event
  exists to report.
- **`count_tokens` is an estimate.** It is answered by the conversation's own
  estimator, so it is uncalibrated before that conversation's first completed
  request and improves after it. It is never exact: there is no upstream
  token-counting endpoint to be exact against.
- **`cache_control` and `thinking` blocks are dropped** on the request path.
  Reasoning is reconstructed on responses from summary events.
- **Image URLs are not prefetched** and resolve only if the backend can reach
  them.
- **The catalog fallback list is fixed** and needs updating if models are renamed
  or retired while the live fetch is unavailable. Its entries carry no context
  window, so the window guard does not fire for a model the fallback named.
- **The credential directory has to be on a filesystem that locks.** Every write
  of the credential file takes a lock beside it, and a filesystem that cannot
  take one — a network mount is the case that exists — fails the write rather
  than proceeding without it. The error names `PROXENOS_HOME`, which
  points the whole directory somewhere else.
- **A key account's catalog carries no windows or efforts.** The list is real
  and is the account's own, and the endpoint states neither for any entry. The
  window guard therefore never fires for a key account and the model half of the
  effort cap has nothing to cap against. The ceiling set in configuration still
  applies.

- **Claude Code never reaches the `input_file` path.** It rasterises PDFs into
  image blocks, so documents from that client reach the model as images. The
  `document` translation is for a client that sends one, and the backend does
  accept it — measured by posting a `document` block directly, which returned a
  code that existed only inside the PDF.

- **Compression saves bytes and no tokens.** Subscription HTTP bodies are
  zstd-compressed and WebSocket frames use `permessage-deflate`, negotiated
  during the upgrade. Roughly two thirds off in both directions, and the inbound
  half is the larger one — the backend echoes the whole request back three times
  per turn. Quota is unaffected. A key request is neither: it is never
  compressed, and there is no socket for it to compress.

- **A web search that produced no citations reports the pages the model opened**,
  which carry a URL but no title. That is worse than a real citation and better
  than an empty result, which the client reads as "nothing found".

- **What a matrix proves depends on what answered it.** A replayed run
  establishes that the proxy does its half; only `--live` establishes that the
  backend does its own. `doctor` states which on the face of its output, and
  `roadmap.md` §L records what has been settled against a live backend and what
  has not.

---

## 6. Stability

The CLI verb set, the control-socket method names, the configuration keys, and
the error-type vocabulary are semver-bound. A shipped name is never repurposed or
removed within a major version; only new ones are added.

**Before 1.0 that rule has one deliberate exception, and it closes on its own.**
Semantic versioning does not bind a zero major, and nothing outside this
project's own CLI has ever spoken the socket — the CLI and the daemon are one
binary, so a rename lands on both at once. A name that turns out wrong is
therefore renamed on a minor bump, said in the changelog, and gone rather than
left beside its replacement. `accounts.forget` arrived that way and left the
same way, renamed to `accounts.remove`; so did the project's own name: everything `codex-cc-proxy` and `CODEX_CC_PROXY_*` named is
`proxenos` and `PROXENOS_*` from v0.5.0, one rename with no aliases kept. The exception
ends when a second caller exists — the graphical front-end is the one planned,
and any other program that speaks this socket ends it just as well — and it ends
whether or not 1.0 has been reached: the moment something else has to be
upgraded in step, only additions are safe. It is a statement about callers, not
about a version number.

**The bound method set is the whole of §3's table, named here so the freeze is
a contract rather than folklore.** From the moment a second caller exists — the
exception above is a statement about callers, not about a version number —
these eighteen names are fixed: `status`, `shutdown`,
`accounts`, `accounts.select`, `accounts.rename`,
`accounts.remove`, `models`, `tiers`, `tiers.set`, `effort.set`,
`cross_account_tiers.set`, `usage`, `usage.refresh`, `env`, `doctor`,
`record.start`, `record.stop`, `config.reload`. `doctor` is bound although it
is not implemented:
a reserved name that appears later must mean what its name said all along. The
same list is a constant in the daemon, so removing or renaming one is a visible
change to the code and not only to this document.

**An unknown method reaches the caller as an unknown method.** The error code
survives the round trip rather than being flattened into one kind, because
"this daemon does not have that method" and "that method refused what you asked"
are different situations and only the first is answered by replacing the daemon.

**A field added to a response is a capability, and a caller that needs it checks
for it.** Adding one is not a breaking change: an older caller ignores what it
does not know, and must not be "fixed" into a strict check, because that would
make every upgrade have to be simultaneous. The obligation runs the other way. A
newer caller that requires a field has to establish it is there rather than infer
it from a version string — comparing versions forces a policy about which
differences matter and gets it wrong for a patched build or a forgotten bump.
Where a field's absence would otherwise be ambiguous, it is emitted empty rather
than omitted, so that absence keeps meaning "this daemon predates it" and nothing
else.

The ingress shape is not ours — it tracks the Anthropic Messages API, and
changes there are not breaking changes in this project's versioning.

---

## 7. Posture

The upstream endpoint is not a published or supported API. It may change or be
withdrawn without notice, and using a subscription this way is a decision each
operator makes for themselves. There is no version of this project that avoids
that, so it is stated rather than omitted.

This project is not affiliated with, endorsed by, or sponsored by Anthropic or
OpenAI. All trademarks belong to their owners.

No telemetry is collected or transmitted. Credentials never appear in process
arguments or logs. Configuration and credential files are created with
restrictive permissions.
