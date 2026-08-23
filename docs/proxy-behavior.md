# Proxy behavior

Normative specification for how proxenos translates between the Anthropic
Messages API and the OpenAI Responses API, and how it manages sessions,
transports, credentials, and token accounting.

This is the definition the code is measured against. [`api.md`](api.md) is the
companion contract for what the proxy *exposes*.

Most rules here exist because the obvious implementation is wrong in a way that
does not fail loudly. Where that is the case, the rule says so.

---

## 1. Premise

Claude Code is not an ordinary Messages API client. Several of its built-in tools
depend on behaviour the server provides, and a translator that handles only
messages and function calls leaves those tools broken while every request still
returns 200.

| Path | Server dependency | Failure when unhandled |
|---|---|---|
| `Read` (image, PDF) | attachment blocks nested inside `tool_result` | bytes never arrive; the model describes the file from its name |
| `WebSearch` | a server-side search tool declared in a secondary conversation | search returns nothing, reported as "no results" |
| `WebFetch` | a model call, believed to be on the haiku tier | fails in a way that looks unrelated to tier mapping |
| tool search | `defer_loading` stubs and `tool_reference` discovery | discovered tools stay uncallable, or every stub inflates context |
| context meter | `input_tokens` in `message_start` | the meter collapses to zero each turn |
| `count_tokens` | pre-flight sizing | absent or wrong |

Preserving these is the product. Everything else in this document serves that.

---

## 2. Request translation

### 2.1 Instructions

Claude Code's system prompt arrives in the top-level `system` field. It maps to
the Responses `instructions` field, never to an item in `input` — the backend
rejects system-role and developer-role messages inside `input`.

A conversation message carrying any role other than `user` or `assistant` is
carried as a `user` input item. The backend rejects the role, not the content.

Folding it into `instructions` instead looks equivalent and is not. The client
attaches per-turn content this way — a billing header among it — so
`instructions` changed on every turn. That breaks two things at once: a delta
requires every non-input field to be unchanged, so every turn uploaded the whole
conversation, and `prompt_cache_key` buys nothing when the cached prefix differs
each time. Measured against a real agent loop: three turns, no deltas at all.
Carried as input items instead, the same loop uploads only what is new.

**Nothing that varies per turn belongs in `instructions`.** Per-turn content
goes in the input, where it appends.

What may be added is text that does not vary: an operator-configured lead before
the system prompt and trailer after it.

The lead exists because the prompt the client sends is written for a different
model and opens by saying so. Nothing else in the request tells the model what
it actually is, and nothing in the client can be made to — its own
append-system-prompt flag reaches the same `system` field, so it can add to that
prompt but never precede it. Stating the identity *after* a prompt that already
asserted a different one reads as a correction rather than a fact, which is why
this is a lead and not part of the trailer.

The trailer is last for the mirrored reason: an instruction meant to take
precedence over the prompt above it has to come after it.

Between the two sits the **working budget**, and it is sent by default. The
premise is measured here rather than assumed: the conversation is replayed
upstream on every turn (§4.3) and the backend echoes it back three times per turn
(§4.4), so context pulled in is paid for repeatedly. Without a budget the model
reads broadly and spends the window fast. It asks for the smallest slice that
answers the question — a targeted search or a bounded line range over a whole
file — and for acting once a read is sufficient.

Its position is deliberate. After the client's prompt, because it exists to
overrule the parts of that prompt asking for broad reading before acting, and an
instruction placed before the one it modifies reads as a suggestion. Before the
operator's trailer, because a shipped default has no business outranking text an
operator wrote on purpose.

It is written as decision rules, with no *always*, *never*, or *must*. Those are
reserved for real invariants: a shipped absolute that collides with the client's
own prompt destabilizes more than a missing detail, and this text sits underneath
a prompt written for a different model that already says a great deal.

All three are per conversation, never per turn. A lead carrying a timestamp or a
token count would change `instructions` on every turn and cost the whole
incremental path — the same failure this section already describes, arriving
through the door built to prevent it.

### 2.2 Content blocks

| Anthropic | Responses |
|---|---|
| `user` / `text` | `message` / `input_text` |
| `user` / `image` | `message` / `input_image` |
| `user` / `document` | `message` / `input_file` |
| `assistant` / `text` | `message` / `output_text` |
| `user` / `document` inside a `tool_result` | `message` / `input_file`, following the output (§2.3) |
| `tool_use` | `function_call` |
| `tool_result` | `function_call_output` (§2.3) |
| `thinking`, `redacted_thinking` | dropped — no equivalent exists |

Base64 image and document sources encode as data URLs. `image_url` is that URL
directly, not an object wrapping one. URL image sources pass through unchanged
and are not prefetched; they resolve only if the backend can reach them.

`input_file` is the one part in this table with no counterpart in the upstream
client, which has no document representation at all. It is the public API's
shape and the only candidate that could carry a document.

**Claude Code does not use it.** Measured: asked to read a PDF, the client
rasterises it and sends `image` blocks, so a PDF reaches the model through the
image path — which is the shape the upstream client does exercise.

The document path exists for a client that sends `document` blocks, and the
backend does accept it — measured by posting one directly, which returned a
code that existed nowhere but inside the PDF. It would fail loudly if not: a
rejected part is a request error, not a silently dropped file.

Assistant content is `output_text` only. An attachment appearing in an assistant
message is dropped rather than converted.

### 2.3 Attachments inside tool results

A tool result is not restricted to text. `function_call_output.output` is either
a bare string or a list of content parts, and an `input_image` part inside that
list is how an image reaches the model — attached to the call that produced it,
with no synthetic message standing between them.

The output collapses to a bare string when, and only when, it is a single piece
of text. Every other case stays a list, including the empty one.

Documents are the exception. No document part exists inside a tool output, so
each is re-emitted as a `user` message placed immediately after the
`function_call_output`, which keeps its text. That placement is not a
preference: `input_file` is defined for message content and nowhere else, so it
is the only position where it could be accepted at all.

This is not an edge case. It is how every file Claude Code reads arrives. Without
it the bytes never reach the model, and the model answers from the filename in
hedged wording that reads as success — the failure is invisible in ordinary
output, which is why §10.3 requires unguessable probes.

### 2.4 Tools

Function tools flatten from `{name, description, input_schema}` to `{type:
"function", name, description, strict, parameters}`. A schema with no
`properties` key gains an empty one.

`strict` is always false. Strict mode constrains the schema — every property
required, no additional properties — and the client's tool schemas do not
comply. Claiming it over a non-compliant schema is a request rejection, not a
stricter model.

`tool_choice` maps: `any` → `required`, `tool` → `{type: "function", name}`,
anything else → `auto`.

### 2.5 Deferred tool loading

Tool discovery happens in the client. Undiscovered tools arrive marked
`defer_loading: true` and are withheld from the upstream request so their schemas
do not occupy context.

The backend has a deferred-loading mechanism of its own, and the flag could be
forwarded to it instead. It is not. Discovery here is driven by the client, and
a second discovery path the client cannot observe would let the model load a
tool whose results never reach the client.

Discovery is observable exactly once: a tool-search result contains
`tool_reference` blocks naming the tools that became available, each as
`{"type": "tool_reference", "tool_name": ...}` — the field is `tool_name`, as
the client sends it, and a block spelling it any other way is not one the
client produces. Those names are recorded on the session, and a recorded tool is forwarded on later turns *even
though it continues to arrive marked `defer_loading`*. That flag is not cleared
by the client, so the recorded set is the only signal that a tool is live.

A tool-search result has no text content, only `tool_reference` blocks. Its
`function_call_output` therefore carries the discovered names serialized as JSON,
so the output is non-empty and the model can tell which tools it may now call.

### 2.6 Web search

`WebSearch` runs as a secondary conversation declaring the server-side search
tool — `{type: "web_search_<version>", name: "web_search"}`, with no
`input_schema`. Any tool whose `type` begins with `web_search` maps to the
Responses API's native `web_search` tool.

Translating it as a function tool produces a tool the model cannot execute and a
search that silently returns nothing.

Both access flags — external and indexed — are stated rather than left to a
default, because a default of false would also produce a search that returns
nothing.

### 2.7 Request fields

Every request sets `stream: true`, `store: false`, `parallel_tool_calls: true`,
and includes `reasoning.encrypted_content`. That is the upstream request and is
unconditional: the backend is always asked to stream, whatever shape the caller
asked to be answered with (§5.5).

`reasoning.effort` derives from the inbound `output_config.effort`, under an
optional ceiling the operator sets. The client cannot choose that ceiling: it
does not know whose quota it is spending, and effort is the largest lever on
what a turn costs.

Two ceilings apply, and the lower wins. The operator's is a cost decision. The
model's comes from the catalog, which states the efforts each one accepts —
they differ, and the client asked for a *tier*, so it cannot know that the model
behind it stops at `xhigh` while another goes to `max`. Forwarding an effort the
model does not support fails the turn for a reason the client could neither
anticipate nor fix. A model whose efforts the catalog never listed caps nothing:
unknown is not a limit.

The ceiling caps and never raises. A request asking for less keeps its own
choice, because capping a maximum is not a request to spend more. With no
request effort at all the ceiling still applies — an operator who capped effort
meant it for the traffic that expresses no preference too, and that is most of
it. With no ceiling and no request effort, the field is omitted and the
backend's own default applies.

`reasoning.summary` is always `auto`.

`prompt_cache_key` derives from session identity (§3.1) and is stable for the
life of a conversation. **It is not what drives the cache.** Sent alone, against
otherwise identical repeated requests, it produced no cached tokens in any
trial — in both orders, with independent prompts per condition. It is kept
because it is harmless and is what the field is for; nothing rests on it.

What the cache actually rests on is measured in §2.8.

Unsupported inbound parameters are dropped through an allowlist rather than
forwarded. Anthropic `cache_control` blocks have no equivalent and are dropped;
upstream caching is implicit.

Server-assigned item ids from a previous response are stripped before an item is
re-sent, and `previous_response_id` is set only by the incremental path (§4.3).

### 2.8 Upstream request headers

| Header | Value |
|---|---|
| `authorization` | `Bearer <access token>` |
| `chatgpt-account-id` | the account id carried in the access token |
| `originator` | a single fixed first-party originator |
| `user-agent` | matching that originator |
| `openai-beta` | the Responses experimental opt-in |
| `accept` | `text/event-stream` on the HTTP transport |

All of these go on the WebSocket upgrade too. The upgrade is an HTTP request
like any other, and `originator` and `user-agent` were once absent from it while
every other path sent them — the socket did not enforce them, so nothing failed
and nothing said so.

**`session_id` carries the prompt cache scope**, and is stable for the life of a
conversation — a UUID, because that is the shape measured to work; whether an
arbitrary string is accepted there is unmeasured.

What it is worth depends on the transport, which is the part that is easy to get
wrong. Over WebSocket it changes nothing: the incremental path chains turns with
`previous_response_id` (§4.3), and that already caches. Over HTTP every turn is
a full send with no chain, and there the header is the whole difference —
measured on one four-turn conversation, uncached input per turn fell from
4,465–4,497 tokens to 625–657, with 3,840 reported cached from the second turn
on.

That makes it a fallback-path optimisation rather than a universal one, which is
worth stating plainly: HTTP is a normal operating mode here (§4.2), not an error
path, so a turn that costs seven times its input tokens is a real cost and not a
hypothetical one.

One originator, always, with no alternate to fall back to. A rejection at this
layer surfaces as an error rather than triggering a retry under a different
identity: a fallback identity is state that has to be tracked, invalidates the
prompt cache when it changes, and turns one clear failure into two unclear ones.

A challenge response — a non-JSON body on a 403 — is reported as an `api_error`
with the body excerpt intact, because the excerpt is the only diagnostic
available.

---

## 3. Sessions

### 3.1 Identity

Claude Code sends no session identifier. Identity is derived from content: a
request belongs to an existing session when its `input` is a strict extension of
that session's baseline.

This is the same predicate that governs incremental upload (§4.3), so session
matching and delta computation share one definition rather than two that can
disagree.

Items are compared by content, not by encoding. A server-assigned `id` is
absent when the client replays the same turn, so ids are excluded. A tool call's
`arguments` travel as a JSON *string*, and the backend emits its keys in the
order the model produced them while the client replays the object it parsed, in
whatever order its own serializer chose — so arguments are compared as parsed
values where they parse, and as literal text where they do not. Measured live:
compared as text, every turn after the model wrote a file forked the
conversation and uploaded the whole history again.

Two conversations that genuinely share a prefix — the same system prompt and the
same opening turn — are indistinguishable until they diverge, and may match the
same session. This is harmless: the shared prefix is identical, so the baseline
is correct for both, and the first divergent turn separates them. What must not
happen is a match on a *partial* prefix, which is why the predicate requires a
strict extension of the full baseline rather than a longest-common-prefix score.

### 3.2 State

A session holds its input baseline and the output items the server added, its
transport binding, its discovered tool names, its retained reasoning items
(§3.3), and its estimator calibration ratio. Sessions expire on idle, and the
store is bounded — eviction is by least recent use, never by refusing a request.

### 3.3 Reasoning continuity

Requests ask for `reasoning.encrypted_content`, so responses carry reasoning
items the model expects to see again on the next turn.

Those items cannot survive a round trip through the client. Anthropic `thinking`
blocks are dropped on the request path (§2.2), and the client would not return
encrypted upstream reasoning even if they were not. Every turn would therefore
begin with the model's prior reasoning discarded.

The session retains server-returned reasoning items and re-injects them in their
original position on the next request. They are part of the baseline for §4.3 in
exactly the same way other server-returned output items are, so the incremental
and full-send paths agree on what the conversation contains.

A conversation is therefore held in two forms. What the client replays can
never contain the server's reasoning; what the backend holds does. Reconciling
converts the first into the second, and the delta is computed on the second by
strict comparison. Running the reconciling rule on an already-reconciled input
misaligns exactly the items it put back, so the order matters and is not
interchangeable.

Re-injection is not optional, and not only about quality. A baseline holding an
item the client cannot replay is never a strict extension of any later replay,
so a strict comparison stops matching the moment the model reasons. Session
identity (§3.1) and delta computation (§4.3) therefore both judge continuation
by the *reconciling* predicate: server-only items in the baseline are matched
past rather than matched against. Without that, a conversation silently
restarts on its third turn — new session, lost calibration, lost discovered
tools, and a full upload every turn thereafter.

This is the one place the proxy adds content the client did not send. It is
additive and upstream-only: nothing synthesized here is ever surfaced back to the
client as model output.

---

## 4. Transport

Everything in this section describes the first provider's path. The transports
below are interchangeable with each other and neither is a degraded form of the
other — but that interchangeability stops at the provider: the relay in §9 uses
HTTP with SSE and nothing else, because WebSocket and incremental upload are
this backend's protocol rather than a general capability. **The choice of
transport belongs to the provider, not to the session.**

### 4.1 WebSocket

WebSocket is primary. One connection is cached per session and opened lazily.
Reuse removes per-turn TCP and TLS setup, which is significant in an agent loop
issuing many sequential requests.

A prewarm request opens a connection before the turn that will use it, so that
turn pays for neither the handshake nor a cold connection.

The daemon does not prewarm in v0.1, and the capability exists unused. A proxy
learns that a conversation exists only when its first request arrives, at which
point opening the connection and sending on it are the same act. Prewarming
needs a signal that a turn is *about* to happen — a front-end that knows the
user is typing has one; an HTTP surface does not.

### 4.2 HTTP fallback

HTTP with SSE is a complete, independently correct transport — not a degraded
path. The backend closes WebSocket connections under policy conditions often
enough that fallback is a normal operating mode.

A session that fails to establish or maintain a WebSocket latches to HTTP for the
rest of its life rather than retrying every turn.

### 4.3 Incremental input

The Messages API is stateless, so the client replays the whole conversation every
turn. Over HTTP with `store: false` the full transcript is re-uploaded each time.
In a long session that dominates both upload cost and time to first token.

On a reused connection only new items are sent, with the previous response id. A
delta is valid only when every non-input request field is unchanged *and* the new
input is a strict extension of the previous input plus the output items the
server added. Server-returned items are part of the baseline and are never
resent.

The connection is part of that validity, not just the session. A response id
names a response held by the connection that produced it, so a delta may only be
sent on the connection that has seen that response. Handed to any other — a
fresh connection opened after an abandoned turn dropped the previous one, or a
connection parked by a turn that produced no response — the backend refuses it
with `400 Invalid previous_response_id` (observed live). That refusal ends the
turn cleanly, so the refusing connection is parked and every following delta
repeats the refusal: the session never heals on its own. A turn whose pooled
connection did not produce the response it would continue is therefore a full
send.

Any mismatch sends the full input. So does a delta that would be empty: the
backend given a previous response id and no new items answers from that
response, so a client retrying an unchanged conversation would be handed the
previous turn again instead of a fresh one.

A turn only enters the baseline once the backend has accepted it. Recording one
that failed would make the next delta continue a response that never saw those
items, and the question would vanish from the conversation without any error.
A brand-new session is the exception: it claims its conversation immediately, so
a concurrent request cannot match its empty baseline and join a conversation it
has nothing to do with. Nothing is at risk there, because a session with no
completed turn has no response to continue and can only send in full.

**Falling back is always safe; a wrong delta is not.** A full send costs
bandwidth. A wrong delta corrupts the conversation and does not fail visibly.
Every ambiguous case resolves toward the full send, and the check is conservative
by construction.

### 4.4 Compression

Compression belongs to the transport, and the two transports do it differently.

**HTTP**: the body is zstd-compressed and announced with `Content-Encoding:
zstd`. The header is the whole mechanism — compressed bytes without it are bytes
the backend cannot parse, and it refuses the request with an error naming
nothing. Only bodies above a threshold are compressed; below it, compression
adds more than it removes.

This is measured against the subscription backend and asserted about no other.
A request spent with a key is never compressed, whatever `[transport]` says —
§8.2 carries what happens when it is.

**WebSocket**: `permessage-deflate`, negotiated during the upgrade rather than
chosen per message. The client offers the extension and the server selects it
(RFC 7692), so declining to offer it is the only way to switch it off. The frame
is text JSON either way — the library compresses that same text frame and marks
it in the frame header, not in the payload.

**Measured live**, one identical turn with the extension offered and declined,
counted on the wire rather than simulated:

| | offered | declined |
|---|---|---|
| inbound | 104,566 | 300,879 |
| outbound | 40,335 | 110,608 |

About 65% in both directions. The inbound half is the larger one and grows with
the conversation, because the backend echoes the entire request back in
`response.created`, `response.in_progress`, and `response.completed` — three
copies of it per turn, which nothing else here has a lever on.

Two further figures are **derived, not confirmed**: running deflate offline over
a captured turn predicted 37,488 and 99,100 bytes, and put the contribution of
context takeover at about 3% — the window is 32 KiB and cannot reach back across
a 99 KB event. The server does negotiate takeover (it selects bare
`permessage-deflate`, with no `no_context_takeover` and no window limit), but
nothing here has measured what it is worth on the wire.

The read limit is raised far above the library's 1 MiB default for the same
reason: one event legitimately carries a whole conversation, and a cap sized for
ordinary messages would sever long ones mid-turn.

This saves bytes and **no tokens at all**. It is worth doing because two thirds
of the traffic is an echo the proxy has no other lever on, not because bandwidth
is scarce.

A binary frame is *not* a way to say "compressed". Nothing in the protocol
attaches that meaning to it: the backend reads a binary frame as JSON, fails,
and refuses the request. Measured directly — plain JSON in a binary frame is
accepted, the same JSON compressed is not.

This compounds with §4.3 where it applies: incremental upload removes most
turns' bulk, and compression reduces what remains on the turns where a full send
is unavoidable. Those are the HTTP turns, which is where compression is
available.

---

## 5. Response translation

### 5.0 Framing

On the HTTP transport, events arrive as SSE. An event block may carry more than
one `data:` line, and the SSE specification defines those as one logical payload
joined with newlines — not as independent JSON documents. Parsing each line
separately corrupts any event large enough to be split, which is exactly the
events that matter: long tool-call arguments and long text deltas.

A `data:` payload of `[DONE]` is a terminator, not content. A payload that does
not parse as JSON is ignored rather than treated as an error.

On the WebSocket transport the same events arrive as discrete messages and need
no reassembly. Both transports produce the same event stream before translation
begins, so §5.1 onward is transport-independent.

### 5.1 Events

Responses events become Anthropic SSE frames through one state machine.
Anthropic permits a single open content block at a time.

| Responses event | Anthropic output |
|---|---|
| `response.created` | `message_start` |
| `response.reasoning_summary_text.delta` | `thinking` block, `thinking_delta` |
| `response.output_text.delta` | `text` block, `text_delta` |
| `response.output_item.added` (function call) | `tool_use` block |
| `response.function_call_arguments.delta` | `input_json_delta` |
| `response.output_item.done` | `content_block_stop` |
| `response.completed` / `.done` | `message_delta` + `message_stop` |
| `response.incomplete` | `message_delta`, `stop_reason: max_tokens` |
| `error`, `response.failed` | `error` frame |

A `tool_use` block's `content_block_start` is deferred until the function name is
known, because Anthropic clients cannot patch a block header after it is emitted.

`stop_reason` is `tool_use` when the turn produced any function call,
`max_tokens` on an incomplete response, `end_turn` otherwise.

A stream opening with a capacity or overload condition becomes an
`overloaded_error` frame so the client retries on its own.

### 5.2 Search results

The backend runs web search server-side and reports it through search call items
and citation annotations. These are reconstructed into Anthropic's structured
shapes — `server_tool_use` and `web_search_tool_result` blocks carrying `url` and
`title` per result.

The client extracts `url` and `title` from those blocks. Passing the model's prose
answer through as the tool result leaves that extraction empty, so the structured
form is required, not preferred.

A search call names the query. The sources come from `url_citation`
annotations, which arrive while the answer is being written — after the search
itself has completed. Both blocks are therefore emitted as the message closes
rather than where the search ran. Their position in the message does not affect
what the client extracts.

Citations are the one part of this the upstream client cannot corroborate: it
discards annotations entirely and so never sees a cited URL. The annotation
shape is the public API's, and whether this backend emits it is a §L question.

A page the model opened is treated as a source even when nothing cited it.
Without that, a search that fetched pages but produced no citations reaches the
client as an empty result — which reads as "nothing found", the precise failure
this section exists to prevent.

A source cited repeatedly is one result. The client renders the list verbatim.

### 5.3 Cancellation

Cancelling the outbound stream aborts the upstream request. Without propagation
the backend generates to completion against a reader that no longer exists,
spending quota on output nobody receives.

### 5.4 Empty streams

A stream that completes having produced no content frames is recorded with its
request and the raw upstream bytes. It is always a defect, and it is otherwise
invisible.

### 5.5 A request that did not ask for a stream

`stream` is the caller's choice and the endpoint's default is not a stream. A
request that omits it, or sets it `false`, is answered with one
`application/json` message body.

There is only one thing to build that body out of: the frame sequence of §5.1.
It is folded shut — content blocks closed, deltas concatenated, tool arguments
parsed back from the fragments that spelled them. The fold is pure over the
frames and invents nothing: arguments that do not parse are a failure in the
error shape of `docs/api.md` §1.1, never an object that looks plausible.

The usage reported is the one `message_delta` carries and never the estimate in
`message_start` (§6.1, §6.2). A body is a completed turn, and a completed turn
has upstream's own figures.

Which shape the caller asked for changes what is written and nothing else.
Calibration, session bookkeeping (§3.3, §4.3) and capture (§5.4) run off the
same sequence either way, so a non-streaming turn advances a conversation
exactly as a streaming one does.

Because nothing is written until the fold is done, a failure on this path is a
status and an error body — the opposite of the streaming path's constraint, for
the same reason: the status is still the proxy's to choose.

Claude Code always streams, so the harness never takes this path. It exists
because the ingress claims to be a Messages API, and a caller that did not ask
for `text/event-stream` did not agree to parse one.

---

## 6. Token accounting

### 6.1 Upstream figures are authoritative

Completed responses report real input, output, and cached token counts. These are
never recomputed.

One conversion is required. OpenAI's `input_tokens` includes cached tokens;
Anthropic's excludes them and reports cache counters separately. So `input_tokens`
becomes `input_tokens - cached_tokens`, clamped at zero, and `cached_tokens`
becomes `cache_read_input_tokens`.

`cache_creation_input_tokens` is always zero. Upstream caching is implicit, with
no distinct write event to report. It stays zero rather than being synthesized
into something plausible.

**What was never observed is reported as unobserved, not as nothing.** The same
rule reaches past a turn's own counters to the quota figures of §8.3: this
daemon records the turns that pass through it, and a turn spent elsewhere —
`doctor --live` relays one from a CLI process that exits holding its response
headers — leaves it nothing. So an account with no figure reports that *this
daemon* has recorded no turn as it, which is a claim the store can make. "None
has been spent as this account" is a claim about the account, and the store has
no standing to make it.

**The same counts are tallied per account, and never become a cost.** A
completed turn's input and output counts are added to the account that served
it — the pinned account where a tier pinned one, the serving account otherwise
(§8.3) — so a metered account can be told how much has been spent through this
daemon. Nothing here knows a price list, and none is inferred: the tally is a
quantity of tokens and stays one. A turn upstream reported no usage for adds
nothing rather than zero, and a turn no account can be named for is not counted
at all, because filing it under whoever happens to be serving would put one
account's spend under another's name.

**It is a floor, not the account's spend.** Turns made anywhere else are
invisible to it. Everywhere it is reported it says so, because a figure that
reads as the whole of an account's spend is wrong in the reassuring direction.

**The tally persists; the quota snapshot does not.** The two halves of what a
restart loses are not equally recoverable, and they are settled differently.

- The **quota snapshot** of §8.3 stays in memory only. Upstream still holds it,
  so an ask recovers it exactly, and a percentage read back from disk describes
  a window that may have reset since — headroom that may no longer exist, which
  is the reassuring direction again. The empty row after a restart is the
  honest one, and `usage --refresh` is how it is filled.
- The **token tally** is written to disk and read back at startup. Nothing
  upstream can restate it: it is what *this daemon* served, counted from
  completed responses. A restart that reset it to zero would state a floor of
  zero, which is not a figure that was measured.

The tally lives in `spend.json` beside the configuration, under
`config_dir()`. It is daemon state rather than configuration, and deliberately
not the credential store: it holds an account name and two token counts, and no
place for any part of a secret to be written. Forgetting an account removes its
row, the same way it drops its figure.

**A write replaces the file; it never writes into it.** `std::fs::write`
truncates the target and then fills it, and a daemon killed between those two
leaves a short file that parses into nothing — read back as an empty tally and
reported as a floor of zero, which is the defect this section exists to remove.
`proxenos stop` under the supervisor kills the daemon on every install, so that
is the ordinary shutdown rather than a rare one. The body is written to a
sibling carrying the process id, flushed, and renamed over the target, so a
reader sees the last finished write or the new one. A sibling left behind by a
killed write is never read.

The file is written at the process umask rather than `0600`. That is
deliberate: it holds an account name and two token counts and no part of any
credential, and the restriction the credential store needs would state
something about this file that is not true.

`PROXENOS_HOME` can point two daemons at one directory, and neither sees the
other's turns. Two things keep that from costing a count. The **merge** takes
whichever count is higher per account, so a daemon that has been running longer
never has its total replaced by a younger one's. The **comparison** covers what
the merge cannot: the merge reads the file once, and a write that landed after
that read is not in what this one is about to replace it with, so the file is
re-read before the replacement and the attempt starts over against the newer
file. Five attempts, then the last one writes what it has.

This does not close the window and does not claim to — the comparison and the
rename are two operations, and a write landing between them is still lost.
What remains is a smaller floor, never a corrupted file and never a count that
moved backwards for any writer that takes the same path. No lock is taken, for
the difference `auth/store.rs` §8 turns on: a lost credential write is a whole
account, a lost tally write is one turn's count.

A file that cannot be read or parsed is treated as an empty tally and written
over — nothing here is worth refusing to serve a turn over, and a tally that
starts at zero says so everywhere it is reported.

The write is blocking I/O on the async worker that served the turn. It runs
once per completed turn rather than per event, over a file of a few hundred
bytes, and it is not moved off the runtime because a spawned write is a write a
shutdown can outrun.

### 6.2 The two points that need an estimate

`count_tokens` is a pre-flight call: nothing has been sent, and the Responses API
has no token-counting endpoint.

`message_start` carries `input_tokens` in Anthropic's protocol, but upstream
reports usage only at completion. Emitting zero is not neutral — the client
renders that value live, so the context meter collapses to zero at the start of
every turn and snaps back when the real figure arrives.

Both use a local estimator, and both are followed by ground truth within the same
exchange. `message_delta` carries cumulative final usage, not an increment, so
writing the true value there replaces the estimate rather than adding to it.

### 6.3 Calibration

The estimator corrects itself against upstream. Each completed request yields a
true input count for a request that was also estimated, and the pair is folded
into a fit retained on the session.

**The fit is a line, not a multiplier.** Part of the unmodelled cost scales with
the conversation and part does not: the instructions wrapper is charged once
however long the session runs. A single ratio cannot represent both. Fitting one
anyway makes it converge from whichever regime it saw first — an early short
request, where the fixed cost dominates, pulls the ratio high, and it then
decays for the remainder of the session while every estimate reads over. Scale
and offset are fitted together instead, by incremental least squares.

Where the fit is underdetermined it is not invented. One observation, or several
at the same size, cannot separate scale from offset; the estimator falls back to
a plain ratio and extrapolates nothing.

This absorbs what a tokenizer alone cannot. The upstream count includes framing
the proxy does not model identically — the instructions blob, serialized tool
schemas, per-item overhead. A byte-exact tokenizer over structurally different
inputs produces a number that is authoritatively wrong, which is worse than one
that is approximate and self-correcting.

**The measurement, and what it settles.** Both estimators were run over a
growing multi-turn session against a modelled upstream count — text cost plus a
per-item framing charge plus a fixed wrapper. Mean absolute error over the
second half: **0.01% calibrated, 68% tokenizer**. The tokenizer is low by
almost exactly the framing it cannot see, and no amount of exactness closes
that, because the gap is not in the text.

The calibrated estimator therefore ships and the tokenizer stays behind a
feature flag, as a comparison instrument rather than a candidate.

That comparison was against a *modelled* count, linear in the same structure the
raw estimate measures, so a linear fit could absorb it exactly. It demonstrated
the mechanism and not the accuracy.

**Measured against the real backend**, over a growing six-turn conversation:

| turn | estimated | actual | error |
|---|---|---|---|
| 1 | 146 | 75 | +95% |
| 2 | 221 | 215 | +2.8% |
| 3 | 420 | 427 | −1.6% |
| 4 | 703 | 687 | +2.3% |
| 5 | 1026 | 1007 | +1.9% |
| 6 | 1406 | 1387 | +1.4% |

So the real relationship is tractable: one observation is enough to bring the
estimate inside 3%, and it stays there as the conversation grows.

The first turn is the weak point and cannot be otherwise — nothing has been
observed yet, so it is the uncalibrated ratio and here it nearly doubled the
true figure. It is the one turn where the context meter is visibly wrong, and it
corrects on the next one.

Before a session's first completed request the estimate is uncalibrated.

---

## 7. Models

### 7.0 Catalog

The catalog is fetched from the backend at startup and held for the life of the
daemon. There is no TTL: a model added, renamed, or withdrawn after the daemon
started is not noticed until something else makes it ask again. The one thing
that does is the daemon changing which account it serves, for the reason below.
That is also why a mapping validated at startup cannot go stale on its own, and
why the only mismatch worth reporting otherwise is a mapped model the catalog
withholds (§7.1).

**A catalog is one account's menu**, and one provider's. The plan decides which
models appear and which efforts each one offers, so a list fetched for one
account is not a statement about another — and a mapping entry whose turns are
relayed to the second provider is measured against no list here at all (§9.1). It is attributed to the account it was fetched for,
and fetched again when the daemon changes which account it serves — selecting
another, or forgetting the one that was serving. A failed refetch **keeps the
list already in force**: fetch failure is not evidence that a model went away
(§7.1), and replacing a real list with the fallback would withdraw models the
account has.

Attribution is what covers the rest. A grant can still arrive without anything
to refetch on: a login started over the control socket completes in the
background, and a login made in the CLI while no daemon is running has no socket
to hand over on. The list stays the previous account's, and every answer built
from it says so (`api.md` §3) rather than presenting it as this account's.

Each entry contributes an id, a visibility flag, and window metadata: a context
window, an optional maximum context window, and an optional effective percentage.
Hidden entries and non-conversational pseudo-models are excluded from what is
offered for mapping, but their window metadata is retained — a session may
reference a model the picker filters out, and knowing its window is better than
not.

The effective window is the context window scaled by the effective percentage,
which reserves headroom for instructions, tool overhead, and output. Where the
entry states no percentage, the configured `upstream.effective_window_percent`
applies — a default, never an override: a percentage the catalog states for its
own model wins. It is resolved when the catalog is parsed, so there is no
compiled-in figure left to fall back to. Where both a context window
and a maximum context window are present, the smaller-scoped `context_window` is
authoritative — the maximum describes a ceiling the account may not have.

A fixed fallback list covers a failed fetch, so the daemon starts and reports
honestly rather than blocking on an unreachable catalog. The fallback carries ids
only. A model with no known window is **unknown, not assumed**: the window guard
(§7.2) does not fire for it, and no percentage is derived from a guess.

Fetch failure is not the same claim as absence. Validation that depends on the
catalog is skipped when the catalog is unavailable, never failed.

### 7.1 Tier mapping

All four tiers — `opus`, `sonnet`, `haiku`, `fable` — are mapped, by the
operator or by the shipped defaults, and each is validated against the live
catalog. The daemon refuses to start on an invalid mapping; an incomplete one is
completed rather than refused.

The client routes different work to different tiers, and background and
summarization traffic runs on the cheapest one. An earlier rule required all four
to be stated, on the grounds that a defaulted mapping hides which model handles
that traffic and what it costs. `status` prints the mapping in use whether or not
it was written down, which meets that concern without making a first run fail on
a file nobody had written yet. A tier written blank is still refused: an omission
accepts the default, a blank is a mistake.

If the catalog cannot be fetched, validation is skipped rather than failed. An
unreachable catalog is not evidence that a model went away.

**The mapping belongs to an account.** A catalog is one account's menu (§7.0),
so a single mapping is only ever right for the models every stored account has,
and that intersection shrinks with each account added: two subscriptions on
different plans are offered different models, and a key account beside a
subscription need not overlap at all. An account therefore states what differs
for it — the tiers it names and nothing else, plus its own effort ceiling — and
the shared tables answer for everything it does not state. Keyed by the name the
store files it under, because that is what every account verb takes and a key
account carries no id to be named by.

**A change is persisted where the value is read from.** An account section
shadows the shared table for what it names, so writing a change to the shared
table while such a section exists would leave it in force on the running daemon
and gone at the next start. The account tables are therefore read from disk when
they are needed rather than from the snapshot taken at startup: they are the one
part of the configuration the daemon writes, and a daemon that cannot see its
own writes gets this wrong in both directions — a mapping persisted for an
account and then ignored when that account is selected, and a later change
written to the shared table because the section it just created is not in the
snapshot that decides where to write. `api.md` §3 carries how each method
chooses.

**A switch re-resolves the mapping and can be refused by it.** Selecting an
account resolves that account's tiers and ceiling and validates them against
the catalog fetched for it, before anything else moves. A mapping naming a
model that account's catalog does not have refuses the switch and leaves the
daemon where it was, catalog included — the alternative is a daemon serving an
account whose every turn is dispatched to a model the backend will not answer
for, which fails one turn later, upstream, saying nothing about tier mapping.
Validation is skipped where the catalog cannot speak for the account being
selected, which is the fallback list and a refetch that failed; refusing a
mapping over somebody else's menu would be worse than not checking. **The
refusal names the account section as the way out.** Naming the model and the
list leaves an operator with one shared table and two plans, editing it before
every switch and undoing the edit after; the section that states what differs
for the account being switched to is what the refusal points at.

**A pinned tier is served as the account it names.** A tier entry may pin
another account, and what that decides is which credential authenticates the
turn: the pinned account's, on every upstream request the tier produces, while
every unpinned tier keeps using the account serving turns. The mapping a turn
resolves against carries the account beside the two model ids, because that
table is the only thing a turn resolves against — an account left out of it
arrives at the transport as no account at all.

**A pinned tier is not validated against the serving account's catalog.** Its
model belongs to the pinned account's menu (§7.0), and that menu is not the list
in force — one catalog is held, for the account serving turns. Refusing a spare
account's model because the serving account is not offered it is the exact case
per-account mappings exist for, whichever provider either account is on.

The exclusion holds at every door onto the mapping: the daemon's start,
`tiers.set`, and a switch. It did not always, and the disagreement was silent
until a restart — the socket accepted a pinned entry and persisted it, and the
next start refused the daemon over the same entry, which is the failure the
write-time check exists to prevent rather than to produce. One function answers
for all three now, so they cannot drift apart again.

A pin naming an account the store does not hold **refuses the turn**, and the
refusal names the account and lists what is stored. There is no fallback to the
serving account: the turn would succeed, read identically to a correct one, and
spend a subscription nobody pointed at it. That is the same reason the consent
key exists, and it is the reason nothing about this is inferred — a mapping and
a store are edited separately, and either one can be the half that is wrong. A
pinned account holding a credential of the wrong kind is refused the same way
§8.2 refuses a mismatched selection, naming the pinned account rather than the
selected one.

**A refresh on a pinned grant goes back where it was read.** Refresh state — the
single-flight lock, and which refresh token the backend has refused — describes
one refresh-token family, so a pinned account gets its own token source rather
than the shared one pointed at a second store. The write matters more: §8.1
resolves a rotation by account id and falls back to the *selection* where the
grant carries none, which for a pinned account is another account's entry. A
grant read for a named account is written back to that account, with the name
standing where the selection stood.

**A pooled socket belongs to the account that opened it.** A connection
authenticates once, at the upgrade, and then carries every turn sent over it
(§4.1). One opened as another account is dropped rather than reused — the reuse
that makes §4.1 worth anything is exactly wrong across accounts. HTTP needs no
such rule: every request carries its own credential.

An account's ceiling **replaces** the shared one rather than being capped by it.
Capping would make an account section unable to raise, and an operator who
writes a different ceiling for one account means that one. The cap that is not
negotiable is the model's own (§2.7), and that one is derived from the catalog
rather than from either line.

### 7.2 Context window

A mapped model id must not contain a `[1m]` marker; the daemon rejects one that
does.

The client infers a context window from the model id. An unrecognized id yields a
200,000-token assumption; an id carrying `[1m]` yields 1,000,000.

Real windows are smaller than 1,000,000, so the marker would make the client
believe it has roughly four times the headroom it has, and auto-compaction would
never fire before the window overran. The 200,000 assumption sits *below* the real
effective window instead, so compaction runs early. Early compaction wastes
context; late compaction fails the session.

The generated environment sets `CLAUDE_CODE_DISABLE_1M_CONTEXT=1`, and it is not
a precaution against a hypothetical future client — it is load-bearing now.
Measured: without it, this client appends `[1m]` to the unrecognized id and
assumes a million tokens. With it, the id stays plain. That is the four-times
overestimate this section warns about, and the flag is what prevents it.

**The same flag governs a wire header.** Measured on ingress capture: without it
the client adds `context-1m-2025-08-07` to `anthropic-beta`; with it the beta is
absent. On an id the client does not recognize that costs nothing, because the
entitlement is not the proxy's to claim. On a relayed id (§9) it is an
entitlement the account may actually hold, and the flag denies it.

So the flag is set **only where at least one tier still translates**. A mapping
served entirely by the relay omits it: there is no unrecognized id left for it to
protect, and every id it would affect is one the client knows. A split mapping
keeps it, because the two costs are not symmetrical — a denied entitlement makes
a session smaller than it could have been, while a fabricated million-token
window makes one that overruns.

Where the catalog knows the window, the environment also states it:
`CLAUDE_CODE_MAX_CONTEXT_TOKENS` and `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, both set
to the effective window, and to the smallest across the mapped tiers since one
value covers them all.

**Neither is stated once any tier is relayed** (§9.1). The client recognizes
those ids natively and knows their windows already, and the catalog this figure
comes from is not their menu — so on a mapping served entirely by the relay an
override could only replace a real window with an invented one.

A split mapping states neither as well, and that is the one place this costs
something: the tiers that still translate fall back to the client's own 200,000
assumption and compact early. It buys the guarantee that matters more. One
variable governs every tier, so a figure taken from the first provider's catalog
would govern the relayed tiers too — and those are the tiers with nothing behind
them, since this proxy's own window guard sits on the translating path and the
relay path has none (§9). An early compaction wastes context on the side that is
still guarded; a late one fails the session on the side that is not.

Both are needed. Stating the window alone is worse than saying nothing: the
client stops applying its own 200,000 assumption and, not recognizing the model,
then enforces no limit at all — so the session grows until the backend refuses
it. The compact window is what turns a stated figure into an enforced one.

**`CLAUDE_CODE_AUTO_COMPACT_WINDOW` is only set when the effective window falls
between 100,000 and 1,000,000**, which is the range the client accepts. Outside
it, the client answers "Expected 'auto' or 100k–1M tokens", and the settings key
of the same meaning is declared to *discard* an out-of-range value rather than
reject it — so a figure outside the range is not an early compaction or a late
one, it is no setting at all, and nothing would say so. The proxy omits it and
warns instead. Both ends are reachable: a small model can fall under the floor,
and so can a large window with a low `upstream.effective_window_percent`.

Compaction does fire against this figure. The client's own threshold is
described as the effective window minus a summary buffer, lowered further by an
override it reads separately, and the history-length check compares the token
count against a function of that window. **Derived from the client's code, not
observed** — no session here has been run long enough to watch it happen.

The client warns that its 200,000 limit is not being enforced. That warning is
correct and expected: exceeding 200,000 is the point, and the real window is
larger. It is silenced only by compacting at 200,000, which would discard a
fifth of the usable context to avoid a message.

The percentage the client displays is computed client-side and is now computed
against the right number.

The proxy independently enforces the real window from catalog metadata, rejecting
an over-window request with a clear error rather than forwarding it into an opaque
upstream rejection.

### 7.3 Client policy

Two of the things a client has to be told cannot be an environment variable.
They live in its settings file, and no export reaches them: checked against the
whole settings schema, there is no per-skill variable and nothing that points at
an extra settings file. The one variable that comes close relocates the client's
entire state directory, credentials and history included, which is not a price
this buys anything worth.

So the proxy **publishes** this policy and never installs it. It is emitted
beside the environment (`docs/api.md` §2.2) and applied by whoever starts the
client: a person writing it into a settings file, `exec` splicing it into one
launch (§2.3), or a supervisor merging it into the argument list it already
builds. Nothing here writes into a file the proxy does not own.

Settings layers union, measured: a rule in a project settings file and a rule on
the command line were both enforced in the same session, while a control skill
denied by neither still launched. A deny rule also survives an untrusted
workspace, where an allow rule is dropped. So the policy can be delivered
through any layer without displacing what is already there — with one exception,
which is that **two `--settings` flags on one argument list are not two layers**:
the client keeps the last and drops the first, silently. That is why `exec`
refuses a collision rather than choosing a side, and why a supervisor that
already passes the flag merges into its own document rather than adding a
second.

**A bundled skill is denied by default.** `claude-api` is a reference for another
provider's API. A session served here is not talking to that API, so the
reference is wrong twice over: it costs context, and a model that reads it
answers confidently about model ids, prices, and parameters that are not its own.

The figures are measured, against a local capture stub with nothing forwarded
anywhere:

| | |
|---|---|
| Skill content injected on one invocation | 73,000 to 93,000 bytes, roughly 18,000 to 23,000 tokens |
| Cost of a refused invocation | one 43-byte error result |
| Effect on the listing the client sends | **none** |

That third row is the one that decides the design. Denying does not remove the
skill from the listing, so the model may still reach for it and lose a turn
finding out. What the deny stops is the load. The arithmetic is what settles it:
one blocked call costs 43 bytes, one allowed call costs four orders of magnitude
more than that and then keeps costing.

The skill figure is a range because both ends were measured and it moves with
what else the session has loaded — the same probe read 92,601 bytes in a
populated environment and 73,214 in a bare one. Quoting either alone would claim
a precision the measurement does not have. Nothing about the decision turns on
where in that range it lands.

**What "keeps costing" means precisely.** In tokens, always: the content lands as
a user item, so it sits in the conversation for the rest of the session and is
charged on every turn, and it moves compaction earlier by that much. In bytes, it
depends on the transport. Over HTTP the whole conversation is re-sent every turn
(§4.2), so it is re-uploaded each time. Over WebSocket the incremental path sends
only what is new (§4.3), so it is uploaded once — but the backend echoes the
entire request back in `response.created`, `response.in_progress`, and
`response.completed` (§4.4), so it returns three times per turn on either
transport. The token cost is the one that holds regardless.

**The connector notice** is suppressed by the same document. The client prints it
whenever an auth token is set, which here is always. Whether the setting still
silences it on the current client version is **unverified here** — see
`docs/roadmap.md` §L.

**The connectors themselves are switched off through the environment.** The same
`disable_connectors` key also emits `ENABLE_CLAUDEAI_MCP_SERVERS=false` among the
routing exports — the client's own documented opt-out for its claude.ai-hosted
servers, and the one piece of this policy an export can carry. It matters for the
launch the settings document never reaches: a client configured by `proxenos env`
alone would otherwise load connectors against an account the backend here cannot
serve. The variable does not silence the notice — that is the settings key's half,
which is why both exist. Whether the current client still honours the variable is
the same kind of open question as the notice — see `docs/roadmap.md` §L.

**Both are switchable**, in `[client]`, where the comments explaining them live.
An operator building against that provider's API wants the reference the rest of
us are paying to avoid, and an empty `deny_skills` gives it back. The default is
on for the same reason the working budget is (§2.1): the cost was measured and
the alternative is worse.

**The policy is published even when it is empty.** One file is both the daemon
and the CLI, and replacing it on disk does not restart a running daemon, so a
newer CLI against an older daemon is what an ordinary upgrade leaves behind. If
an empty policy and a daemon that cannot answer for one looked the same, nothing
could tell the operator which they had — so the payload always carries the field
and absence means only that the daemon predates this. The verbs whose output
would otherwise be quietly incomplete refuse; the one that carries routing alone
continues and says which daemon answered. `docs/api.md` §2.2 and §6.

**A denied call is attributed by `status` and nowhere else.** The client refuses
with "Skill execution blocked by permission rules" and names no source, so
`status` reports the policy under the configuration's own key names — the person
holding that message needs the key that undoes it, not a restatement of it.

---

## 8. Credentials

Authentication is borrowed. This proxy operates no authorization flow of its
own and holds no refresh-token family: a subscription grant is read from the
profile of the program that owns it (§8.4), and the only credential it stores
itself is a key, which has no flow behind it (§8.2).

Credentials belonging to other tools are **read and never written** (§8.4).
Refresh-token families rotate, and sharing one means two clients writing over
each other's stored grant. A superseded token was measured still redeeming
shortly after rotation, so this is about ownership rather than immediate
breakage — but a client that does not hold the current token is one refresh
away from holding nothing. So the tool that owns a grant is the only one that
may rotate it, and this side spends what it finds.

The expiry is a claim inside the access token, and is read from there. That is
the figure the backend validates against, so it is the one that decides.
Nothing verifies the signature, and nothing should: the token arrived over TLS
from the server that issued it, and the proxy is reading its own credentials to
learn when they lapse — not deciding whether to trust them.

Where the claim cannot be read the token counts as expired, because a turn
refused early costs one message while a turn started on a dead token fails
mid-request.

The account id is likewise a claim, read from the id token and sent upstream as
a header.

Nothing here refreshes, so there is no single-flight to arrange and no refused
token to retire. A grant at or past its expiry is refused for the turn, and the
next turn reads the profile again — which is how a refresh performed by the
owning program arrives without anything on this side noticing (§8.4).

Credentials sit behind a `CredentialStore` trait. Keys are kept in a file
created `0600`; grants are read through the same trait from wherever the owning
program keeps them, which on macOS is a keychain item. Credentials never appear
in process arguments, logs, or the configuration file.

### 8.1 More than one account

One store holds several grants, and one of them is **selected**: the account
every turn is made as, and the only one `CredentialStore` reports. A caller that
authenticates a request sees one grant and needs to know nothing else. Which
grants exist and which is selected is the `AccountStore` half.

An account is **identified** by the account id its grant carries, and *named*
by an operator's label where one was given, otherwise by that same id,
otherwise by an assigned `account-N`. The name can be changed afterwards
without touching the grant: an id is what the backend calls the account and a
name is what the operator calls it, and correcting the second is not a reason
to spend an authorization on the first. A name another account already holds is
refused, because two entries answering to one name would hand the turns to
whichever was found first. The two are different questions: a login
carrying a label for an account already stored renames that account, and a
login carrying none keeps the name it is already filed under. Neither adds a
second entry for it. Nothing in a
grant other than the account id is an account id, so a store with neither label
nor id assigns a name rather than deriving one from a token — a name taken from
a token would be a fabricated fact about the account, and a secret in a field
meant to be printed.

A label that already names a *different* account is refused rather than
honoured. Taking it would write the new grant over the one holding that name,
which is the silent retirement this split exists to prevent. The refusal costs
the authorization just spent, and one more login replaces it; the other way
costs a grant that may not be replaceable.

**A login adds; a refresh saves.** They are different verbs on purpose. An
authorization produces a new grant, and writing it over whichever account
happened to be selected would retire a working one with nothing said. A refresh
writes the grant of the account it read, and appending there would leave two
entries sharing one refresh-token family. Authorizing an account that is already
stored replaces that account's grant rather than adding a second entry for it,
for the same reason.

Both resolve the entry by account id rather than by the selection at the moment
of writing. A refresh is a read, a network round trip, and a write; the
selection can move in between, and a write aimed at whatever is selected *then*
drops one account's rotated grant into another's entry — destroying a refresh
token that only a re-login replaces, and leaving that account authenticating as
somebody else. Only a grant carrying no account id falls back to the selection.

The file is **replaced, never truncated in place**: the new content is written
beside it and moved over it, under a name carrying the writing process's id so
two writers cannot interleave into a file that is neither. One account's
rotated token is not worth risking every account to a write that stops halfway.

Every write is taken **under a lock the filesystem enforces**, held for as long
as it takes to read the file, change it, and replace it. Every write rewrites
the whole file, so two overlapping writers would otherwise mean one discarding
whatever the other had just done — a whole account, not a stale token — and the
pair that overlaps in practice is real: `login` in the CLI writes this file
directly while the daemon may be persisting a refresh.

The lock is a file of its own beside the credentials, never read and never
written. It cannot be the credential file, because a write replaces that one by
rename, and a lock held on it would be a lock on an inode the next writer never
opens. It is advisory and the kernel drops it when the descriptor closes, so a
process that dies partway through a write leaves nothing behind for the next one
to wait on, and it stays on disk when the credentials are cleared, because
removing it would leave the next two writers locking two different files.

A filesystem that cannot take the lock **cannot hold credentials**, and the
write says so rather than proceeding without one. Locking is not universal — a
home on a network mount is the case that exists — and the alternative is a write
that reports success while doing exactly what this rule was written to stop.
The failure names the file and names the move: `PROXENOS_HOME` points the
whole directory somewhere local. Falling back to an unlocked write is
deliberately not offered; if it ever is, it belongs behind something the
operator chose, not behind a log line nobody reads.

A write that finds the file changed since it read still **starts over** rather
than replacing it. The lock reaches only writers that take it, and an older
binary or a hand edit takes none; the comparison is what catches those. It
cannot close the window on its own — the comparison and the replacement are two
operations, and a writer landing between them is lost — which is what the lock
is for.

**Accounts do not interfere with each other.** Each holds its own
refresh-token family, so rotating one leaves every other exactly where it was.
This is a property of separate grants, not a measured property of rotation: a
superseded token was once observed still redeeming and later refused as
`refresh_token_reused`, so nothing here depends on a superseded token staying
usable. What must be kept out of the design is the other arrangement — two
holders of *one* account — because there the last refresh retires the token
every other holder is still carrying.

**A refusal is about one refresh token**, and is held as that token rather than
as a flag. Held as a flag it becomes a fact about the process: the message a
refusal produces tells the operator to log in again, they do, and the grant they
produce lands in this store — where a flag would go on refusing it until the
daemon restarted, and a login through the CLI never reaches the daemon at all.
Held as the token, a grant that is not the refused one is tried and the refused
one is still never retried, whether it comes back by a switch or by a login.

A quota belongs to the account that earned it, and is held under that account's
name — §8.3.

Clearing forgets one account and leaves the rest usable, selecting another to
serve turns; clearing the last one removes the file, so "not authenticated" is
still read from its absence. Clearing what is already gone is not an error.

A credential file written before the store held more than one account is a bare
grant, and is read as the single account it describes, named by its account id.
It migrates on the next write rather than on read: reading credentials is not a
reason to rewrite them. A `selected` naming an account that is not stored falls
back to the first one, because the file still holds usable grants and answering
"not authenticated" there sends an operator to re-authorize for nothing.

### 8.2 A credential that is not a subscription

An account holds one of two kinds. A **grant** is the OAuth credential above:
refreshed, expiring, carrying an account id and a plan. A **key** is one
secret. It has no refresh, no expiry, no account id and no plan, and nothing
reports a plausible value in place of any of them — an invented expiry would
drive a refresh that cannot happen, and an invented account id would put a
header on the wire the endpoint taking a key never asked for.

Every account verb works on either: list, select, rename, forget. What differs
is where the credential may be spent, and that difference is the point.

**One place resolves a credential into headers**, and every path that
authenticates asks it: both transports, the catalog fetch, and the quota fetch.
A grant answers with its bearer token, the `originator` identifying a
subscription client, and the account id it is spending. A key answers with its
bearer token and nothing else. The `originator` moved here from the transports
for that reason: it belongs to the subscription dialect rather than to every
request. A transport holding no credential at all still sends it, which is what
the replay paths have always seen.

**A credential is refused against the other kind's endpoint**, before anything
is sent, in a message naming both halves. The model list is paired
structurally rather than checked: the daemon holds one endpoint per kind and
picks by the credential it is about to spend, so there is no argument that
could cross them. The endpoint would otherwise answer
with something about an invalid token, which sends whoever reads it looking for
the wrong problem.

**Whose account is asked before which kind it holds.** Every endpoint these
transports are pointed at is the first provider's, and borrowing made an
account on the second provider selectable (§8.4) — so a Claude grant or key
passes the kind check, is spent at a backend that has never heard of it, and
comes back refused in words naming neither the account nor the endpoint. The
relay path asks the same question of its own credential (§9), from the other
side.

Three things follow from that pairing rather than being decided separately. A
key request is **never compressed**: zstd on a request body is measured against
the subscription backend (§4.4) and nowhere else, and an endpoint that does not
decompress it parses the bytes as JSON and rejects them — observed live as a
unicode decode error naming neither compression nor the endpoint. A key
account uses **HTTP only**: the WebSocket protocol here belongs to the
subscription backend, and nothing has been observed about a key endpoint
speaking it, so there is no socket to fall back from. And a key account has
**no quota to report**: the figure is a subscription entitlement, so asking for
one with a key is the same refusal rather than a request spent to be told so.

**That absence is not the same absence as the others, and is not reported as
one.** Every other reason an account has no figure is a figure pending — make a
turn, or wait for a provider to answer. A key's is permanent, and it is
permanent *because there is no ceiling*: a key is metered per token, so the row
with no percentage is the row whose spend nothing bounds. Reported as "no
subscription quota" alone, the only account that bills for every token is the
one rendered as having nothing to watch, beside a subscription showing a number.
So the row states the absence of a ceiling rather than the absence of a figure,
and carries the one quantity that can honestly stand beside it: the tokens this
daemon has served as the account (§6.1), as a count, with no cost stated or
estimated.

**On the second provider a key is two credentials wearing one word.** A
subscription setup token and an API key are both stored as `key`, and they are
metered in opposite ways: the token draws down an entitlement whose figure rides
the response headers of every relayed turn, and the API key has no ceiling and
is metered per token. One sentence cannot be true of both — the token's figure
is genuinely one turn away, and the key's never arrives — so the store records
**which of the two a key is**, at the moment it is handed over, from the stem
its shape carries. What is persisted is a classification and never any part of
the secret (§8). Three rows follow, and each says only what is known: the
setup token reports a figure pending; the API key reports the absence of a
ceiling with the served-token count beside it, exactly as the paragraph above
states for the other provider's key; and a key that is **neither** — one whose
shape matched no stem, or one stored before the field existed — reports that
this daemon has not recorded which meter it is on. A prefix is evidence and not
proof, so an unrecognized shape is filed as neither rather than as the likelier
one, and nothing re-reads a stored secret to classify it after the fact.

**The stem that says "setup token" is worn by two credentials, and nothing here
separates them.** `claude setup-token` mints one valid about a year. The
harness's own OAuth *access* token — the one in its `Claude Code-credentials`
keychain entry — begins with the same `sk-ant-oat` stem and is valid for hours,
and it is the credential an operator is likeliest to have on hand. Classified,
stored, rendered and relayed, the two are indistinguishable: both file as a
subscription token, both are presented as a bearer, both report a figure
pending. For the second, all of that is true only until its `expiresAt`, after
which every turn it carries is refused and no field in the store says why —
the proxy holds no refresh for a key at all (`roadmap.md` §L), which is the
same property that made the year-long token adequate in the first place. (The
two also differ in scope: the keychain token carries `user:profile` and a setup
token does not, which is why `/api/oauth/usage` answers one and refuses the
other — already rendered, named here only as another place they are not
interchangeable.)

**The stem stands anyway, and the ambiguity is spoken instead.** A bare bearer
carries no structure to read without decoding it, and decoding a credential to
classify it is a new way for a secret to reach a log — a worse trade than the
one being fixed. So classification is unchanged, and `login --key` for an
anthropic key beginning `sk-ant-oat` **names both credentials on stderr where
stdin is a terminal**: the one moment a person is present to be told what a
short-lived paste will do. It says the stem, never any part of the key. Where
stdin is a pipe it says nothing, on either stream, because a scripted login's
output is read by something (§ the `--key` contract in `api.md`).

The caution is what remains of a distinction that used to matter more. The
guided `--setup-token` flow that stored one of those two credentials is gone:
what it existed to provide is now borrowed from the profile that holds it
(§8.4), with a refresh behind it rather than a token that stops working with
nothing said.

A fourth follows from the catalog rather than the pairing. The key endpoint's
model list is real and authoritative — it answers with every model the key can
reach — and it carries no window and no supported efforts for any of them. So
for a key account the window guard (§7.2) never fires and the model half of the
effort cap (§2.7) has nothing to cap against; the operator's configured ceiling
still applies, because that one is not derived from the catalog. Nothing is
silently wrong: the list is the account's own, and every entry states no window
rather than a guessed one.

A login through the CLI **hands over to a running daemon**, because the daemon
reads the file on every request but nothing else about a switch happens on its
own: the conversations bound to the previous account keep the endpoint they
dialed, and after a change of kind that endpoint refuses every turn they carry.
No daemon running is the ordinary case for a login and not a failure of one. A
credential file edited by hand still gets none of that.

A key is stored from **stdin**, never from an argument, and under a name the
operator gives — a command line is visible to every process on the machine and
lands in shell history, and a key carries no id to be named by.

Reading stdin from a terminal **says what it is waiting for before it waits**.
An unprompted read is indistinguishable from a hang, and the only hint that
ctrl-d is wanted arrives after an empty read — once the operator has already
guessed. The prompt goes to stderr and only where a person is typing, so a piped
key is byte-for-byte what it was.

Neither kind can be stored over the other. A key written where a grant is
would retire that grant with nothing said, and a rotation whose account is no
longer stored is refused rather than appended — appending it would create an
account nobody asked for and make it the one serving turns, from a background
refresh.

An account with no kind recorded is a grant, the same read-the-old-shape rule
§8.1 applies to the file as a whole.

### 8.3 A quota belongs to one account

**One figure per account, not one per daemon.** Two accounts can serve one
session: a pinned tier's turns spend the account it names (§7.1) while every
other tier spends the serving one. A single latest snapshot reports whichever
account made the most recent turn as though it answered the question that was
asked, so a cheap-tier turn on a pinned account overwrites the headroom of the
subscription the operator is actually watching.

**A figure is filed under the account that served the turn it rode in on** —
the pinned account where the tier pinned one, otherwise the serving account,
resolved to its name at that moment rather than left to be resolved by whoever
asks later. Both survive side by side: a turn on a pinned tier records under the
pin and leaves the serving account's own figure exactly where it was.

**Freshness is stated per account.** A figure that rode a turn and a figure that
was asked for over the socket are both legitimate and differently stale, so each
carries how it was come by and the moment it was taken. Neither is corrected,
recomputed, or aged into an estimate: what the provider said is what is
reported. That stays true now that any account can be asked for one: asking
records under the account it asked as, marked as asked for, and leaves every
other account's figure and freshness exactly where they were.

**A figure can be asked for per account, not only for the one serving.** Riding a
turn is the free path, and it can only ever fill in the account that made the
turn — so an account kept as a spare has one route to a figure, which is to
become the serving account and make a turn. That is the question answering
itself: the spare's headroom is what decides whether to switch to it. Asking a
provider for a quota is not making a turn, the credential is already stored, and
the backend answers for whichever credential asks. So `usage.refresh` asks once
per account, each on that account's own credential, and nothing about which
account serves turns is read or changed — authorization by name reads one
account by name (§7.1) and the selection is untouched either side of the sweep.

**Only where a figure is possible, and each failure is one account's.** A key
holds no subscription entitlement, and a credential of the provider that states
quota only on relayed turns has no endpoint that would answer (§9.4); neither is
asked, and both keep the sentence they already had rather than gaining a failed
request. Where an account is asked and the answer does not come, that is said on
that account's own row and blanks, delays, or stands in for no other's. Nothing
is invented for a row that could not be asked.

**Asking does not refresh a grant the operator did not select.** An expired
access token would have to be renewed before it could ask, and a refresh rotates
a token family: a second holder of the same grant is left holding a token
retired by a sweep it never asked for. So a spare with an expired grant reports
that, rather than being renewed in the background. The serving account is the
exception, because every turn it makes already refreshes it, and asking as it
changes nothing that was not happening anyway.

**The daemon-wide line answers for the account being asked about.** Where a
single account is held, nothing is repeated under its own name, so that line is
the whole answer — and a generic "no turn has been made yet" under a lone key
account promises a figure that will never arrive. The reason given is the
serving account's own, by the same rules as its row.

**Absent stays absent.** A window a provider did not report is omitted rather
than rendered as zero used, and an account with no figure at all reports that
it has none — as a statement about this daemon's record (§6.1), because a turn
made outside the daemon is invisible here and reporting it as unspent would be
wrong about the account. That covers every account of the second provider, whose quota
endpoint is an open question (`roadmap.md` §L) — until it is answered, those
accounts report unavailable rather than a plausible figure.

**A window carries the provider's own words, not only its number.** Where the
provider states a per-window status, the threshold it set for that status, or
which window it considers representative of the account, each is parsed and
reported. None is inferred from the percentage. An account can sit at 93% on a
window the provider has already flagged `allowed_warning` past a threshold it
published, on a turn that still went through — `limit_reached` stays false,
because only an outright refusal is the limit being reached, and the warning is
carried beside the figure rather than folded into it. Where the provider names
one window as the one that decides, that window is marked: with one window near
empty and another near full in the same snapshot, an unmarked list reads
whichever line comes first, and that is the reassuring one.

**A window the provider named rather than measured is kept under its name.** An
overage window has a figure and a reset and no duration at all, so duration
cannot identify it; it is carried with the provider's own word for it. Dropping
it at parse is not the same as deciding it does not belong on a meter — the one
is silent, and the figure was in hand.

**Staleness is a property of a window, never of a snapshot.** A stored figure
outlives the window it describes: the provider resets on a schedule and this
proxy learns a new figure only when a turn is made, so after a reset with no
turn since, the last figure still describes a window that is back to zero. One
snapshot can hold a five-hour window whose reset has passed beside a seven-day
one whose has not, so marking the snapshot would be wrong in both directions at
once — hiding a seven-day figure that is still true, or passing a five-hour one
that is not. It is stated per window, against the reset epoch the provider
already gave, and a window the provider stated no reset for is never called
stale. The error this exists to prevent is the overstating one: spend shown
against an empty window sends an operator to switch accounts they did not need
to switch.

**An absence says how far this daemon can see.** "No turn has been relayed as
this account" is a claim about the world; what a daemon can say is that none
reached it. A turn relayed outside it — `doctor --live --probe relay` builds its
own store and spends the account for real — leaves no figure here, and the two
genuinely differ. And a key account's absence is stated with what it does not
cover: the figure is a subscription entitlement and a key holds none, but a key
is the one credential kind whose spend is metered per token, so an absence
stated alone is the row that most deserves a number reading as safety.

**What a select and a removal invalidate.** With every figure held under a name,
a **select** invalidates nothing that is named: each figure still describes the
account it was taken from, and reporting it as that account is right whether or
not that account is the one serving now. What a select does drop is a figure no
account could be named for, which is reported where the daemon-wide figure is
reported and would otherwise read as the newly selected account's headroom. A
**removal** drops the removed account's figure and the tally of what was served
as it, whether or not it was the one serving: both belong to an account this
daemon can no longer spend.

---

### 8.4 A grant this process does not own

A **borrowed** grant belongs to another program's profile directory: a
`CODEX_HOME` for the ChatGPT app and the `codex` CLI, a `CLAUDE_CONFIG_DIR` for
the client. That directory is the identity — point at it and the account it
holds is the account turns are spent against — so choosing which account pays
is choosing which directory to read.

**A borrowed grant is read, never written, and never refreshed.** The refresh
token in one is single-use: exchanging it rotates the stored value in place,
and the previous one is refused afterwards (`refresh_token_reused`, §8). Doing
that here would log the operator out of the program that owns the file, and the
symptom would appear over there rather than here. The owning program refreshes
on its own next turn; an expired borrowed grant is reported as expired rather
than repaired.

Every refusal names the store it read and what to do about it, and the remedy
differs by provider: one sends the operator to the ChatGPT app or `codex
login`, the other to running the client once. Naming the wrong one sends them
somewhere that cannot help.

**Codex.** One grant per `CODEX_HOME`, in `auth.json`. The file records no
expiry, so it is read from the access token's own claim, the same rule the rest
of §8 follows. `tokens.account_id` and the id token's `chatgpt_account_id`
claim carry the same value — three signed-in profiles, all three equal — and
the field is preferred because the owning program writes it deliberately. A
profile whose `auth_mode` is anything other than `chatgpt` is refused rather
than borrowed: it authenticates against a different endpoint with different
billing, and such a profile can still carry a stale `tokens` block from a
sign-in the operator has replaced, so the mode is checked before the tokens
are.

**Claude.** On macOS the grant is a keychain item, and **which item is decided
by whether `CLAUDE_CONFIG_DIR` was set at all, not by what it was set to**:
unset gives `Claude Code-credentials`, and set gives
`Claude Code-credentials-<sha256(value)[..8]>` — including when the value names
the very directory the bare name describes. The digest is taken over the
value verbatim. Three spellings of one directory produced three different
items, so nothing canonicalizes; canonicalizing would name an item the client
never writes. On Linux there is no keychain and the same JSON sits in
`.credentials.json` inside the profile directory. Windows is unchecked: nobody
has looked, and inventing a location would produce a profile that reads as
"never signed in" for a reason of our own making.

**What is measured, and where.** Everything above about the keychain — the item
names, the digest over the value verbatim, the sixteen reads per client run,
the blanked item — was observed on macOS, on signed-in profiles, and the tests
that encode it run there. The Linux layout comes from the client rather than
from a machine anyone here ran: the code path is exercised end to end against a
reader that hands it those bytes, so what is unproven is the location, not the
parsing. On any other platform the daemon **starts** and refuses at the first
profile that needs a location, naming the platform; a configuration holding
only keys neither needs one nor is refused. Refusing at startup would refuse a
valid configuration, and guessing a location would report a profile as never
signed into for a reason of our own making.

The item stores its expiries in **milliseconds** where everything else here is
in seconds, and they are truncated on the way in. Truncating can only make a
token look older than it is, which costs one refresh; the other direction costs
a turn that fails mid-request.

**A blanked item reads as a refusal.** When the client fails to refresh, it
overwrites the item with an empty access token and a zero expiry rather than
removing it. That is indistinguishable from a profile nobody signed into, and
both want the same answer, so an empty half is refused by name rather than
carried as a grant with an odd expiry.

**A lapsed grant may be asked about, and only Claude is ever asked.** What asks
is a request for a quota figure (`usage --refresh`): the caller wants the figure
that comes *after* a refresh, so it waits for one. A turn does not ask — it
refuses on a lapsed grant and says whose program renews it — because a turn that
blocked while a client started up would spend a minute before its first byte.

The one move available here is to run the program that owns the profile, wait
for it to exit, and read the profile again: the rotation happens inside that program,
which is the only process allowed to perform it. `claude -p` on the cheapest
tier is that run, with stdin closed — without which the client waits several
seconds for input that never comes — and a deadline, after which the process is
killed and the profile is left alone.

**What is run is `claude_program`, or the bare name where that is unset.** A
bare name is resolved through the *daemon's* `PATH`, and a daemon started by
launchd inherits almost none of one — so on that machine the ask fails with
`could not run \`claude\`` until the path is written out (`api.md` §4). The same
program is what the second provider's quota request reads its version from, so
one key settles both.

Codex is never run. Its grant refreshes only on a real turn, which spends the
operator's quota and rotates the refresh token, and one failing run was measured
sending fourteen refresh requests in a row; its access token also lasts ten
days, so the case barely arises.

**A profile whose refresh token has already lapsed is never asked either.**
A client that fails to refresh overwrites its own stored item with an empty
access token and a zero expiry, so asking there destroys what is left of the
grant instead of renewing it. `refreshTokenExpiresAt` says so locally, for
free, before anything is run. Where the client never recorded it, the profile
is asked anyway: unknown is not dead, and if it turns out to be dead the
operator has to sign in again either way.

**One run for a whole sweep, however many profiles it covers.** `usage
--refresh` asks about every account, and asking means starting a program and
waiting for it — so a per-account bound is no bound at all: four lapsed
profiles would be four minutes of a caller with nothing to time it out. The
budget is one client run, spent by whichever account needs it first. The rest
are asked for their figures without a refresh, and each row says it was not
asked rather than reporting a figure nobody obtained.

**One run per profile, under a lock held for its whole duration.** Ten callers
arriving at once produce one client, not ten: the rest block on the lock, and
by the time they take it the run has already written whatever it was going to
write. The lock is per profile, because two profiles refreshing at once are two
clients writing two different stores. It is released on failure as well as on
success, or the next caller would wait for a run that is not happening.

**Which profile serves turns is this side's state, and the only thing about a
borrowed account this daemon writes.** It is kept beside the other daemon state
rather than in the configuration document, for the same reason the token tally
is: `accounts --use` is a runtime verb and the document is the operator's.

**One declared profile serves without being chosen. More than one, with nothing
chosen, is refused.** There is nothing to choose between in the first case, and
in the second the choice decides whose subscription pays — resolving it to
whichever entry comes first spends the wrong one invisibly. A selection naming
an entry the operator has since deleted is refused by name for the same reason,
rather than falling through to another account.

**A declared profile nobody has signed into is still listed.** It is an entry
the operator wrote, and dropping it from the listing would read as one they
never wrote; what is unknown about it is reported absent.

**Every other write refuses, naming the profile.** Adding, renaming, forgetting
and saving are verbs of a store that owns what it holds, and this one does not.
The refusal says what does: the program that owns the profile changes what is
inside it, and the configuration file is where this daemon's view of it is
edited.

**A borrowed Anthropic grant has a quota endpoint; the credential it replaced
did not.** `GET /api/oauth/usage` answers 200 for one, carrying `five_hour` and
`seven_day` utilisation, a `limits` array with the provider's own severity per
window, spend, and extra usage. A subscription token is refused there for want
of a scope, which is why quota on that provider had to be read from turn
headers alone (§8.3) — the borrowed grant is what closes that gap.

Its body is a third shape and shares nothing with the other two: the windows are
named rather than positional, the figure is already a percentage, and the reset
is an RFC 3339 timestamp instead of an epoch. Each window carries the provider's
own severity, read rather than inferred from the percentage: an account can sit
high on a window the provider is still calling normal. Nothing in that body
states that a turn would be refused, so nothing derived from it claims one would.

**A credential names whose endpoints it belongs to, as well as which of that
provider's two it reaches.** Both are needed: a borrowed subscription grant on
the second provider is a subscription credential that must never be sent to the
first provider's backend, and the relay asks about the provider rather than the
kind — a grant and a key on that provider are spent at the same endpoint.

Each provider's subscription path is addressed as its own client. The first
wants an originator and the account id on every request; the second wants the
beta header its OAuth grants are gated behind, and is asked for quota under the
owning client's user-agent string rather than this proxy's. Sending either
provider's extras to the other is how a borrowed grant fails with a message
about the wrong half.

**Who pays is said on every surface that has a place to say it.** A borrowed
account is a directory the operator signed into somewhere else, so the name in
the configuration file is their own label and nothing about it is the account.
Each listing therefore carries the store it was read from, and the identity the
credential itself holds travels beside the label: the status line receives the
serving account whether or not a quota figure has arrived, a launch prints it
once before the client starts, and `accounts` names the profile behind each row.

**A profile that has become a different account is marked.** The identity is
recorded at the moment the profile is chosen, and a later read that finds a
different one says so on the row that serves turns and in the launch line. This
cannot happen to a credential a daemon holds itself, and it is the one failure
borrowing introduces: the operator signs into the owning program as somebody
else, the directory keeps its name, and every turn afterwards is billed to an
account nobody pointed at them. A profile that cannot be read is never marked —
it has not changed identity, it has not been read.

**A second profile is signed in by running the program that will own it.**
`login --profile` creates a directory, runs that program's own login against it
with the variable that names it, and afterwards reads the profile to find out
whether there is anything to declare. It is the rule the rest of this section
follows, applied one step earlier: the client authenticates, the client writes,
and this side learns the result by reading. Nothing here sees a token, and a
directory that holds no grant is declared nowhere.

Declaring the first profile writes down the ones already being read. A written
entry stops the daemon looking for the stock profiles, so a first
`login --profile` on a machine that had been discovering them would otherwise
take away every account the operator already had — the verb adds one, and must
not subtract two. Only the discovered profiles that hold a grant are written:
an entry for a program that was never signed into is an account that cannot
serve.

Two cases fall out of that shape rather than being features of their own. A
directory that is already signed in is adopted without running anything, which
is how a profile made elsewhere is taken on. And where there is no terminal to
answer the login's prompts, the command is printed with its variable already
attached instead of being started somewhere it can only hang.

**With nothing declared, the stock profile of each program is read.** A first
run should not make an operator write down what the programs on the machine
already know: `[profiles]` empty means look at the profile each client uses
with no variable set, and whichever holds a grant is an account. One signed-in
client is therefore one account, which serves without being chosen; two are two,
and the existing rule applies — neither serves until one is chosen, because
that choice decides whose subscription pays.

Discovered and declared never mix. Writing any entry replaces the found set
entirely: an entry is the operator's own statement about which identity pays,
and a discovered profile sitting beside it would be a second opinion nobody
asked for. A discovered profile holding no grant is not listed either — nobody
asked for it, and reporting that a program was never signed into on a machine
that does not have it answers a question nobody put. A declared one is always
listed, whatever state it is in, because a row that vanished reads as an entry
the operator never wrote. The listing says which of the two sets it is showing.

**A credential the backend refuses is remembered against the account that
spent it.** On the second provider it is the only thing that can say a profile
needs signing in again: `auth.json` records no date to count down to, and
`codex login status` does not supply one either — measured, it reads the file
and reports "logged in" for a profile whose tokens are junk. So the answer
comes from the backend, on a turn nobody is watching, and is kept where
somebody can read it: a status, the sentence the backend wrote, and when.

Three things follow. It is filed under the account that was serving at the
moment of the turn rather than whichever is selected when somebody asks. It is
cleared by the next turn that works, because signing in again is what fixes one
and a warning outliving the problem sends an operator to renew what already
works. And only what the *backend* said counts: a lapsed grant this side
refuses before sending anything wears the same error kind, and reporting it
here would tell an operator to sign in over a profile this daemon simply could
not read.

**A login that has to be renewed is said so before it lapses, on the week it
matters.** A Claude profile's stored item records `refreshTokenExpiresAt`,
which is the date its own client counts down to ("your login expires in 3
days"), and it is read here already. Within seven days `accounts` puts the
count on the row and `status` adds the remedy; outside that window neither says
anything, because a date carried eleven months of the year is one the reader
learns to skip.

The notice exists because of what the date means here rather than as a
convenience. Past it the client cannot refresh the profile either, and asking
it to try blanks what is left of the stored grant — so without the notice the
first sign is a grant that emptied itself. A Codex profile has no equivalent
field: `last_refresh` and an access-token expiry say when it was last renewed,
not when renewing stops working. Nothing is stated there, and nothing is
guessed.

**A grant left in this daemon's own store is not read, and is said to be not
read.** A credential file written by a version that obtained its own grants
still holds them. Nothing here obtains or refreshes one now, so such an entry is
skipped rather than offered as an account that cannot be spent — and named in
the listing, because a credential that quietly stopped counting reads as one
that vanished.

**The keychain is read by spawning `security`.** The item's ACL trusts that
binary; a process reading through Security.framework is a different application
to the keychain and is prompted. One client run reads the item sixteen times,
so a prompting read is not a nuisance but an unusable daemon.

## 9. The second provider

A provider that speaks the surface this proxy already exposes needs no
translation at all. A turn belonging to one is **relayed**: forwarded as it
arrived, and streamed back as it returns.

This section is **confirmed live**: relayed turns round-trip against the real
endpoint of the second provider — plain and streaming, generation and refusal
— with a subscription bearer the relay substituted. The endpoint wants the
client's own identity shape (its beta list, `x-app`, its system prompt), which
the client always sends and this path forwards verbatim, so no header is added
or invented; a bare request stripped of that shape is refused upstream, which
is that provider's decision to make, not this proxy's to pre-empt. The rows in
`roadmap.md` §L record what settling this falsified along the way.

**The body is relayed verbatim.** Not observed as a property — stated as a
rule, because the obvious implementation breaks it quietly. Parsing the request
and writing it out again would round-trip it through this proxy's own types,
and every field they do not model would go missing somewhere no test looks. So
the routing decision is made from the raw bytes, before anything is parsed, and
the bytes that arrive are the bytes that leave.

The `relay` probe (§10.3) holds that rule. Its marker sits inside a field this
proxy has no type for, so a body round-tripped through those types fails it,
and fails it on the one thing a re-encoded body still gets right: the turn
succeeds and the answer reads correct. It runs on both modes. Replayed, a
stand-in backend records what it was sent and both halves are checked. Against
the real endpoint only the answer half is: forwarding is the whole behaviour of
this path, so the outbound bytes leave on a socket this process cannot read, and
the row names that rather than passing over a value nothing looked at. The turn
is authorized as a named account on this provider, read from the store, so
nothing about which account serves turns is read or changed.

Everything §2 does on the translating path is therefore absent here: no
instructions lead, no tier name rewritten into a model id, no tool flattening,
no effort cap, no window guard. So is §3: no baseline, no delta, no previous
response id. This path holds no per-conversation state, because it has nothing
to hold — the client sends the whole conversation every turn and the backend
reads it.

### 9.1 Routing

Each stored account states which provider its credential is spent against (§8),
and a turn routes by **model id**: the id in the body is looked up among the
mapped models, and the account that mapping names decides the path. A mapping
that pins one (§7.1) names it; a mapping that pins nobody names the account
serving turns, which is what an unpinned tier has always meant. An id whose
account is on the second provider is relayed. An id no mapping names follows
the account that would authenticate it: relayed when that account is on the
second provider, translated as before when it is on the first.

The unpinned case matters on its own. An operator who stores a key for this
provider and selects it has said where their turns go, and routing that ignored
the selection would send every one of them to the other provider's endpoint —
refused there as a credential of the wrong kind, which is a message about the
credential rather than about the half that is wrong.

**A turn never authenticates against a backend its account was not stored
for.** Translation spends the authenticating account's credential against the
first provider's backend, so an id the mapping does not name, arriving while
an account on the second provider would authenticate it, is relayed to that
account instead. The credential travels only to its own provider's endpoint,
and that provider judges the id — the only authoritative answer to whether it
is served. A launch-time model override rides on this, symmetrically: an
unmapped id goes to the account serving turns on either provider, passed
through to the translating backend or relayed to the second, with no mapping
edit. Crossing providers still takes a pointer — a pinned tier (§7.1) or a
changed selection — because serving an id from an account nobody named would
spend a subscription nobody pointed at the turn.

By model id rather than by tier name, because this path never rewrites the
model. The client is handed final ids by `env` and `exec` at launch and sends
them for the session's life, so what arrives in the body is already what the
backend must see.

**One model id may be claimed by at most one account.** Two mappings naming one
id and two different accounts leave nothing to say which account a turn belongs
to, and that is refused — naming the id and both claimants — rather than
resolved by picking one. Picking spends a subscription nobody pointed at the
turn and says nothing about having done so.

The refusal is scoped to ids this path claims. Two tiers of the first provider
sharing an upstream model is ordinary, decides nothing, and stays what it has
always been.

A pinned account the store does not hold is *not* refused here. It falls
through to the translating path, where §7.1 already refuses it by name — one
mistake with one message rather than two.

**A relayed tier is not validated against the first provider's catalog.** That
list is one account's menu (§7.0), and one provider's: an id on this path is
absent from it by construction, so measuring it there refuses a correct mapping
with a message naming a menu the id was never offered on. The exclusion holds at
every place a mapping meets the catalog — the daemon's start, `tiers.set`, and a
switch — beside §7.1's exclusion of a pinned tier, which is the neighbouring
case with the neighbouring reason. The same tier is left out of the
withheld-model report (`api.md` §3), which otherwise answers a question that list
cannot speak to.

### 9.2 Headers

The header set is the only thing that changes between ingress and egress. The
request path's query string follows the body's rule, not this one: it is
forwarded exactly as sent — `?beta=true` is observed live — and never
invented where the client sent none.

- **`authorization` is replaced** with the account's credential. The client's
  own bearer is a placeholder: `ANTHROPIC_AUTH_TOKEN` has to be set for the
  client's sake and its value is ignored (§8).
- **`x-api-key` is dropped.** No observed client sends one, and a turn
  authenticated as whatever the caller happened to hold is a turn this proxy
  did not route.
- **Everything else passes through as the client sent it** — `anthropic-version`,
  `anthropic-beta`, and the client's own identifying headers included. The beta
  list is the client's statement about what it can parse in the reply, so
  editing it would change what comes back.
- **Hop-by-hop and length headers are not forwarded.** They describe this hop
  rather than the message, and the length and transfer coding belong to
  whichever HTTP client writes the request. `accept-encoding` is dropped for
  the same reason: this path does not decode a content coding, so asking for
  one would leave it relaying bytes the client never agreed to.

What the real endpoint accepts is not settled here. The client's half of the
delta is recorded — `roadmap.md` §L — and the endpoint's half is open.

### 9.3 Errors

An upstream refusal on this path **is already an Anthropic error**. Its status
and its body pass through as they arrive. Rewrapping would restate a message
the backend wrote, and a rewrap that loses the error type takes the client's
own retry logic with it (§1.1).

A refusal this proxy makes — an ambiguous model id, an account that cannot be
read, a credential of the wrong kind — is its own, in the same shape everything
else in §1.1 uses. A connection that never opened is retryable, because nothing
was sent.

### 9.4 What a relayed turn leaves behind

The payload is untouched on this path; the record of it is not optional. Two
things a translating turn leaves behind, a relayed one leaves in the same place.

**Ingress capture records a relayed turn** (`api.md`, `record`), and what it
records is **the bytes that were relayed**. The rest of this section is a rule
about not re-encoding the body, and a capture rebuilt from this proxy's own
types would break it in the one place the breakage is hardest to see: a fixture
that is not what the client sent still replays, still passes, and is wrong about
every field those types do not model. The headers go through the same redaction
by name as everywhere else — the name is the datum, the value is a secret in a
file that is not the credential store.

Capture is never allowed to change the turn. A body that cannot be held as raw
JSON is not captured and the turn goes anyway, the same as a capture that cannot
be written (`api.md`).

**The model id joins the served list** the quota answer states (`api.md` §2). The
mapping alone cannot stand in for it. A client is handed final ids by `env` and
`exec` at launch and sends them for the session's life, so an operator who
remaps a tier mid-run leaves the mapping naming an id no running session sends —
and a status line reading the mapping would stop recognizing the session it is
painting. What a turn was actually made against is the durable record, and it is
kept here for the same reason §7.1's path keeps one.

**The response's quota headers become this account's figure.** The second
provider states rate-limit headroom in `anthropic-ratelimit-unified-*` response
headers on every turn, and for a subscription credential that is the *only*
place it states one: its usage endpoint refuses that credential for want of a
scope, so the `usage.refresh` path of §8.3 has nothing to ask for these
accounts and does not ask. Reading the
headers costs nothing — the figure rides a turn already being made — and it is
filed under the account that made the turn, never under whichever account is
selected when someone later asks.

The names read here are the names §9.2 emits on the other path: what this proxy
hands its own client when it translates is what the provider hands this proxy
when it relays. Utilization is a fraction on the wire and a percentage in a
snapshot; that conversion is the only arithmetic, and nothing else is derived.
The plan name is **absent**, because no header states one and an account's plan
is not deducible from its headroom. `allowed_warning` is a turn that went
through: only an outright `rejected` is the limit being reached, and reading the
warning as a refusal would show a limit the account has not hit. A response
carrying no quota header at all yields no snapshot rather than an empty one — an
empty snapshot reads as "quota known, nothing used", which is the reassuring
direction to be wrong in.

---

## 10. Testing

Development is test-first.

### 10.1 Translation

Every rule is a pure function over data and is specified by a failing test before
it is implemented. Table-driven cases cover mappings; snapshots cover emitted
frame sequences.

### 10.2 Upstream contract

What the backend sends cannot be invented. Ground truth is captured first, becomes
a fixture, and the fixture becomes the failing test. This is still test-first —
the test's content comes from observation rather than imagination.

### 10.3 Capabilities

A capability test must turn on content the model could not infer — random codes,
verbatim strings. A model handed nothing at all describes a file confidently from
its name, and that output is indistinguishable from success. Plausibility is never
evidence.

The matrix these produce says what it did not touch as well as what it did. A
failed row prints the probe's rationale; a live run marks the `count-tokens`
row, whose surface never reaches the backend the live header speaks for; and one
line names the account the run spent, the account the relay arm spent when it
ran, and the paths the run left alone — the WebSocket transport always, and
either of the other two paths where no probe on it passed. Green rows say nothing
about a path nothing drove, and a reader with no line to tell them otherwise
reads green as coverage of the whole proxy.

That line is assembled from the outcomes, and a path has three states rather
than two. A path with a passing row was exercised, and only then is the account
it spent named. A path whose probes all ran and all failed was reached and
established nothing, which the line says in those terms — reporting it as
unexercised would hide that the path was reached. A path nothing ran on, or
whose every row was skipped, was not exercised and names no account.

Each state is a heading, each path appears under exactly one of them, and a
heading with nothing under it is not printed: a run that exercised nothing must
not open with a bare claim of what it exercised. The `Not exercised:` heading is
always printed, because the WebSocket transport is always under it. Overstating
and understating are the same defect here, so a run that did drive the
translation path still says so plainly.

### 10.4 Transport and sessions

Transport tests run against a local server replaying recorded exchanges.
WebSocket coverage includes reuse, prewarm, fallback latching, and cancellation.

Incremental upload is specified by its invariants:

- a valid delta contains exactly the new items
- any change to a non-input field forces a full send
- a non-extending input forces a full send
- server-returned items are never resent
- a full send is always valid
