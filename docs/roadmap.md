# Roadmap

What is shipped, what is intended next, and what only a live backend could
settle. Release history is in [`CHANGELOG.md`](../CHANGELOG.md); this file does
not repeat it.

## Where this stands

The current release is **v0.32.1**. Shipped, at the level of capability:

- **Two providers behind one Messages surface.** Codex through the translate
  path (Messages ⇄ Responses), Anthropic through the relay path, which forwards
  Messages untranslated. Which one a turn takes follows the provider of the
  account serving it (`proxy-behavior.md` §9).
- **Both transports on the translate path.** WebSocket with incremental delta
  upload and `permessage-deflate`, HTTP with SSE and a zstd body as an ordinary
  operating mode rather than an error path.
- **Harness fidelity.** Image and document attachments, native web search
  reconstructed into real result blocks, `WebFetch`, tool search, tool calling,
  and a context meter fed by a calibrated estimate that the upstream count
  replaces.
- **Accounts.** Several stored accounts, borrowed subscription grants declared
  under `[profiles]`, stored keys, per-account tier mappings and effort,
  opt-in cross-account tiers, and a per-session account through
  `exec --account`.
- **An operator surface that states only what it observed.** `status`,
  `accounts`, `models`, `usage` with per-account quota and persisted spend,
  provider incidents, `reload` without a restart, and a control socket whose
  vocabulary `status` publishes.
- **One daemon, two doors.** Loopback without a token, and an optional stated
  address that requires one, so other machines can be served by this daemon.
- **Operations.** `start`/`stop`, a supervisor on launchd and systemd user
  services, `doctor` against the replay corpus or live, `record` for ingress,
  upstream, and surface captures, and release binaries with a verified install
  script. A supervised daemon installed by that script can `update` itself to a
  release, from its own machine or another.

## Everything here is verifiable offline

No completion criterion requires a live backend, credentials, or quota. The
suite runs entirely against a local replay server, because correctness that can
only be shown by spending money stops being demonstrable the moment quota runs
out. What genuinely cannot be settled offline is collected in §L and stated as
open until a live run settles it.

## Intended

### Next

Named, never numbered: twice a numbered intention shipped as something else,
and a roadmap that misnames a released version is read as a record and is wrong
as one. Intended, not committed.

**Package-manager routes.** There is no Homebrew tap and no container image;
`install.sh`, the release archives and `cargo install` are the install routes. **Done when** either installs a release binary that `doctor`
answers from.

---

## L. The live gate

Questions that needed a working backend and could not be settled offline. Each
is one of: **settled live**, **derived, not confirmed**, or **open**. Before a
question is settled, the claim it concerns in `proxy-behavior.md` is derived
from the upstream's protocol definitions and says so.

Settling these falsified rules that no offline test could catch — the empty
delta, the reasoning mismatch, the compressed WebSocket frame, the response's
expiry field, the non-streaming request answered with a stream. Each was fixed
by amending the spec in the same commit as the code.

New questions belong here as they are found.

### Borrowed grants and keys

#### Does a `codex exec` turn refresh a genuinely lapsed borrowed Codex grant?

**Open — derived, not confirmed.** The daemon runs a cheap `codex exec` against
a borrowed Codex profile whose access token has lapsed, as it runs `claude -p`
for an Anthropic one, and the upstream client refreshes proactively inside that
run (access token within five minutes of expiry, or `last_refresh` older than
eight days). Confirmed: the turn runs, exits 0, and needs no model id. Not
confirmed: that it rotates a lapsed grant — a Codex access token is a signed JWT
and cannot be backdated locally to force the case. Settle it when a borrowed
grant actually lapses: one `usage --refresh`, then read whether the profile's
tokens rotated.

#### Does a superseded refresh token stay redeemable?

**Open — previously answered yes, now doubtful.** An early measurement saw a
superseded token still redeem after rotation. A stored token was later refused
with `refresh_token_reused`, so that result most likely described a grace
window rather than a durable property. Do not rely on it, and never run a
second holder against a copy of a credential or a profile directory: whichever
copy refreshes last retires every other copy's token.

#### Does a refresh return a fresh id token?

**Settled live: yes.** A stored expiry was put in the past and one turn made,
so the daemon owning the store refreshed and persisted the result. Access,
refresh, and id token all rotated, and the new id token carries a new `iat` and
`exp`, so a grant's plan claim cannot go stale. The response also carries
`expires_in`. **Falsified:** the spec's claim that no such field existed.

#### Do two grants on one client id stay independent?

**Settled live: yes.** Two accounts on one client id, refreshed one after the
other: each refresh rotated only its own family, and the first account still
authenticated after the second refreshed. The unsafe arrangement remains two
holders of one grant.

#### Does the key endpoint behave as this proxy assumes?

**Settled live: yes, with one exception.** `doctor --live` as a key account
passed all nine probes against the real endpoint, streaming behaved as the
recordings do, and the catalog is the real one — though it states no context
window, so `models` prints `window unknown`. A catalog `401` seen once did not
reproduce, and its cause is not established. **Falsified:** that a key request
may be compressed. A zstd body is refused with `400 invalid_json`, so key
requests are sent uncompressed.

### Translate path

#### Does the backend accept the request shape?

**Settled live: yes.** Headers, `instructions`, and tools as sent; a turn
completes with a correct frame sequence. System and developer roles inside
`input` are refused — `400 System messages are not allowed` — which §2.1 rests
on. A `tool_choice` of `any`, sent as `required`, produced a call to the named
tool. An `input_file` part, which the client never sends because it rasterises
PDFs, was posted directly and read: a PDF holding a random code returned that
code.

#### What does compression look like on each transport?

**Settled live.** HTTP takes a zstd body with `Content-Encoding: zstd`. The
socket negotiates `permessage-deflate` on upgrade, with full context takeover
and no window limit. One identical turn counted on the wire: 104,566 in /
40,335 out compressed against 300,879 in / 110,608 out uncompressed — 65% off
both ways, and zero tokens either way. **Falsified:** that a binary frame
signals compression, and the earlier decision that compression was not worth
it, which had measured the request only.

#### Does WebSocket connect, or close with a policy code?

**Settled live: it connects.** No policy close has been seen, and the catalog
marks these models `prefer_websockets`. One account's experience is not
evidence about every account's, so the HTTP fallback stays tested as an
ordinary path.

#### Does incremental upload produce the same conversation live as on replay?

**Settled live: yes, after two fixes.** **Falsified twice:** the delta was
empty on every continuing turn, so the backend repeated the previous answer;
and once the model returned a reasoning item, every turn from the third on
uploaded the whole conversation. A live four-turn conversation now uploads one
item per turn.

#### Does the `session_id` header matter?

**Settled live: yes, on the HTTP path.** The header scopes the prompt cache;
`prompt_cache_key` alone produced no cached tokens in any trial. Over the
socket it adds nothing, because `previous_response_id` already chains and
caches turns. With the socket disabled, uncached input per turn was
4,465–4,497 without the header against 625–657 with it. **Falsified:** that
`prompt_cache_key` drives caching, and a first probe that shared one prompt
across conditions and let cache entries leak between them. `thread-id` was not
isolated and is not sent.

#### What does the model catalog contain?

**Settled live.** It needs a `client_version` query parameter and filters by
it: below a model's `minimal_client_version` the list is empty rather than an
error. Entries are keyed by `slug`, state `visibility` as a word, and carry
`supported_reasoning_levels`.

#### Is `ultra` gated by plan as well as by model?

**Settled live: yes, both.** It exists only on `gpt-5.6-sol` and needs at least
a Plus subscription; a free plan is refused with `Invalid value: 'ultra'`. The
catalog advertises `ultra` on `gpt-5.6-terra`, which refuses it — the catalog
is a menu, not a contract. The refusal is surfaced verbatim and the plan is
reported by `status`.

#### Does the backend emit `url_citation` annotations?

**Settled live: yes.** A captured search carried `url_citation` annotations
with title, URL, and span, and both reached the client as `web_search_result`
entries.

#### Do the capability probes pass against a live backend?

**Settled live: yes.** **Falsified:** the corpus's image and document payloads
were not real files, so both probes passed on replay while proving nothing; and
a marker spread across several deltas was never contiguous in the raw frames.
The probes now carry a real PNG and a real PDF and match on assembled text.

### The client

#### Is `CLAUDE_CODE_DISABLE_1M_CONTEXT` inert for plain model ids?

**Settled live: no.** Without it the client appends `[1m]` to an unrecognized
id and assumes a million tokens, and adds `context-1m-2025-08-07` to its beta
header. **Falsified:** that the flag was a precaution.

#### Does the context meter stay steady across a turn?

**Settled live: yes.** `message_start` carries the estimate and
`message_delta` replaces it with the true count.

#### Is the true input count linear in the estimator's raw figure?

**Settled live: yes.** Six turns, residuals under 3% from the second turn on;
the uncalibrated first turn was +95%. Recorded in `proxy-behavior.md` §6.3.

#### Does compaction fire at the window the proxy supplies?

**Derived, not confirmed.** The client's own schema measures the trigger
against the supplied `autoCompactWindow` less a summary buffer, and accepts the
value only between 100,000 and 1,000,000, discarding anything outside that
range silently — so the proxy omits it there and warns. A session long enough
to watch compaction happen has not been observed.

#### Do `WebSearch` and `WebFetch` route through the haiku tier?

**Settled live: yes, both.** With haiku on a distinguishable model, both
reported their calls against it while main turns used sonnet's. An unmapped
haiku breaks both in a way that looks unrelated to tier mapping.

#### Do connectors and claude.ai-hosted MCP servers stay out of a proxied session?

**Settled live: yes, and neither setting is what keeps them out.** Auth
precedence disables connectors on any launch whose base URL is a proxy; the
client's own notice says so. `client.disable_connectors` governs only whether
that notice is shown. Launches with `ENABLE_CLAUDEAI_MCP_SERVERS` set, absent,
and overridden to true all attached no claude.ai server, the headless ones
proven by unguessable markers.

#### Does any client read `anthropic-ratelimit-unified-*` from a proxy?

**Settled live: no.** The status-line payload's rate limits are gated on a flag
documented as false for an API-key auth path, which `ANTHROPIC_AUTH_TOKEN`
selects by definition. The headers are still emitted, and the client does read
them for its retry banner on a quota 429.

### Relay path

#### What does the relay need beyond forwarding the body?

**Settled live: nothing beyond `proxy-behavior.md` §9.2.** A setup-token bearer
relayed as `Authorization: Bearer` is accepted, streaming included, without
`oauth-2025-04-20`; that beta belongs to the usage endpoint. The endpoint does
require the client's own identity shape — its beta list, `x-app`, its system
prompt — which the client always sends and the relay forwards. The client posts
`/v1/messages?beta=true`, and the query string is relayed as sent.
**Falsified:** the hypothesis that Messages needs the OAuth beta header, which
was tested rather than implemented.

#### How is a subscription credential for the relay obtained?

**Settled live.** `claude setup-token` mints a bearer valid for about a year,
stored with `accounts add-key --provider anthropic`; a borrowed profile under
`[profiles]` serves as well. **Falsified:** a guard written for
`sk-ant-oat1-`, when a real token begins `sk-ant-oat01-`; the stem `sk-ant-oat`
is what separates a setup token from an API key.

#### Does the relay's routing survive a client that sends a tier name?

**Settled live, then made moot.** A client launched through `exec` sends final
ids and never a tier word. An id no mapping names now follows the account that
would authenticate it, so a relayed unknown id is refused by the provider's own
`not_found_error`.

#### Does the second provider answer a quota question?

**Settled live, per credential.** For a setup token, the usage endpoint answers
`403 permission_error` for want of the `user:profile` scope, with or without
the beta header; the figure comes from the `anthropic-ratelimit-unified-*`
headers of turns already relayed. A borrowed grant carries the scope and is
asked there by `usage --refresh`. For an API key, no quota endpoint is known,
and those accounts report unavailable. Recorded as
`fixtures/upstream/relay-quota-headers.json` and
`fixtures/upstream/relay-usage-scope-refusal.json`.

#### What does the real Messages surface answer with?

**Settled live.** Seven exchanges in `fixtures/surface/` — plain generation,
streaming text, a streaming tool call, a refusal, a sizing call, thinking, and
server-tool search. The emitted surface is held to them as a strict subset:
real answers carry fields this proxy omits (`stop_details`, `caller`,
`service_tier`, `inference_geo`, nested `cache_creation`, a thinking block's
`signature`, `signature_delta`, `citations_delta`), and a client tolerates an
absent field. No emitted shape is left unreached.
