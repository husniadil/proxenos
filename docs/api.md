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
ignored.

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
This path is proven against fixtures and not yet confirmed against the second
provider's live endpoint (`proxy-behavior.md` §9, `roadmap.md` §L).

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
proxenos run        start the daemon (--detach: in the background)
proxenos login      store an API key, read from stdin (--as NAME names it,
                    --provider names which provider it is spent against)
proxenos accounts   stored accounts (--use switches, --rename, --forget drops)
                    one row per account: a `*` on the one serving turns, the
                    name, the address or `key`, and the provider — named on
                    every row, since with two providers stored an unnamed one
                    is a guess
proxenos status     connection, tier mapping, model catalog
proxenos models     available models
proxenos env        environment for Claude Code, as shell exports
proxenos settings   the same configuration, as one settings document
proxenos exec       run a command with that configuration applied
proxenos stop       ask the running daemon to stop
proxenos doctor     probe backend capabilities (--live answers from the real one)
proxenos usage      what quota is left (--refresh asks, per account)
proxenos statusline wrap a status-line script, adding that quota
proxenos record     capture exchanges as fixtures
```

Every verb except `run`, `login`, and `doctor` operates through the control
socket (§3) against a running daemon.

`login` stores **a key, and only a key**. A subscription grant is not this
daemon's to obtain: it belongs to the program whose profile holds it, and is
read from there (`proxy-behavior.md` §8.4). Running `login` without `--key` is
refused, and the refusal says to sign in over there and declare the profile
under `[profiles]` instead. There is no authorization flow here, no callback
port, and no `--setup-token`: what that flag existed to store is what a
borrowed Claude profile now supplies, with a refresh behind it rather than a
token that silently stops working.

Storing a key never moves which account pays. A lone account serves turns
without anything recorded, so the choice is written down before a second
account exists; every account stored after that leaves the selection alone, and
`accounts --use NAME` is the verb that moves it. Storing a credential and
choosing what serves turns are two decisions, and one command making both moved
every turn onto a newly stored account without saying so.

The key arrives on **stdin**, never in a command line: an argument is visible to
every process on the machine and lands in shell history. Where stdin is a
terminal it says on **stderr** what it is waiting for and reads from a hidden
prompt; where stdin is a pipe it says nothing, so
`printf '%s' "$KEY" | proxenos login --key ...` writes to stdout only the line
naming what it stored. One thing more is said, on stderr and only at a terminal:
an `anthropic` key beginning `sk-ant-oat` gets a note that the stem belongs to
two credentials — the year-long token `claude setup-token` mints and the
harness's own hours-long OAuth access token — that nothing stored can tell them
apart, and that the second will simply stop authenticating
(`proxy-behavior.md` §8.2). The key is stored either way; the note names the
stem and no part of the secret.

`--as NAME` is required, because a key carries no id to be named by.
`--provider` states which provider's endpoints it is spent against — `codex` by
default, `anthropic` for a key that serves turns through the relay
(`proxy-behavior.md` §9). Storing a key under a name that already holds a key of
the *same* provider rotates it in place, silently, which is what a replaced
secret needs. A name that holds a key of a *different* provider is refused
instead, naming the account, the provider it currently holds, and the
`accounts --forget NAME` that clears the way. A name that is already a declared
profile is refused too: one name, one account.

`accounts` lists what this daemon can serve — the declared profiles first, then
the keys — marking the one serving turns. `--use NAME` switches to another, and
its confirmation says how far the switch moved: a switch within one provider
changes whose quota is spent and reads `still on codex`, while one across
providers changes which backend answers, which path the turn takes and which
subscription is drawn down, and names both sides — `codex to anthropic`.
`--rename FROM TO` and `--forget NAME` work on a key, which is this daemon's to
name and to drop. Both are **refused for a borrowed profile**, naming it: a
profile's name is the key it is declared under, and forgetting one is deleting
an entry from a file the operator can see. All of them go through the socket,
because the daemon holds the selection: a CLI that edited the file directly
would leave a running daemon serving the account it read at startup.

## 3. Control socket

A Unix domain socket, or a named pipe on Windows, carrying JSON-RPC:

| Method | Returns | v0.1 |
|---|---|---|
| `status` | connection state, whether the grant has been **refused**, plan and which source reported it, the tier mapping and the effort ceiling, any mapped model the catalog withholds, whether the catalog was authoritative, the client policy in effect, and the build and `instance` serving the socket | yes |
| `accounts.forget` | forgets one account — the selected one, or `{"account": name}` — and answers with the name it cleared and the one serving turns afterwards; the rest stay usable, and an idle account's removal leaves the serving grant's quota alone | no — was `disconnect` |
| `accounts` | every stored account, what kind of credential each holds, and which one serves turns; no tokens | no — v0.3 |
| `accounts.select` | `{"account": name}`, the account every following turn is made as, the provider now serving and the one serving a moment ago (absent where nothing was) — one select moves every unpinned turn onto that provider's subscription — whether the catalog was refetched for it, and the tier mapping now in force; refuses, and moves nothing, where that account's mapping names a model its catalog does not have, naming whose menu refused and how to give that account its own mapping | no — v0.3 |
| `accounts.rename` | `{"account": from, "name": to}`, the name this daemon calls an account by, and whether an account section moved with it; the grant and the account id are untouched | no — v0.3 |
| `models` | catalog, whether it is the fallback list, and whether it was fetched for an account other than the one serving turns | yes |
| `tiers` | tier mapping | no — was `tiers.get` |
| `usage` | the serving account's quota as of its last turn, or that no turn has been made, plus `models` — the ids this daemon serves — and `accounts`, one entry per stored account with its own figure, its freshness, and `unavailable` where it has none. Each window carries `used_percent`, `window_minutes`, `resets_at`, and — where the provider stated them — `status`, `surpassed_threshold`, `representative`, and `label` for a window no duration identifies | yes |
| `usage.refresh` | asks the backend for a figure now, **per account** — every stored account whose credential can hold one, each on its own credential and each recorded under its own name. The answer is the serving account's outcome plus `accounts`, one entry per stored account carrying either its figure or the sentence saying why it has none. Nothing about which account serves turns is read or changed | yes |
| `env` | the §2.2 block: `variables`, and `settings` always present | yes |
| `shutdown` | `{"stopping": true, "version": ...}`, then the process goes once the answer is written | yes |
| `record.start` / `record.stop` | fixture capture | yes — `{"mode": "ingress"}` by default, `"upstream"` must be named because it bills every turn that follows |
| `tiers.set` | tier mapping, validated against the catalog and in effect until the daemon stops; `{"account": name}` writes that account's section instead of the shared table. A tier's value takes the same two forms the file does — a model id, or `{"account": …, "model": …}` pinning the tier to another account. The pinned form needs `cross_account_tiers = true` and is refused by name without it, and its model is excluded from catalog validation: the catalog is the serving account's menu and cannot speak for the pinned one | yes |
| `effort.set` | the effort ceiling, or `null` to remove it; in effect until the daemon stops; `{"account": name}` as for `tiers.set` | yes |
| `cross_account_tiers.set` | `{"enabled": bool}` — consent for pinned tiers. **Always persisted**, unlike the setters above: consent is the operator changing what the daemon is, and a grant that evaporated at restart would leave the file refusing a mapping the operator permitted. Granting applies to the next call, not the next restart; revoking is refused by name while any tier still pins an account, because the write would produce a file the daemon refuses to start from | yes |
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

**`disconnect` is gone and `accounts.forget` replaces it**, and the answer's
`disconnected` field is `forgotten`. The old name shipped in v0.1, when there
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
taken at startup means a daemon that cannot see its own writes. Everything else
is still read once at startup, because nothing else here writes it. A file that
no longer parses keeps the snapshot: the daemon is already running on it.

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
selection by itself, and each account's own figure stays valid. `accounts.forget`
drops the figure of the account it forgets, serving or idle, because that
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
provider's own list endpoint names ids but states no windows — so `models` for
a relay-serving daemon answers from a list built into the binary, windows
included, and says `curated: true` rather than presenting it as a fetch. The
same answer carries `provider`, the stored id of the account serving turns, so
a renderer can name whose list it is instead of describing it by role. It is
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
§7.0). `accounts.select` and an `accounts.forget` that hands over to another account
fetch it again as whoever serves now, and their answers carry
`catalog_refreshed` — a fetch that failed keeps the previous list in force, and
everything downstream of it still describes that account. A CLI `login` calls
`accounts.select` when it lands **and only when it selected** — storing a key
that left the selection where it was moved nothing for the catalog to follow.
Where nothing refetched — a key stored while no daemon was running, or a profile
signed into elsewhere after this daemon started — `status.catalog_stale` and
`models.stale` say the list is not this account's and `status.catalog_account`
names whose it is.

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

**`accounts --use` says which provider now serves.** One select moves every
unpinned turn onto that account's provider and spends that provider's
subscription. The operator asked for it, but a name does not state a provider,
and only the daemon holds the answer.

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

**`status` reports the version of the build serving the socket.** It is not
necessarily the build that asked: one file is both, and replacing it does not
restart a running daemon. The CLI says so only when the two differ, because a
line printed on every run is one nobody reads on the run that matters.

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

[upstream.key]
endpoint = "https://..."
catalog  = "https://..."

[upstream.anthropic]
endpoint = "https://..."

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

`[upstream.anthropic]` is where a relayed turn goes (`proxy-behavior.md` §9).
One entry, because the relay does one thing: it speaks the surface this proxy
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
ceiling, not a fixed value, and is capped again by what the model accepts. Omit
it for no ceiling; omitting it does not mean zero effort, it means the backend's
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

The file is read once, at startup: **v0.1 does not watch it**, and a change
takes effect on the next `run`. `--port` is the only override outside the file.
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
left beside its replacement. `accounts.forget` arrived that way, and so did the
project's own name: everything `codex-cc-proxy` and `CODEX_CC_PROXY_*` named is
`proxenos` and `PROXENOS_*` from v0.5.0, one rename with no aliases kept. The exception
ends when a second caller exists — the graphical front-end is the one planned,
and any other program that speaks this socket ends it just as well — and it ends
whether or not 1.0 has been reached: the moment something else has to be
upgraded in step, only additions are safe. It is a statement about callers, not
about a version number.

**The bound method set is the whole of §3's table, named here so the freeze is
a contract rather than folklore.** From v0.7.0 — the release that ships the
graphical front-end, and with it the second caller the exception above is a
statement about — these seventeen names are fixed: `status`, `shutdown`,
`accounts`, `accounts.select`, `accounts.rename`,
`accounts.forget`, `models`, `tiers`, `tiers.set`, `effort.set`,
`cross_account_tiers.set`, `usage`, `usage.refresh`, `env`, `doctor`,
`record.start`, `record.stop`. `doctor` is bound although it is not implemented:
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
