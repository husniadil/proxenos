# Proxy behavior

Normative specification for how proxenos translates between the Anthropic
Messages API and the OpenAI Responses API, and how it manages sessions,
transports, credentials, and token accounting.

This is the definition the code is measured against. [`api.md`](api.md) is the
companion contract for what the proxy *exposes*.

Most rules here exist because the obvious implementation is wrong in a way that
does not fail loudly. Each rule is stated first, short, under its own heading.
**Why** gives the reason. **Tried and dropped** appears only where a rejected
alternative still explains the rule. Every top-level section ends with the
files that implement it.

Numbered sections are cited from code and from other documents. Their numbers
and subjects are fixed; unnumbered headings beneath them are free to move.

---

## 1. Premise

Claude Code is not an ordinary Messages API client. Several of its built-in
tools depend on behaviour the server provides, and a translator that handles
only messages and function calls leaves those tools broken while every request
still returns 200.

| Path | Server dependency | Failure when unhandled |
|---|---|---|
| `Read` (image, PDF) | attachment blocks nested inside `tool_result` | bytes never arrive; the model describes the file from its name |
| `WebSearch` | a server-side search tool declared in a secondary conversation | search returns nothing, reported as "no results" |
| `WebFetch` | a model call on the haiku tier | fails in a way that looks unrelated to tier mapping |
| tool search | `defer_loading` stubs and `tool_reference` discovery | discovered tools stay uncallable, or every stub inflates context |
| context meter | `input_tokens` in `message_start` | the meter collapses to zero each turn |
| `count_tokens` | pre-flight sizing | absent or wrong |

Preserving these is the product. Everything else in this document serves that.

`WebFetch` and `WebSearch` both run on the haiku tier: with haiku mapped to a
distinguishable model, the client reported both against it while main turns
used the sonnet tier's model. An unmapped or unservable haiku tier breaks both.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/ingress.rs` | The Messages surface: `/v1/messages`, `/v1/messages/count_tokens`, routing between translate and relay |
| `crates/core/src/translate/` | The pure translation layer this document mostly specifies |
| `crates/proxy/src/probe.rs` | The capability probes that check each row above (§10.3) |

---

## 2. Request translation

### 2.1 Instructions

#### The system prompt maps to `instructions`, never to `input`

The top-level `system` field becomes the Responses `instructions` field.

##### Why

The backend rejects system-role and developer-role messages inside `input`
(`400 System messages are not allowed`, observed verbatim).

#### A message with any other role is carried as a `user` item

A conversation message whose role is neither `user` nor `assistant` becomes a
`user` input item. It is never folded into `instructions`.

##### Why

The backend rejects the role, not the content. The client attaches per-turn
content this way, a billing header among it, so folding it into `instructions`
would change that field on every turn. That breaks two things at once: a delta
requires every non-input field to be unchanged (§4.3), and the prompt cache buys
nothing when the cached prefix differs each time.

##### Tried and dropped

Folding such messages into `instructions`. Measured against a real agent loop:
three turns, no deltas at all. Carried as input items, the same loop uploads
only what is new.

#### Nothing that varies per turn belongs in `instructions`

`instructions` is built from four parts, in this order, empty parts skipped and
the rest joined by a blank line:

1. the **lead**: one line naming the model actually answering
   (`[instructions] identity`, on by default);
2. the client's system prompt;
3. the **working budget** (`[instructions] working_budget`, on by default);
4. the operator's **trailer** (`[instructions] append`).

All four are constant for the life of a conversation.

##### Why

A lead carrying a timestamp or a token count would change `instructions` every
turn and cost the whole incremental path, the same failure as above arriving
through the door built to prevent it.

#### The lead precedes the client's prompt

##### Why

The prompt the client sends is written for a different model and opens by
saying so. Nothing else in the request tells the model what it is, and nothing
in the client can be made to: its append-system-prompt flag reaches the same
`system` field, so it can add to that prompt but never precede it. An identity
stated *after* a prompt that already asserted a different one reads as a
correction rather than a fact.

#### The working budget sits after the prompt and before the trailer

The budget asks for the smallest slice that answers the question (a targeted
search or a bounded line range rather than a whole file) and for acting once a
read is sufficient. It is written as decision rules, with no *always*, *never*,
or *must*.

##### Why

The conversation is replayed upstream on every turn (§4.3) and echoed back
three times per turn (§4.4), so context pulled in is paid for repeatedly.
Without a budget the model reads broadly and spends the window fast.

After the client's prompt, because it exists to overrule the parts of that
prompt asking for broad reading, and an instruction placed before the one it
modifies reads as a suggestion. Before the trailer, because a shipped default
has no business outranking text an operator wrote on purpose.

Absolutes are reserved for real invariants: a shipped absolute that collides
with the client's own prompt destabilizes more than a missing detail does.

#### The trailer is last

##### Why

An instruction meant to take precedence over the prompt above it has to come
after it.

### 2.2 Content blocks

#### The block mapping

| Anthropic | Responses |
|---|---|
| `user` / `text` | `message` / `input_text` |
| `user` / `image` | `message` / `input_image` |
| `user` / `document` | `message` / `input_file` |
| `assistant` / `text` | `message` / `output_text` |
| `user` / `document` inside a `tool_result` | `message` / `input_file`, following the output (§2.3) |
| `tool_use` | `function_call`, `arguments` serialized as a JSON string |
| `tool_result` | `function_call_output` (§2.3) |
| `thinking`, `redacted_thinking` | dropped; no equivalent exists |

A message mixing prose and calls splits in order: calls and their outputs are
items in their own right, so text before a call is flushed as its own message.
A message left with no content at all (an assistant turn that carried only
thinking) produces no item.

#### Sources become URLs

Base64 image and document sources encode as data URLs. `image_url` is that URL
directly, not an object wrapping one. URL sources pass through unchanged and
are not prefetched; they resolve only if the backend can reach them. A source
of an unrecognized type is dropped.

An `input_file` is named from its media type, because nothing in a `document`
block carries the original name: `attachment.pdf` (PDF or no media type),
`attachment.txt`, `attachment.md`, and `attachment.bin` for anything else.

#### Assistant content is `output_text` only

An image or document inside an assistant message is dropped rather than
converted.

#### The document path exists, and the client does not use it

`input_file` has no counterpart in the upstream client, which has no document
representation. It is the public API's shape and the only candidate that could
carry a document.

##### Why

Claude Code rasterises a PDF and sends `image` blocks (measured), so a PDF
reaches the model through the image path. The document path serves a client
that does send `document` blocks. The backend accepts it: a document posted
directly returned a code that existed nowhere but inside the PDF. A rejected
part would be a request error, not a silently dropped file.

### 2.3 Attachments inside tool results

#### An image travels inside the tool output

`function_call_output.output` is either a bare string or a list of content
parts. An `input_image` part inside that list attaches the image to the call
that produced it, with no synthetic message between them.

#### The output is a bare string only for a single piece of text

Every other case stays a list, including the empty one. A tool result carrying
no `content` at all becomes the empty string: there are no parts to make a list
of.

#### A document follows the output as a `user` message

No document part exists inside a tool output, so each document is re-emitted in
one `user` message placed immediately after the `function_call_output`, which
keeps its text and images.

##### Why

`input_file` is defined for message content and nowhere else, so that is the
only position where it can be accepted.

This is how every file Claude Code reads arrives. Without it the bytes never
reach the model, and the model answers from the filename in hedged wording that
reads as success. The failure is invisible in ordinary output, which is why
§10.3 requires unguessable probes.

### 2.4 Tools

#### Function tools flatten

`{name, description, input_schema}` becomes `{type: "function", name,
description, strict, parameters}`. A missing schema becomes
`{"type": "object"}`, and an object schema with no `properties` key gains an
empty one.

#### `strict` is always false

##### Why

Strict mode requires every property to be required and no additional
properties. The client's tool schemas do not comply, and claiming strict over a
non-compliant schema is a request rejection, not a stricter model.

#### Unsupported `pattern`s are dropped, everywhere in the schema

Every `pattern` the backend's schema validator would refuse is removed, in the
tool schema and in every subschema below it (`properties`, `patternProperties`,
`$defs`, `definitions`, `dependentSchemas`, `items`, `prefixItems`,
`additionalItems`, `additionalProperties`, `unevaluated*`, `propertyNames`,
`contains`, `not`, `if`/`then`/`else`, `allOf`/`anyOf`/`oneOf`). The keys of
`patternProperties` are patterns too, and an unsupported key is dropped with
its subschema.

The validator's dialect is narrower than the one the client's schemas are
written against: no Unicode property escapes (`\p{Cc}`), no braced code points
(`\u{1F600}`), no control escapes (`\cA`), no `(?<name>` group, and no class
range whose endpoint is an escape. Lookahead, lookbehind, non-capturing groups,
and plain escapes it takes.

##### Why

Refusal is not partial. One unsupported pattern anywhere in one tool's schema
rejects the whole request, the client can neither see the reason nor fix it,
and the turn dies wherever that tool is declared. Dropping a pattern costs the
model a hint about one argument, and with `strict` false nothing enforced it
either way.

#### Kept patterns are decided by an allow-list

A pattern using any construct the checker does not recognize is dropped, even
where the validator would have taken it: an unrecognized escape letter, a
backreference digit, an empty class, a stray `]` or `}`, a control character,
or unbalanced parentheses.

##### Why

A false accept fails the turn; a false drop loses a hint.

#### `tool_choice` maps to three shapes

`any` → `required`; `tool` → `{type: "function", name}`; `auto`, `none`, and
anything unrecognized → `auto`.

##### Why

`none` upstream would withhold the tools, and the client sends it on turns
where the tool list still has to be visible.

### 2.5 Deferred tool loading

#### An undiscovered deferred tool is withheld

A tool marked `defer_loading: true` is not sent upstream until it has been
discovered.

#### The backend's own deferral is not used

##### Why

Discovery here is driven by the client. A second discovery path the client
cannot observe would let the model load a tool whose results never reach the
client.

#### Discovery is recorded on the session and outlives the flag

Discovery is observable exactly once: a tool-search result contains
`tool_reference` blocks, each `{"type": "tool_reference", "tool_name": ...}`.
The field is `tool_name`, as the client sends it. The names are recorded on the
session, and a recorded tool is forwarded on every later turn *even though it
continues to arrive marked `defer_loading`*.

##### Why

The client never clears the flag, so the recorded set is the only signal that a
tool is live.

#### A tool-search result names what became available

A tool-search result has no text, only `tool_reference` blocks. Its
`function_call_output` is the JSON string `{"available_tools": [...]}`.

##### Why

An empty output leaves the model unable to act on a search it just ran.

#### The client's own deferral is switched back on

The launch environment always sets `ENABLE_TOOL_SEARCH=true` (`api.md` §2.2).

##### Why

The client disables deferred loading whenever its base URL is not first-party.
Both paths carry the contract deferral needs: this section on the translating
path, and a verbatim relay to a backend that runs the search itself on the
other. Measured on both: an MCP set costing about 101k tokens up front defers
to zero, and turns succeed.

### 2.6 Web search

#### Any tool whose `type` begins with `web_search` is the native search tool

`WebSearch` runs as a secondary conversation declaring
`{type: "web_search_<version>", name: "web_search"}` with no `input_schema`. It
maps to the Responses `web_search` tool with `external_web_access: true` and
`indexed_web_access: true`.

##### Why

Translated as a function tool, it becomes a tool the model cannot execute and a
search that silently returns nothing. Both access flags are stated rather than
left to a default, because a default of false produces the same empty search.

### 2.7 Request fields

#### The fixed fields

Every request sets `stream: true`, `store: false`, `parallel_tool_calls: true`,
`reasoning.summary: auto`, and `include: ["reasoning.encrypted_content"]`.

`stream: true` is unconditional: the backend is always asked to stream,
whatever shape the caller asked to be answered with (§5.5).

#### The model is the tier's upstream id

A request naming a mapped tier is sent with that tier's upstream model id. An
id no mapping names passes through unchanged.

#### `reasoning.effort` comes from the request, under two ceilings

The inbound `output_config.effort` is the starting point. Two ceilings apply,
and the lower wins:

- the **operator's**, from configuration (`effort`, or an account's own
  `effort`, §7.1);
- the **model's**: the highest effort the catalog lists for it.

The ceiling caps and never raises. A request asking for less keeps its own
choice. With no request effort, the ceiling applies. With neither, the field is
omitted and the backend's default applies. A model whose efforts the catalog
never listed caps nothing: unknown is not a limit.

##### Why

The client cannot choose the ceiling: it does not know whose quota it is
spending, and effort is the largest lever on what a turn costs. An operator who
capped effort meant it for traffic that expresses no preference too, which is
most of it.

The client asks for a *tier*, so it cannot know the model behind it stops at
`xhigh` while another goes to `max`. Forwarding an effort the model does not
support fails the turn for a reason the client could neither anticipate nor
fix.

#### The result then snaps to an effort the model accepts

Whatever survives the ceilings moves to the highest level the model lists at or
below it, or, where it sits below everything on offer, *up* to the model's
lowest. The model's floor therefore outranks the operator's ceiling. A model
with no listed efforts snaps nothing.

##### Why

Asking for less than a model can do is a request for its cheapest setting, not
for one it would refuse. An unlisted effort fails the turn in either direction.

#### `prompt_cache_key` is sent and nothing rests on it

It carries the session's id (§3.1) and is stable for the life of a
conversation.

##### Why

Sent alone against otherwise identical repeated requests, it produced no cached
tokens in any trial, in both orders, with independent prompts per condition. It
is kept because it is harmless and is what the field is for. What the cache
actually rests on is in §2.8.

#### Unsupported inbound fields are dropped

Only fields the request types model are read. Anthropic `cache_control` has no
equivalent and is dropped; upstream caching is implicit.

#### Server-assigned ids are stripped, except on reasoning items

An item re-sent from a previous response loses its server id. A retained
reasoning item keeps the id the server gave it: it goes back as the server's
own item (§3.3). Identity comparison ignores ids either way (§3.1), so this
changes what is sent and not what matches.

`previous_response_id` is set only by the incremental path (§4.3).

### 2.8 Upstream request headers

#### The header set

| Header | Value | Transport |
|---|---|---|
| `authorization` | `Bearer <access token or key>` | both |
| `chatgpt-account-id` | the account id the grant carries (§8.4); a key sends none | both |
| `originator` | one fixed first-party originator; a key sends none (§8.2) | both |
| `user-agent` | matching that originator | both |
| `session_id` | the conversation's id | both |
| `openai-beta` | the Responses WebSocket opt-in | WebSocket upgrade only |
| `accept` | `text/event-stream` | HTTP only |
| `content-type` | `application/json` | HTTP only |
| `content-encoding` | `zstd`, where the body was compressed (§4.4) | HTTP only |

The catalog request carries the same identity headers and is refused without
them.

##### Why

The WebSocket upgrade is an HTTP request like any other. A missing `originator`
or `user-agent` there is not enforced by the socket, so its absence fails
nothing and says nothing; the upgrade therefore carries the full identity set.

#### `session_id` carries the prompt cache scope

A UUID, stable for the life of a conversation. A UUID because that is the shape
measured to work; whether an arbitrary string is accepted is unmeasured.

##### Why

Over WebSocket it changes nothing: the incremental path chains turns with
`previous_response_id` (§4.3), and that already caches. Over HTTP every turn is
a full send with no chain, and there the header is the whole difference.
Measured on one four-turn conversation: uncached input per turn fell from
4,465–4,497 tokens to 625–657, with 3,840 reported cached from the second turn
on. HTTP is a normal operating mode (§4.2), so that is a real cost, not a
hypothetical one.

#### One originator, with no alternate

A rejection at this layer surfaces as an error. Nothing retries under a
different identity.

##### Why

A fallback identity is state to track, invalidates the prompt cache when it
changes, and turns one clear failure into two unclear ones.

#### An upstream refusal keeps its body

A non-success status is mapped to the Anthropic error vocabulary: 429 →
`rate_limit_error`, 401 and 403 → `authentication_error`, 400 →
`invalid_request_error`, 5xx → `overloaded_error`, anything else → `api_error`.
The message is the response body, trimmed to 500 characters, and a
`retry-after` header is passed through. A connection that never opened is
`overloaded_error`, because nothing was sent.

##### Why

A challenge page (a non-JSON body on a 403) carries no structured error, and
the excerpt is the only diagnostic available.

### Where it lives

| File | What it holds |
|---|---|
| `crates/core/src/translate/request.rs` | `translate_request`: instructions, content blocks, tool results, tools, deferral, effort |
| `crates/core/src/translate/schema.rs` | The `pattern` allow-list and the subschema walk |
| `crates/core/src/anthropic/mod.rs` | Inbound Messages types, `tool_reference`, `web_search` detection, source URLs |
| `crates/core/src/responses.rs` | Outbound Responses types, `CallOutput::from_parts`, `Effort` |
| `crates/proxy/src/ingress.rs` | Tier resolution, the effort ceilings, instruction parts per turn |
| `crates/proxy/src/upstream/http.rs` | HTTP headers, `ORIGINATOR`, the error excerpt |
| `crates/proxy/src/upstream/websocket.rs` | Upgrade headers, `BETA_HEADER` |
| `crates/proxy/src/auth/authorize.rs` | The per-credential header set (`authorization`, `originator`, `chatgpt-account-id`) |
| `crates/proxy/src/error.rs` | `from_upstream_status`, the status-to-error-type map |
| `crates/proxy/src/control/handler.rs` | `environment_for`, which emits `ENABLE_TOOL_SEARCH` |

---

## 3. Sessions

### 3.1 Identity

#### A request belongs to a session when its input strictly extends the baseline

Claude Code sends no session identifier, so identity is derived from content.
"Strictly" means every baseline item appears at the same index, unchanged.

##### Why

This is the same predicate that governs incremental upload (§4.3), so session
matching and delta computation share one definition rather than two that can
disagree.

#### Items are compared by value, not by encoding

A server-assigned `id` is ignored. A tool call's `arguments` string is compared
as a parsed JSON value where it parses, and as literal text where it does not.

##### Why

An id is absent when the client replays the same turn. Arguments travel as a
JSON *string*: the backend emits keys in the order the model produced them, and
the client replays the object it parsed in its own serializer's order. Compared
as text, every turn after the model wrote a file forked the conversation and
uploaded the whole history again (measured live). Arguments that do not parse
have no canonical form, and inventing one would make two different calls equal.

#### A shared prefix may match; a partial prefix may not

Two conversations with the same system prompt and opening turn are
indistinguishable until they diverge, and may match the same session.

##### Why

That is harmless: the shared prefix is identical, so the baseline is correct
for both, and the first divergent turn separates them. A longest-common-prefix
score would graft one conversation onto another.

### 3.2 State

#### What a session holds

Its input baseline (what was sent plus the output items the server added), the
last request and response id, its transport binding (§4), its discovered tool
names (§2.5), its retained reasoning items (§3.3), its estimator fit (§6.3),
and its id, which is both the `session_id` header and `prompt_cache_key`.

#### The store is bounded and forgets idle conversations

At most 64 sessions, evicted by least recent use. A session untouched for an
hour is forgotten; the sweep runs whenever a request is resolved. Nothing is
ever refused for lack of room.

##### Why

A full store must degrade to full sends, never to errors. The client never says
a conversation has ended, so idleness is the only signal there is.

#### The longest matching baseline wins

##### Why

A candidate that extends two baselines extends the shorter only because the
shorter is a prefix of the longer, and continuing it would drop everything in
between.

#### A new session is claimed before its first turn is confirmed

The items just sent are seeded into a brand-new session's baseline. A session
that has completed a turn is never seeded; its baseline moves only when the
next turn completes.

##### Why

Seeding stops a concurrent request from matching an empty baseline and joining
a conversation it has nothing to do with. Seeding a *confirmed* baseline is what
makes a failed turn corrupt the next delta: the backend never saw the items,
the baseline says it did, and the next delta skips them.

#### Sizing never creates a session

`count_tokens` looks a conversation up without creating or reordering one.

##### Why

An entry made there would never advance, would match every first turn that
followed, and at capacity would evict a conversation someone is having.

#### A change of serving account forgets every session

Selecting another account, or removing the one serving, clears the store.

##### Why

The conversations are bound to the previous account's connections. Each pays
one full upload on its next turn, which is what every ambiguity resolves toward
anyway (§4.3), and the alternative is a conversation billed to an account the
operator just moved off.

### 3.3 Reasoning continuity

#### Server reasoning is retained and re-injected in place

Requests ask for `reasoning.encrypted_content`, so responses carry reasoning
items. The session keeps them and puts them back, in their original position,
on the next request. They belong to the baseline exactly as other returned
items do.

##### Why

They cannot survive a round trip through the client: `thinking` blocks are
dropped on the way in (§2.2), and the client would not return encrypted
upstream reasoning anyway. Without re-injection every turn begins with the
model's prior reasoning discarded.

#### Continuation is judged by the reconciling predicate

A conversation is held in two forms. What the client replays never contains the
server's reasoning; what the backend holds does. **Reconciling** converts the
first into the second by matching past server-only items rather than against
them. Session identity (§3.1) uses the reconciling predicate. The delta (§4.3)
is then computed on the reconciled input by strict comparison.

##### Why

A baseline holding an item the client cannot replay is never a strict extension
of any later replay, so strict matching stops the moment the model reasons. The
conversation would silently restart on its third turn: new session, lost
calibration, lost discovered tools, a full upload every turn.

Running the reconciling rule on already-reconciled input misaligns exactly the
items it put back, so the order is fixed.

#### This is the one place the proxy adds content the client did not send

It is additive and upstream-only. Nothing synthesized here is surfaced to the
client as model output.

### Where it lives

| File | What it holds |
|---|---|
| `crates/core/src/session.rs` | `extends`, `delta`, `reconcile`, value comparison of items, `Baseline` |
| `crates/proxy/src/session.rs` | `Session`, `SessionStore`: capacity, idle expiry, longest match, seeding, read-only lookup |
| `crates/core/src/translate/response.rs` | `retained_reasoning`, collected from completed reasoning items |
| `crates/proxy/src/ingress.rs` | Reconciling before the send, advancing the baseline when the stream ends |
| `crates/proxy/src/control/handler.rs` | Clearing sessions on a select or a removal of the serving account |
| `crates/core/tests/session.rs` | The predicate's invariants |

---

## 4. Transport

#### Transport belongs to the provider, not to the session

Everything in this section describes the first provider's path. Its two
transports are interchangeable and neither is a degraded form of the other. The
relay in §9 uses HTTP with SSE and nothing else.

##### Why

WebSocket and incremental upload are this backend's protocol, not a general
capability.

### 4.1 WebSocket

#### WebSocket is primary, one connection per session

The connection is opened lazily on a session's first turn and reused for every
later one.

##### Why

Reuse removes per-turn TCP and TLS setup, which is significant in an agent loop
issuing many sequential requests.

#### A connection is read once before it is handed over

After sending, the first event is read before the stream is returned. A socket
that closes before sending anything is a failed attempt, not an empty turn.

##### Why

A policy close accepts the handshake and *then* closes. Handing back an empty
stream would render as a turn where the model said nothing.

#### A connection is parked only after a clean turn

The connection returns to the session when the turn ends on
`response.completed`, `response.incomplete`, `response.failed`, or `error`. One
that failed mid-turn is dropped. Where two turns overlap and both open sockets,
the first to finish is kept and the other closes.

##### Why

Reusing a socket in an unknown state risks attaching the next turn to a
conversation the backend already abandoned, silently.

#### A pooled socket belongs to the account that opened it

A socket is reused only for a turn authenticated as the same account (§7.1).

##### Why

A connection authenticates once, at the upgrade, and carries every turn sent
over it. A turn sent over another account's socket spends the opener's quota,
succeeds, and says nothing.

#### A stale pooled socket is retried once, fresh, in full

A failure on a connection carried over from an earlier turn is retried once on
a new connection as a full send. Only a failure on a fresh connection latches
the session to HTTP (§4.2). A first event of type `error` whose code is
`websocket_connection_limit_reached` or `previous_response_not_found` is such a
failure, although the socket stays open: it says the connection cannot carry
the turn, not what the model answered.

##### Why

A socket the backend closed while idle is not evidence that WebSocket does not
work here. The retry is full because `previous_response_id` names a response
the closed socket held and the new one has never seen.

#### Prewarm exists and is not used

A prewarm frame (`generate: false`) opens a connection before the turn that
will use it. The daemon never sends one.

##### Why

A proxy learns that a conversation exists only when its first request arrives,
at which point opening the connection and sending on it are the same act.
Prewarming needs a signal that a turn is *about* to happen, which a front-end
watching the user type has and an HTTP surface does not.

### 4.2 HTTP fallback

#### HTTP with SSE is a complete transport

Every HTTP turn carries the whole conversation and the `session_id` header.

##### Why

The backend is documented to close WebSocket connections under policy
conditions. No such close has been observed on the accounts tested, and one
account's experience is not evidence about every account's, so fallback is
covered as an ordinary path.

#### A session that cannot use WebSocket latches to HTTP for its life

When a fresh WebSocket attempt fails (a refused handshake, a policy close), the
turn proceeds over HTTP and the session never tries the socket again.
`[transport] websocket = false` sends every session over HTTP.

##### Why

Retrying every turn spends a failed handshake per turn to re-learn what the
first one established, on the latency path of every request.

### 4.3 Incremental input

#### On a reused connection, only new items are sent

The request carries `previous_response_id` and only the items the conversation
added since that response.

##### Why

The Messages API is stateless, so the client replays the whole conversation
every turn. In a long session a full re-upload dominates both upload cost and
time to first token.

#### A delta requires all of these

- a previous response id and a previous request exist;
- every non-input field of the request is unchanged (compared by serializing
  the request with its input emptied, so a field added later is covered);
- the reconciled input strictly extends the baseline (§3.3);
- the delta is not empty;
- the pooled connection is the one that produced the previous response, and
  was opened as the same account.

Anything else sends the full input.

##### Why

A response id names a response held by the connection that produced it. Handed
to any other connection, the backend refuses it with `400 Invalid
previous_response_id` (observed live). That refusal ends the turn cleanly, so
the refusing connection is parked and every later delta repeats it: the session
never heals on its own.

An empty delta is not a small delta. Given a previous response id and no new
items, the backend answers from that response, so a client retrying an
unchanged conversation would receive the previous turn again.

#### Server-returned items are part of the baseline and never resent

#### A turn enters the baseline only when its stream ends

The baseline advances to what was sent plus what the server returned when the
upstream stream ends with `response.completed`, and that response is the one a
later delta continues. A turn whose transport failed before or during the
stream, or whose response failed or ended on an `error` event, never advances
it.

##### Why

Recording a failed turn makes the next delta continue a response that never saw
those items, and the question vanishes from the conversation with no error. A
brand-new session is seeded early (§3.2), and nothing is at risk there: with no
completed turn there is no response to continue, so it can only send in full.

#### Falling back is always safe; a wrong delta is not

A full send costs bandwidth. A wrong delta corrupts the conversation and does
not fail visibly. Every ambiguous case resolves toward the full send.

### 4.4 Compression

#### HTTP: zstd on the body, announced

A body larger than 1 KiB is zstd-compressed and sent with `Content-Encoding:
zstd`. A smaller body is sent as it is. Governed by `[transport] compression`.

##### Why

The header is the whole mechanism: compressed bytes without it are refused with
an error naming nothing. Below the threshold compression adds more than it
removes.

#### A key request is never compressed

This is measured against the subscription backend and asserted about no other
(§8.2).

#### WebSocket: `permessage-deflate`, negotiated at the upgrade

The client offers the extension and the server selects it (RFC 7692), so
declining to offer it is the only way to switch it off. The frame stays a text
frame carrying JSON; the library compresses it and marks that in the frame
header.

##### Why

Measured live, one identical turn with the extension offered and declined,
counted on the wire:

| bytes | offered | declined |
|---|---|---|
| inbound | 104,566 | 300,879 |
| outbound | 40,335 | 110,608 |

About 65% in both directions. The inbound half is larger and grows with the
conversation, because the backend echoes the entire request in
`response.created`, `response.in_progress`, and `response.completed`: three
copies per turn, which nothing else here has a lever on.

It saves bytes and **no tokens at all**.

The server negotiates context takeover (bare `permessage-deflate`, no
`no_context_takeover`, no window limit). Its contribution is **derived, not
measured**: offline deflate over a captured turn put it at about 3%, since a
32 KiB window cannot reach back across a 99 KB event.

##### Tried and dropped

A binary frame as a way to say "compressed". Nothing in the protocol attaches
that meaning: plain JSON in a binary frame is accepted, the same JSON compressed
is refused (measured).

#### The WebSocket read limit is far above the library default

A single frame may be up to 64 MiB, with a 128 MiB buffer.

##### Why

One event legitimately carries a whole conversation (the echo above). A cap
sized for ordinary messages would sever a long conversation mid-turn.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/upstream/conduit.rs` | `Conduit`: choosing a transport, the stale-socket retry, latching, prewarm |
| `crates/proxy/src/upstream/websocket.rs` | The upgrade, `plan_upload`, `non_input_fields_match`, frame limits |
| `crates/proxy/src/upstream/pool.rs` | `PooledConnection`: `saw`, `opened_as`, `pump`, `park` |
| `crates/proxy/src/upstream/http.rs` | The HTTP transport and its SSE body |
| `crates/proxy/src/upstream/compression.rs` | `zstd`, `worth_compressing` |
| `crates/proxy/tests/transports.rs` | Reuse, fallback latching, delta validity against a replay server |
| `crates/proxy/tests/compression.rs` | Both compression halves, including that the extension is offered |

---

## 5. Response translation

### 5.0 Framing

#### SSE `data:` lines within one event are one payload

On the HTTP transport, the lines of one event are joined with newlines before
parsing. A line opening with `:` is a comment and ignored. `\r\n`, `\r`, and
`\n` all end a line, including a `\r\n` split across two chunks. An event left
unterminated when the body ends is still delivered.

##### Why

The SSE specification defines several `data:` lines as one logical payload.
Parsing each line separately corrupts exactly the events large enough to be
split: long tool-call arguments and long text deltas.

#### A payload that does not parse is ignored

`[DONE]` is not special on the live path: it does not parse as JSON, so it is
dropped by this rule, and the stream ends when the body does. Only the surface
vocabulary comparison drops it by name, so it does not appear there as an event
this proxy fails to emit.

#### WebSocket events need no reassembly

Each text frame is one event; pings are answered by the library. Both
transports produce the same event stream before translation, so §5.1 onward is
transport-independent.

#### Every emitted frame names its event

Each SSE frame the proxy writes carries `event:` as well as `data:`.

##### Why

A client that dispatches on the event name sees nothing without it, and the
Anthropic API sends both.

### 5.1 Events

#### One state machine, one open block at a time

Upstream events are read permissively: dispatched on `type`, anything
unrecognized ignored.

| Responses event | Anthropic output |
|---|---|
| `response.created` | `message_start` |
| `response.reasoning_summary_text.delta`, `response.reasoning_text.delta` | `thinking` block, `thinking_delta` |
| `response.output_text.delta` | `text` block, `text_delta` |
| `response.output_text.annotation.added` | nothing yet; a citation for §5.2 |
| `response.output_item.added` (function call with a name) | `tool_use` block start |
| `response.function_call_arguments.delta` | `input_json_delta`, for the open call only |
| `response.output_item.done` (function call) | the full arguments if none were streamed, then `content_block_stop` |
| `response.output_item.done` (reasoning) | nothing; retained for §3.3 |
| `response.output_item.done` (web search call, message) | nothing yet; collected for §5.2 |
| `response.completed` | `message_delta` with final usage, `message_stop` |
| `response.incomplete` | `message_delta` with `stop_reason: max_tokens` and final usage, `message_stop` |
| `error`, `response.failed` | `error` frame |

The `model` reported in `message_start` is the id the client asked for, not the
upstream id it mapped to.

##### Why

A backend that adds an event must not break a client that has not learned it.
The client matches the reported model against what it asked for.

#### A `tool_use` block starts only once its name is known

A function call announced without a name waits for its completed item, which
then opens the block. A completed call whose arguments were streamed does not
repeat them.

##### Why

An Anthropic client cannot patch a block header after it is emitted, and
receiving the arguments twice leaves it parsing the same JSON twice.

#### The stop reason

`tool_use` when the turn produced any function call, `max_tokens` on an
incomplete response, `end_turn` otherwise.

#### A stream that ends without completing is closed

If the upstream stream ends after `message_start` without a terminal event, the
open block is closed and `message_delta` and `message_stop` are emitted.

##### Why

A message left open hangs the client on a turn the backend abandoned, which is
indistinguishable from a model still thinking.

#### A refusal before the response starts is a status

The first four upstream events are read before anything is written. An `error`
event among them becomes an HTTP error response in the shape of `api.md` §1.1,
with the status the event states (502 where it states none) mapped as in §2.8.
A stream that ends among them, before any event of the response itself, is
`overloaded_error`.

##### Why

The backend opens a stream with a quota snapshot and metadata before it speaks
to the outcome. A 200 whose body is one error frame and no `message_start` is
not a message the client can read. The peek is bounded so a slow backend cannot
hold the status open.

#### A failure after the response starts is an error frame

An `error` or `response.failed` event later in the stream becomes an `error`
frame, typed from its code: `server_is_overloaded` and `slow_down` →
`overloaded_error`; `rate_limit_exceeded`, `usage_limit_reached`, and
`insufficient_quota` → `rate_limit_error`; `context_length_exceeded`,
`invalid_prompt`, and `bio_policy` → `invalid_request_error`; anything else →
`api_error`. A transport failure mid-stream is an `overloaded_error` frame; a
WebSocket closed before the turn's terminal event is one, and does not advance
the baseline (§4.3).

##### Why

The status is already sent. Transient conditions must reach the client's own
retry logic as retryable, and terminal ones as terminal; an unrecognized code is
reported, not guessed at.

### 5.2 Search results

#### Search is reconstructed into Anthropic's structured blocks

Each search call becomes a `server_tool_use` block (`name: "web_search"`, input
`{query}`) followed by a `web_search_tool_result` block listing the sources as
`{type: "web_search_result", url, title}`.

##### Why

The client extracts `url` and `title` from those blocks. Passing the model's
prose answer through as the tool result leaves that extraction empty.

#### Where query and sources come from

- The query is the search action's `query`, or the first of its `queries`.
- A source is each `url_citation` annotation, from the streamed annotation
  event or from the completed message's content. A citation with no title uses
  its URL.
- A page the model opened (`open_page` or `find_in_page` action) is a source
  even when nothing cited it.
- A URL seen more than once is one source.

##### Why

A search that fetched pages but produced no citations would otherwise reach the
client as an empty result, which reads as "nothing found". The client renders
the list verbatim, so duplicates would show as the same page twice.

#### The blocks close the message

Both blocks are emitted after the last content block, before `message_delta`.
Every search call's result block carries the turn's full source list.

##### Why

Citations arrive while the answer is written, after the search completed. The
position does not affect what the client extracts.

Citations are the part the upstream client cannot corroborate: it discards
annotations. The annotation shape is the public API's.

### 5.3 Cancellation

#### Cancelling the outbound stream aborts the upstream request

Dropping the response drops the upstream stream. On WebSocket the connection is
dropped rather than parked.

##### Why

Without propagation the backend generates to completion against a reader that
no longer exists, spending quota on output nobody receives.

### 5.4 Empty streams

#### A stream that produced no content is recorded

A stream that ends having produced no content delta is written to the recorder
with its request and the upstream events it parsed: the parsed values, not the
raw bytes, so a payload that never parsed is not in the record.

##### Why

It is always a defect, and otherwise invisible: the client sees a well-formed
turn that said nothing.

### 5.5 A request that did not ask for a stream

#### `stream` absent or false is answered with one JSON body

The response is `application/json`, built by folding the §5.1 frame sequence:
blocks closed, deltas concatenated, tool arguments parsed back from their
fragments.

##### Why

The ingress claims to be a Messages API, and the endpoint's default is not a
stream. A caller that did not ask for `text/event-stream` did not agree to
parse one. Claude Code always streams, so the harness never takes this path.

#### The fold invents nothing

Arguments that do not parse are a failure in the error shape of `api.md` §1.1,
never a plausible object. The usage reported is the one `message_delta`
carries, never the `message_start` estimate (§6.1, §6.2).

#### Only the written shape differs

Calibration, the tally, session bookkeeping (§3.3, §4.3), and capture (§5.4)
run off the same sequence either way.

##### Why

A non-streaming turn advances a conversation exactly as a streaming one does.
Because nothing is written until the fold is done, a failure here is still a
status and an error body.

### Where it lives

| File | What it holds |
|---|---|
| `crates/core/src/sse.rs` | `SseDecoder`, `encode_frame` |
| `crates/core/src/translate/response.rs` | `ResponseTranslator`, search reconstruction, `classify`, `translate_usage` |
| `crates/core/src/anthropic/stream.rs` | Outbound frame types |
| `crates/core/src/anthropic/aggregate.rs` | `aggregate`, the non-streaming fold |
| `crates/proxy/src/ingress.rs` | `peek_preamble`, `upstream_refusal`, `frame_stream`, `json_response`, empty-stream capture |
| `crates/proxy/src/surface.rs` | The surface vocabulary comparison |
| `crates/core/tests/response_translation.rs`, `crates/core/tests/response_snapshots.rs` | Frame sequences |
| `crates/core/tests/sse_framing.rs` | SSE framing |

---

## 6. Token accounting

### 6.1 Upstream figures are authoritative

#### Completed responses' counts are never recomputed

The usage block of `response.completed` or `response.incomplete` is used as
given.

#### One conversion: cached tokens leave `input_tokens`

`input_tokens` becomes upstream `input_tokens − cached_tokens`, clamped at
zero. `cached_tokens` becomes `cache_read_input_tokens`.

##### Why

OpenAI's `input_tokens` includes cached tokens; Anthropic's excludes them and
reports cache counters separately. The clamp stops an unsigned wrap from
rendering a context meter far past full.

#### `cache_creation_input_tokens` is always zero

##### Why

Upstream caching is implicit, with no write event to report. It stays zero
rather than being synthesized into something plausible.

#### What was never observed is reported as unobserved

An account with no figure reports that *this daemon* has recorded no turn as it,
not that none was spent.

##### Why

The daemon records the turns that pass through it. A turn spent elsewhere
(`doctor --live` relays one from a CLI process that exits holding its response
headers) leaves it nothing. "None has been spent" is a claim about the account
that the store has no standing to make.

#### Served tokens are tallied per account, and never become a cost

A completed translated turn's upstream `input_tokens` and `output_tokens` are
added to the account that served it: the launch tag's account, else the tier's
pinned account, else the serving account (`api.md` §2.3, §8.3).

- The tallied input is the **raw** upstream figure, cached tokens included: the
  tally records what upstream billed, not the converted figure above.
- A turn upstream reported no usage for adds nothing.
- A turn no account can be named for is not counted.
- Nothing here knows a price, and none is inferred.

##### Why

A metered account can then be told how much has been spent through this daemon.
Filing an unnamed turn under whoever happens to be serving would put one
account's spend under another's name.

#### Only a translated turn is tallied

A relayed turn is not parsed (§9), so it leaves a quota snapshot (§9.4) and no
tally. An account of the second provider, every turn of which is relayed, keeps
a tally of zero however much it is spent.

#### The tally is a floor

Everywhere it is reported, it says so.

##### Why

Turns made elsewhere are invisible to it, and a figure that reads as the whole
of an account's spend is wrong in the reassuring direction.

#### The tally and the quota snapshots persist, each by its own rule

The tally lives in `spend.json` and the snapshots in `quota.json`, both under
`config_dir()`. Removing an account drops its row from both.

- The **tally** is read back whole. A restart that reset it would state a floor
  of zero that was never measured.
- A **quota snapshot** (§8.3) is restored per window, against the reset time the
  provider gave. A window whose reset has passed is dropped. A window with no
  stated reset is dropped. An account left with no window is not restored. A
  record with no taken-at time is not restored.

##### Why

A percentage says nothing about whether its window still exists; the reset says
exactly that. A passed window is back to zero and showing it reads as headroom
that is not there. After an arbitrary gap nothing about an undated or unreset
window can be shown to still hold. An empty snapshot reads as "quota known,
nothing used". The time taken is half of what the meter prints (`2h ago`), so a
figure that cannot be dated is not shown as current.

Both are daemon state, not configuration and not the credential store: each
holds account names and numbers, and has no place for any part of a secret.

#### A write replaces the file; it never writes into it

The body is written to a sibling named with the process id, flushed, and
renamed over the target. A sibling left by a killed write is never read.

##### Why

`std::fs::write` truncates then fills, and a daemon killed between the two
leaves a short file that parses as an empty tally: a floor of zero. `proxenos
stop` under the supervisor kills the daemon on every install, so that is the
ordinary shutdown.

#### Both files are written at the process umask

##### Why

They hold no part of any credential, and `0600` would state something about them
that is not true.

#### Two daemons sharing a home lose at most a turn's count

`PROXENOS_HOME` can point two daemons at one directory, and neither sees the
other's turns.

- The **tally** merge takes the higher count per account. Before replacing,
  the file is re-read; if it changed, the attempt starts over. After five
  attempts the last one writes what it has.
- A **snapshot** write reads once, keeps the **later** record per account, and
  replaces. `usage --refresh` replaces a record the same way a turn does.

No lock is taken.

##### Why

A tally accumulates, so the higher count is closer to the truth; a snapshot
replaces, so the later measurement describes the account now. The comparison
and the rename are two operations and a write landing between them is lost:
that costs a smaller floor, never a corrupted file or a count that moved
backwards. A lost credential write is a whole account, which is why the
credential store takes a lock (§8.1); a lost tally write is one turn's count.

#### A tally file that cannot be read is an empty tally

It is written over, and every write failure is silent.

##### Why

Serving turns does not depend on this file. A daemon that refused a turn over
its bookkeeping would trade the product for it.

#### The write is blocking, on the worker that served the turn

##### Why

It runs once per completed turn over a few hundred bytes, and a spawned write is
a write a shutdown can outrun.

### 6.2 The two points that need an estimate

#### `count_tokens` is estimated

The Responses API has no counting endpoint and nothing has been sent. The answer
uses the conversation's own calibrated estimator where the conversation is
known (§3.2, read-only lookup), and an uncalibrated one otherwise.

##### Why

A fresh estimator per call would leave sizing uncalibrated however long the
session had run.

#### `message_start` carries an estimate, never zero

##### Why

Upstream reports usage only at completion, and the client renders
`message_start` usage live. A zero collapses the context meter at the start of
every turn. The estimate is never below one.

#### Ground truth replaces the estimate within the exchange

`message_delta` carries cumulative final usage, so the true value replaces the
estimate rather than adding to it.

#### The raw estimate

Characters divided by 3.6, rounded up, plus 4 tokens per item. The items are
the system prompt, each message, and each tool that is sent; a withheld deferred
tool (§2.5) counts nothing. Characters are text, tool names, call inputs, and
tool descriptions and schemas. An image or document counts as 3,000 characters
rather than its base64 length.

### 6.3 Calibration

#### The estimator corrects itself against upstream

Each completed turn yields the raw upstream `input_tokens` for a request that
was also estimated, and the pair is folded into a fit retained on the session.
The observation is on the *raw* estimate: the correction in force is inverted
off the reported estimate first.

##### Why

Fitting against a figure the fit produced makes the correction chase its own
output.

#### The fit is a line, not a multiplier

Scale and offset are fitted together by incremental least squares.

##### Why

Part of the unmodelled cost scales with the conversation and part does not: the
instructions wrapper is charged once however long the session runs.

##### Tried and dropped

A single ratio. It converges from whichever regime it saw first: an early short
request, where the fixed cost dominates, pulls it high, and every estimate then
reads over while it decays.

#### An underdetermined fit is not invented

With fewer than two observations, or all at one size, or a fitted slope that is
not positive, the estimator uses the mean ratio of true to raw, and 1 before any
observation.

##### Why

One size cannot separate scale from offset. A conversation does not get cheaper
as it grows, so a non-positive slope is noise, and applying it would make longer
sessions estimate lower.

#### A calibrated estimate ships; a tokenizer does not

The tokenizer estimator stays behind the `tokenizer` feature as a comparison
instrument.

##### Why

The upstream count includes framing the proxy does not model identically: the
instructions blob, serialized tool schemas, per-item overhead. A byte-exact
tokenizer over structurally different input is authoritatively wrong, which is
worse than approximate and self-correcting.

Against a *modelled* count (text cost, a per-item charge, a fixed wrapper) over
a growing session, mean absolute error over the second half was 0.01%
calibrated and 68% tokenizer. That demonstrates the mechanism, not the
accuracy: the model was linear in the same structure the raw estimate measures.

**Measured against the real backend**, over a growing six-turn conversation:

| turn | estimated | actual | error |
|---|---|---|---|
| 1 | 146 | 75 | +95% |
| 2 | 221 | 215 | +2.8% |
| 3 | 420 | 427 | −1.6% |
| 4 | 703 | 687 | +2.3% |
| 5 | 1026 | 1007 | +1.9% |
| 6 | 1406 | 1387 | +1.4% |

One observation brings the estimate inside 3%, and it stays there.

#### The first turn is uncalibrated

##### Why

Nothing has been observed yet. It is the one turn where the context meter is
visibly wrong, and it corrects on the next one.

### Where it lives

| File | What it holds |
|---|---|
| `crates/core/src/translate/response.rs` | `translate_usage`: the cached-token conversion |
| `crates/proxy/src/estimate.rs` | `CalibratedEstimator`, `Fit`, the raw estimate, the feature-gated tokenizer |
| `crates/proxy/src/ingress.rs` | `count_tokens`, `calibrate`, `tally` |
| `crates/proxy/src/usage.rs` | `UsageStore`: the tally, snapshot persistence, `restore`, `merge_into`, `replace_file` |
| `crates/proxy/src/config.rs` | `spend.json` and `quota.json` paths |
| `crates/proxy/tests/estimator.rs` | The fit's behaviour |
| `crates/proxy/tests/usage.rs` | Tally and snapshot persistence |

---

## 7. Models

### 7.0 Catalog

#### The catalog is fetched once and held

It is fetched at startup, as the serving account, and held for the life of the
daemon with no TTL.

##### Why

A mapping validated against it cannot then go stale on its own. A model added
or withdrawn later is not noticed until something makes the daemon ask again.

#### A catalog is one account's menu, and one provider's

The list is attributed to the account it was fetched for. It is fetched again
when the daemon changes which account serves: selecting another, or removing
the one serving. A `models` question about another stored account (`api.md` §3)
fetches that account's list to answer with and puts nothing in force. Both
catalog endpoints are the translating provider's, so an Anthropic account's
credential is never sent to either; its menu is the relay's (§9.1).

##### Why

The plan decides which models appear and which efforts each offers, so a list
fetched for one account says nothing about another. A mapping entry whose turns
are relayed is measured against no list here at all (§9.1).

#### A failed refetch keeps the list in force

##### Why

Fetch failure is not evidence that a model went away, and replacing a real list
with the fallback would withdraw models the account has.

#### A list that is not this account's says so

A grant can arrive with nothing to refetch on: a login over the control socket
completes in the background, and a login in the CLI with no daemon running has
no socket to hand over on. The list stays the previous account's, and every
answer built from it says so (`api.md` §3).

#### Each entry contributes an id, visibility, efforts, and a window

- The id is `id`, or `slug`.
- Visibility is `is_visible` where present, else `visibility != "hide"`, else
  visible.
- Efforts are the `supported_reasoning_levels`.
- The window is `context_window`, or `max_context_window` where the entry
  states no `context_window`.

Hidden entries are withheld from what is offered for mapping, but kept: their
windows and efforts still apply to a session that names them.

##### Why

Where both windows are stated, `context_window` is the smaller-scoped and
authoritative one; the maximum describes a ceiling the account may not have.
Offering a model that stated no visibility is the safer error than withholding
one the operator can use.

#### The effective window reserves headroom

The effective window is the window scaled by the entry's
`effective_context_window_percent`, or by `upstream.effective_window_percent`
where the entry states none. It is resolved when the catalog is parsed.

##### Why

The share reserves room for instructions, tool overhead, and output. A share
the catalog states for its own model wins: the configured value is a default,
never an override, and there is no compiled-in figure left to fall back to.

#### A failed fetch starts on a fallback list of ids only

A model with no known window is **unknown, not assumed**: the window guard
(§7.2) does not fire for it, and no share is derived from a guess. Validation
that depends on the catalog is skipped against the fallback, never failed.

##### Why

The daemon starts and reports honestly rather than blocking on an unreachable
catalog.

#### An authoritative empty catalog names the client version

A catalog that came back with no models refuses validation with a sentence
pointing at `upstream.client_version`.

##### Why

The backend answers a client version older than every model's minimum with an
empty list rather than an error, which reads exactly like an account with no
models.

### 7.1 Tier mapping

#### All four tiers are always mapped

`opus`, `sonnet`, `haiku`, and `fable` each resolve to a model: the operator's,
or the shipped default. An omitted tier takes the default and is marked
**defaulted**. A tier written blank is refused.

##### Why

An omission accepts the default; a blank is a mistake. `status` prints the
mapping in use whether or not it was written down.

##### Tried and dropped

Requiring all four to be stated, so the model handling background traffic is
never hidden. It made a first run fail on a file nobody had written, and
`status` already answers the concern.

#### A stated model is never overruled; a default may be

A defaulted tier naming a model this account's catalog does not carry is
replaced with one it has, and the substitution is logged. The replacement is the
first listed model among the tier's own earlier generations, then the defaults
of each tier below it (fable, opus, sonnet, haiku, in that order), then
`gpt-5.5`; failing all of those, the first offered model. A free account, which
lists neither sol nor astra, runs fable and opus on terra.

##### Why

A stated model is the operator's decision and they may know something the
catalog does not. A default is this proxy's guess about an account it has never
seen.

#### A stated model the catalog lacks marks its tier; it does not stop the daemon

At start and at `config.reload`, such a tier keeps the stated id and carries the
reason it cannot serve. A turn asking for it is refused, naming the tier, the
model, and what the catalog has. Every other tier keeps serving. Where every
tier is marked, the daemon still starts.

The mark is reported by `status`, the model list, `doctor` on a live run, and
once at WARN in the startup log. It is re-derived every time a mapping is put in
force, so fixing the file and reloading clears it.

##### Why

The blast radius belongs to the tier. `reload` is how the mapping is fixed, and
a process that exited cannot be reloaded.

##### Tried and dropped

Refusing to start. One retired model took down every tier that resolved and
every process depending on them.

#### A switch is refused rather than marked

`tiers.set` and `accounts.select` validate the mapping and refuse, leaving the
daemon where it was, catalog included.

##### Why

Those are things the operator typed a moment ago: refusing is immediate
feedback and nothing that was serving stops. A start and a reload have no
fallback, so they mark instead.

#### An unavailable catalog skips the check

Against the fallback list, or a refetch that failed, nothing is marked,
substituted, or refused.

##### Why

An unreachable catalog is not evidence that a model went away, and refusing a
mapping over somebody else's menu is worse than not checking.

#### The mapping belongs to an account

`[accounts.<name>]` states what differs for one account: its own `tiers` and
its own `effort` ceiling. The shared `[tiers]` and `effort` answer for
everything it does not state. It is keyed by the name the store files the
account under.

An account's ceiling **replaces** the shared one rather than being capped by it.

##### Why

Two subscriptions on different plans are offered different models, and a key
account need not overlap a subscription at all, so one mapping is right only
for the models every account has. A name is the key because every account verb
takes one and a key account carries no id. Capping would make an account unable
to raise; the non-negotiable cap is the model's own (§2.7).

#### A change is persisted where the value is read from

Account tables are read from disk when needed, not from the startup snapshot.
`api.md` §3 carries how each method chooses where to write.

##### Why

An account section shadows the shared table, so writing to the shared table
while a section exists leaves the change live now and gone at the next start.
The account tables are the part of the configuration the daemon writes, and a
daemon that cannot see its own writes gets this wrong in both directions.

#### A switch re-resolves the mapping and can be refused by it

Selecting an account resolves its tiers and ceiling and validates them against
the catalog fetched for it before anything else moves. The refusal names the
account section as the way out.

##### Why

The alternative is serving an account whose every turn names a model its
backend will not answer for, failing a turn later with a message that says
nothing about tier mapping. Naming only the model leaves an operator editing one
shared table before every switch.

#### A tier may pin another account, with consent

A tier entry `{ account = "<name>", model = "..." }` serves that tier's turns as
the named account; every unpinned tier uses the serving account. Pinning is
refused unless `cross_account_tiers = true`.

##### Why

A pin routes one client's traffic across accounts: main turns spend one quota
while the pinned tier spends another's, invisibly to the session. That is only
done on the operator's word.

#### A pinned tier authenticates every upstream request as its account

The account travels in the routing table beside the model, because that table
is the only thing a turn resolves against. A pinned account's credential is read
by name through its own reader, and its sockets are its own (§4.1).

#### A pinned tier is not validated against the serving account's catalog

The exclusion holds at every door: start, `tiers.set`, and a switch, all through
one function.

##### Why

Its model belongs to the pinned account's menu, which is not the list in force.

##### Tried and dropped

Validating pins at start but not at `tiers.set`. The socket accepted and
persisted a pinned entry, and the next start refused the daemon over it.

#### A pin naming an unknown account refuses the turn

The refusal names the account and lists what is stored. A pinned account holding
a credential of the wrong kind is refused as §8.2 refuses a mismatch, naming the
pinned account.

##### Why

Falling back to the serving account would succeed, read identically to a correct
turn, and spend a subscription nobody pointed at it. A mapping and a store are
edited separately, and either can be the half that is wrong.

#### Per-tier client effort is published, never applied

A tier entry may state `effort`, one of the client's own levels. It reaches the
client in the launch settings as that model's `effortLevel` (`api.md` §2.2). It
never touches a request here; the ceilings of §2.7 still cap what arrives. Two
tiers on one model must agree on its effort.

##### Why

The client keys effort by model, not tier, so disagreeing tiers on one id would
deliver one of the two silently.

### 7.2 Context window

#### A mapped model id must not contain `[1m]`

The daemon rejects one that does.

##### Why

The client infers a window from the model id: an unrecognized id is assumed to
hold 200,000 tokens, and an id carrying `[1m]` 1,000,000. Real windows here are
smaller than a million, so the marker would make the client believe it has
about four times the headroom it has, and compaction would never fire in time.
Early compaction wastes context; late compaction fails the session.

#### `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` is set wherever any tier translates

A mapping served entirely by the relay omits it.

##### Why

Measured: without it the client appends `[1m]` to an unrecognized id and assumes
a million tokens. The flag also removes `context-1m-2025-08-07` from the
client's `anthropic-beta` (measured on ingress capture). On a translated id that
costs nothing; on a relayed id it denies an entitlement the account may hold. A
split mapping keeps it, because a denied entitlement makes a smaller session
while a fabricated window makes one that overruns.

#### The window is stated only when no tier is relayed

With every tier translating and the catalog knowing at least one window,
`CLAUDE_CODE_MAX_CONTEXT_TOKENS` and `CLAUDE_CODE_AUTO_COMPACT_WINDOW` are both
set to the smallest **effective** window across the mapped tiers. Once any tier
is relayed, neither is set.

##### Why

One value covers every tier, and the smallest is the only one that cannot
overrun. The effective window, because this daemon refuses a turn above it.

A relayed id is one the client knows natively, and this catalog is not its menu.
On a split mapping, a figure from the first provider's catalog would also govern
the relayed tiers, which have no window guard behind them; the translating tiers
instead fall back to the client's 200,000 and compact early. An early compaction
on the guarded side is the cheaper failure.

##### Tried and dropped

Stating the raw `context_window`, so the client's meter would not read short.
The meter then offered a band of context this daemon refuses, and the meter and
the refusal named different limits.

#### Both variables, or neither

##### Why

Stating the window alone is worse than saying nothing: the client stops applying
its own 200,000 assumption and, not recognizing the model, enforces no limit at
all. The compact window turns a stated figure into an enforced one.

#### The compact window is set only between 100,000 and 1,000,000

Outside that range it is omitted and a warning is logged.

##### Why

The client answers anything else with "Expected 'auto' or 100k–1M tokens", and
the settings key of the same meaning discards an out-of-range value silently.
Both ends are reachable: a small model, or a low
`upstream.effective_window_percent`.

That compaction fires against this figure is **derived from the client's code,
not observed**.

#### The client's 200,000 warning is expected

##### Why

Exceeding 200,000 is the point. Silencing it would mean compacting at 200,000
and discarding a fifth of the usable context to avoid a message.

#### A relayed tier's model id is emitted only where the operator named it

`ANTHROPIC_DEFAULT_<TIER>_MODEL` is set for every translated tier. For a relayed
tier it is set only where the model was stated for the relaying account: a pin,
or an entry in `[accounts.<name>.tiers]`. Otherwise it is left unset and the
client's own id relays verbatim.

##### Why

A relayed tier decides nothing (§9.1), and its entry in the shared table is the
first provider's menu. Seen live: `--model haiku` arrived at the second provider
as a first-provider model id.

#### The environment is asked about the account that will serve the session

A launch tag names that account (§9.1, `api.md` §2.3); the selection answers
where nothing is tagged. The mapping resolves for that account too.

##### Why

Asking the selection while the turn asks the tag handed a session tagged onto a
second-provider account the first provider's ids, which that backend refused as
unrecognized.

#### The daemon enforces the effective window itself

A request whose `message_start` estimate (§6.2) exceeds the model's effective
window is refused as `invalid_request_error` before it is sent, naming both
figures. A model with no known window is not checked.

##### Why

Sending it spends the request to learn what the catalog already said and returns
an opaque upstream rejection. An unknown window is unknown, not unlimited.

### 7.3 Client policy

#### Policy is published, never installed

What must reach the client's settings file is emitted beside the environment
(`api.md` §2.2) and applied by whoever starts the client: a person writing it
into a settings file, `exec` splicing it into one launch (`api.md` §2.3), or a
supervisor merging it into its argument list. Nothing here writes a file the
proxy does not own.

##### Why

No environment variable reaches these settings: checked against the whole
settings schema, there is no per-skill variable and nothing that points at an
extra settings file. The one variable that comes close relocates the client's
entire state directory, credentials and history included.

#### Settings layers union, except two `--settings` flags

A rule in a project settings file and a rule on the command line were both
enforced in one session (measured). A deny rule survives an untrusted workspace,
where an allow rule is dropped. But given two `--settings` flags, the client
keeps the last and drops the first, silently.

##### Why

This is why `exec` refuses a collision rather than choosing a side, and why a
supervisor that already passes the flag merges into its own document.

#### The published document

| `[client]` key | Default | Settings emitted |
|---|---|---|
| `deny_skills` | unset: `claude-api` denied when any tier translates, nothing when all are relayed | `permissions.deny: ["Skill(<id>)", ...]` |
| `disable_connectors` | `true` | `disableClaudeAiConnectors: true` |
| `disable_remote_control` | `true` | `remoteControlAtStartup: false` |
| `disable_commit_attribution` | `true` | `attribution.commit: ""` |

Per-tier `effort` adds `modelSettings` (§7.1). A written `deny_skills` list is the
operator's rule on either path; an empty list denies nothing.

#### `claude-api` is denied for a translated session

##### Why

It documents the second provider's model ids, prices, and parameters. A
translated session is not talking to that API, so the reference is wrong twice:
it costs context, and a model that reads it answers confidently about a model
that is not itself. A relayed session is served by the provider it documents, so
there it is the right reference.

Measured against a local capture stub, nothing forwarded:

| | |
|---|---|
| Skill content injected on one invocation | 73,000 to 93,000 bytes, roughly 18,000 to 23,000 tokens |
| Cost of a refused invocation | one 43-byte error result |
| Effect on the listing the client sends | **none** |

Denying does not remove the skill from the listing, so the model may still reach
for it and lose a turn. What the deny stops is the load. The range is real: the
same probe read 92,601 bytes in a populated environment and 73,214 in a bare
one.

The load keeps costing. In tokens always: it lands as a user item and is charged
every turn, moving compaction earlier. In bytes it depends on transport: re-sent
every HTTP turn (§4.2), uploaded once over WebSocket (§4.3), and echoed back
three times per turn on either (§4.4).

#### The connector notice is suppressed by the settings key

`disableClaudeAiConnectors` silences the notice the client prints whenever an
auth token is set, which here is always. Confirmed on the current client
(`roadmap.md` §L).

#### Connectors are off regardless of this proxy

The same key also emits `ENABLE_CLAUDEAI_MCP_SERVERS=false` among the routing
exports. It is a costless belt: claude.ai-hosted servers never join a session
whose base URL is a proxy, because another auth source takes precedence, and
three launches with and without the export attached none (`roadmap.md` §L).

#### The policy is published even when it is empty

The payload always carries the field. Absence means only that the daemon
predates client policy. The verbs whose output would be incomplete without it
refuse; the one that carries routing alone continues and says which daemon
answered (`api.md` §2.2, §6).

##### Why

One file is both the daemon and the CLI, and replacing it does not restart a
running daemon, so a newer CLI against an older daemon is what an ordinary
upgrade leaves behind. An empty policy and a daemon that cannot answer must not
look the same.

#### A denied call is attributed by `status`

`status` reports the policy under the configuration's own key names.

##### Why

The client refuses with "Skill execution blocked by permission rules" and names
no source. The person holding that message needs the key that undoes it.

### Not done, on purpose

- No catalog TTL. A switch is what refetches (§7.0).
- No `[1m]` in a mapped id, and no way to opt into one on the translating path
  (§7.2). `exec` upgrades a plain `--model` to its `[1m]` variant only where the
  serving account relays (`api.md` §2.3).
- No settings file written by this proxy (§7.3).

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/catalog.rs` | `Catalog`: parse, fallback, `effective_window`, `substitute_unavailable_defaults`, `validate`, `mark_missing`; `CatalogSource` refetch |
| `crates/proxy/src/config.rs` | `[tiers]`, `DEFAULT_TIERS`, `[accounts.<name>]`, `cross_account_tiers`, `ClientConfig`, `model_settings`, `check_effort_conflicts` |
| `crates/proxy/src/policy.rs` | `Snapshot`, `Policy::snapshot_for`: the mapping a turn resolves against |
| `crates/proxy/src/upstream/relay.rs` | `validated_models`, `validated_tiers`: which tiers the catalog may judge |
| `crates/proxy/src/control/handler.rs` | `environment_for`, `tiers.set`, `accounts.select`, `config.reload` |
| `crates/proxy/src/ingress.rs` | The marked-tier refusal and the window guard |
| `crates/proxy/src/launch.rs` | `exec`'s `--settings` collision rule and the `[1m]` argument upgrade |
| `crates/proxy/tests/catalog.rs` | Catalog parsing and visibility |

---

## 8. Credentials

#### Authentication is borrowed

This proxy runs no authorization flow and holds no refresh-token family. A
subscription grant is read from the profile of the program that owns it (§8.4).
The only credential it stores is a key (§8.2).

##### Why

A refresh-token family belongs to one holder. Exchanging a refresh token rotates
it, and a later refusal of a superseded token (`refresh_token_reused`) shows a
client that does not hold the current token is at most a grace window away from
holding nothing. The tool that owns a grant is the only one that may rotate it.

#### Expiry is read, never repaired

- **Codex**: from the `exp` claim of the access token. The signature is not
  verified: the proxy is reading its own credential to learn when it lapses, not
  deciding whether to trust it. An unreadable claim counts as expired.
- **Claude**: from the item's own `expiresAt`, in milliseconds, truncated to
  seconds. No token claim is consulted.

A grant within 60 seconds of its expiry is refused for the turn, and the refusal
says the owning program renews it. The next turn reads the profile again.

##### Why

A turn refused early costs one message; a turn started on a token that lapses
mid-request fails mid-request. Truncating milliseconds can only make a token
look older. Reading the profile every turn is how a refresh done by the owning
program arrives with nothing on this side noticing.

#### Credentials never reach argv, logs, or the configuration file

Credentials sit behind a `CredentialStore` trait. Keys are kept in a file
created `0600` on Unix; on Windows it inherits the per-user ACL of the
configuration directory. Grants are read through the same trait from wherever
the owning program keeps them.

### 8.1 More than one account

#### Two stores

The daemon's store composes borrowed profiles with the key file and serves
turns. The `FileStore` behind the key file holds keys, and holds grants written
by versions that obtained their own. The daemon's store refuses `add` and
`save` for a grant, and leaves grants in the key file out of what it lists
(§8.4).

Rules below about *writing* a grant (`add`, `save`, their collisions) are the
`FileStore` contract.

#### One account is selected

The selected account is the one every unpinned, untagged turn is made as.
`CredentialStore` reports only it; `AccountStore` reports which exist and which
is selected.

#### An account is identified by its account id and named by a label

The name is the operator's label where given, else the account id, else an
assigned `account-N`. Renaming never touches the credential. A name another
account holds is refused. Where one arrives anyway, a stored key and a profile
that appeared later under its name, neither serves that name until one is
renamed.

##### Why

An id is what the backend calls the account; a name is what the operator calls
it. Two entries answering to one name would hand turns to whichever was found
first. Nothing in a grant but the account id is an id, so no name is derived
from a token: that would be a fabricated fact and a secret in a printed field.

#### A login carrying an existing label renames; a label naming another account is refused

A login for an account already stored replaces that account's grant and never
adds a second entry. With a label, it renames the account; without one, it keeps
its name. A label that names a *different* account is refused.

##### Why

Taking the label would write the new grant over the one holding that name: a
silent retirement. The refusal costs the authorization just spent; the other way
costs a grant that may not be replaceable.

#### A write resolves its entry by account id

Only a grant carrying no account id falls back to the selection.

##### Why

A read, a network round trip, and a write can straddle a selection change. A
write aimed at whatever is selected *then* drops one account's grant into
another's entry.

#### The file is replaced, never truncated in place

The new content is written beside it under a name carrying the process id and
moved over it.

#### Every write holds a filesystem lock

The lock is held while the file is read, changed, and replaced. It is a separate
`.lock` file beside the credentials, never read or written, advisory, released by
the kernel when the descriptor closes, and never removed.

##### Why

Every write rewrites the whole file, so two overlapping writers would discard a
whole account. The pair that overlaps in practice is `accounts add-key` in the
CLI and the daemon writing for a verb of its own. The lock cannot be the
credential file, which is replaced by rename. Removing it would leave the next
two writers locking different files.

#### A filesystem that cannot lock cannot hold credentials

The write fails, naming the file and `PROXENOS_HOME` as the way to point the
directory somewhere local.

##### Why

The alternative is a write that reports success while doing what the lock
exists to stop. A home on a network mount is the case that exists.

#### A write that finds the file changed starts over, five times at most

After five attempts it errors.

##### Why

The lock reaches only writers that take it; an older binary or a hand edit takes
none, and the comparison catches those. Five consecutive losses is not
contention but something rewriting the file in a loop.

#### Accounts do not interfere with each other

Each holds its own refresh-token family, so rotating one leaves every other
where it was.

##### Why

That is a property of separate grants. What must be kept out of the design is
two holders of *one* account, where the last refresh retires the token every
other holder carries.

#### What the backend said about a credential is remembered per account

A status, the backend's sentence, and when it arrived; never the credential. Not
persisted. Details in §8.4.

#### Removing an account

Removing forgets one account and leaves the rest usable. Removing the last one
removes the file, so "not authenticated" is read from its absence. Removing what
is already gone is not an error.

#### The old single-grant file is read as one account

A credential file holding a bare grant is read as the account it describes,
named by its account id, and migrates on the next write, not on read.

#### An unresolvable selection

In the `FileStore`, a `selected` naming nothing stored falls back to the first
entry. The daemon's store refuses by name and points at `accounts use`.

##### Why

The key file still holds usable credentials, and "not authenticated" would send
an operator to re-authorize for nothing. The daemon's store selects across
borrowed profiles and keys, and picking the first of that is picking an identity
nobody named.

### 8.2 A credential that is not a subscription

#### A key is one secret, and nothing is invented beside it

A **grant** carries an expiry, an account id, and a plan. A **key** carries none,
and nothing reports a plausible value in their place.

##### Why

An invented expiry would drive a refresh that cannot happen. An invented account
id would put a header on the wire the key endpoint never asked for.

#### Every account verb works on either kind

List, use, rename, and remove. What differs is where the credential may be spent.

#### One place resolves a credential into headers

Both transports, the catalog fetch, the quota fetch, and the relay ask it.

| Credential | Headers |
|---|---|
| first-provider grant | `authorization: Bearer`, `originator`, `chatgpt-account-id` where the grant has one |
| second-provider grant | `authorization: Bearer`, `anthropic-beta: oauth-2025-04-20` |
| key, either provider | `authorization: Bearer` |

A transport holding no credential (the replay paths) sends `originator` alone.

##### Why

`originator` belongs to the subscription dialect, not to every request. The
second provider gates its OAuth grants behind that beta header.

#### A credential is refused at the other provider's or kind's endpoint

Before anything is sent, a credential is checked first for **provider**, then for
**kind**, and refused in a sentence naming both halves. The model list is paired
structurally: the daemon holds one endpoint per kind and picks by the credential
about to be spent.

##### Why

Every endpoint the transports are pointed at is the first provider's. A
second-provider grant or key would pass a kind check and come back refused in
words naming neither the account nor the endpoint. A key at a subscription
endpoint comes back as an invalid token, which sends the reader after the wrong
problem.

#### What follows for a key account

- **Never compressed.** zstd is measured against the subscription backend only
  (§4.4). An endpoint that does not decompress parses the bytes as JSON: observed
  live as a unicode decode error naming neither compression nor the endpoint.
- **HTTP only.** The WebSocket protocol belongs to the subscription backend.
- **No quota request.** The figure is a subscription entitlement, so asking is
  refused rather than spent.
- **No windows or efforts.** The key endpoint's model list is real and
  authoritative and states neither, so the window guard (§7.2) never fires and
  only the operator's effort ceiling applies (§2.7).

#### A key row states the absence of a ceiling, with the served count

A first-provider key's row says it has no ceiling and carries the tokens this
daemon served as it (§6.1), as a count with no cost.

##### Why

Every other missing figure is a figure pending. A key's is permanent *because
nothing bounds its spend*: it is metered per token. Reported as "no subscription
quota" alone, the one account that bills every token would read as the one with
nothing to watch.

#### A second-provider key is classified when stored

A second-provider key is either a subscription setup token or an API key, told
apart by its prefix when it is handed over (`sk-ant-oat` or `sk-ant-api`). Only
the classification is persisted.

| Classification | Row says |
|---|---|
| setup token | a figure pending (it rides relayed turns, §9.4) |
| API key | no ceiling, with the served count (zero, since every turn relays) |
| neither (no recognized prefix, or stored before classification) | this daemon has not recorded which meter it is on |

##### Why

The two are metered in opposite ways, and one sentence cannot be true of both. A
prefix is evidence, not proof, so an unrecognized shape is filed as neither
rather than the likelier one. A stored secret is never re-read to classify it.

#### The `sk-ant-oat` prefix is worn by two credentials, and the CLI says so

`claude setup-token` mints one valid about a year. The client's own OAuth access
token, in its keychain entry, has the same prefix and lasts hours. Stored as a
key, the second works until its expiry and then every turn is refused with no
field saying why: a key has no refresh. `accounts add-key` for an anthropic key
with that prefix names both credentials on stderr **where stdin is a terminal**,
naming the prefix and never any part of the key. Where stdin is a pipe it says
nothing.

##### Why

Decoding a bearer to classify it is a new way for a secret to reach a log. The
terminal is the one moment a person is present. A scripted login's output is
read by something (`api.md`, `accounts add-key`). A subscription that should
refresh is borrowed from its profile instead (§8.4).

#### A CLI login hands over to a running daemon

##### Why

The daemon reads the store on every request, but conversations bound to the
previous account keep their connections; after a change of kind that endpoint
refuses every turn. No daemon running is the ordinary case and not a failure. A
file edited by hand gets no handover.

#### A key is read from stdin, under a name the operator gives

When stdin is a terminal, a prompt goes to stderr first. A piped key is read as
sent, with surrounding whitespace trimmed: the newline `echo` adds is never part
of a key.

##### Why

A command line is visible to every process and lands in shell history. An
unprompted terminal read is indistinguishable from a hang.

#### Neither kind is stored over the other

A key written where a grant is, or a grant (or a login label) where a key is, is
refused. A key over a key of a **different provider** is refused; over a key of
the same provider it rotates in place. A rotation whose account is no longer
stored is refused rather than appended. An entry with no kind recorded is a
grant.

##### Why

Each is a silent loss of a working credential. A key entry carries no account id,
so the grant collision rule cannot see it. Appending a rotation would create an
account nobody asked for and make it serve turns.

### 8.3 A quota belongs to one account

#### One figure per account

A figure is filed under the account that served the turn it rode in on: the
tagged or pinned account, else the serving account, resolved to its name at
that moment. Figures of different accounts sit side by side.

##### Why

Two accounts can serve one session. A single latest snapshot would let a
cheap-tier turn on a pinned account overwrite the headroom of the subscription
the operator is watching.

#### Freshness is stated per account, and nothing is aged into an estimate

Each figure carries how it was obtained (a turn, or asked for) and when. What
the provider said is reported as said.

#### A figure can be asked for per account

`usage.refresh` asks once per account, each on its own credential. Which account
serves turns is neither read nor changed.

##### Why

Riding a turn only ever fills in the account that made the turn. A spare
account's headroom is what decides whether to switch to it, and requiring a
switch to learn it is the question answering itself.

#### Only where a figure is possible, and each failure is one account's

A key account is not asked (§8.2). A second-provider key has no endpoint that
answers and is not asked (§9.4). An account that is asked and does not answer
says so on its own row only.

#### The daemon-wide line answers for the serving account

Where a single account is held, the daemon-wide line is the whole answer, and
its reason is that account's own, by the rules of its row.

##### Why

A generic "no turn has been made yet" under a lone key account promises a
figure that will never arrive.

#### Absent stays absent

A window the provider did not report is omitted, not rendered as zero. An
account with no figure says this daemon has none (§6.1).

#### A window carries the provider's own words

Where the provider states a per-window status, a threshold, or which window is
representative, each is parsed and reported and none is inferred from the
percentage. Only an outright refusal sets `limit_reached`; `allowed_warning` is
a turn that went through, carried beside the figure. The window the provider
names as deciding is marked.

##### Why

An account can sit at 93% on a window already flagged `allowed_warning`. With
one window near empty and another near full, an unmarked list reads whichever
line comes first, and that is the reassuring one.

#### A window the provider named rather than measured is kept under its name

An overage window has a figure and a reset but no duration, so it is carried
under the provider's word for it. A named window a newer snapshot does not
restate is kept from the older one until its reset; one that states no reset is
not kept, since nothing would ever drop it.

#### Staleness belongs to a window, never to a snapshot

A window whose reset epoch has passed is marked stale while the daemon runs. A
window with no stated reset is never called stale in a running daemon, and is
not restored across a restart (§6.1).

##### Why

This proxy learns a new figure only when a turn is made. One snapshot can hold a
passed five-hour window beside a live seven-day one, so marking the snapshot is
wrong in both directions. The error prevented is the overstating one: spend shown
against an empty window sends an operator to switch accounts for nothing.

#### An absence says how far this daemon can see

"No turn has been relayed as this account" is phrased as none reaching this
daemon. A key account's absence states what it does not cover.

##### Why

`doctor --live --probe relay` builds its own store and spends the account for
real, leaving no figure here. A key's spend is metered per token, so an absence
stated alone would read as safety.

#### What a select and a removal invalidate

A **select** keeps every named figure and drops only a figure no account could
be named for. A **removal** drops the removed account's figure and its tally.

##### Why

A named figure still describes its account. An unnamed one would read as the
newly selected account's headroom. A removed account can no longer be spent.

### 8.4 A grant this process does not own

#### A profile directory is the identity

A borrowed grant lives in another program's profile: a `CODEX_HOME` for the
ChatGPT app and `codex`, a `CLAUDE_CONFIG_DIR` for the client. Choosing which
account pays is choosing which directory to read.

#### A borrowed grant is read, never written, never refreshed

An expired grant is reported as expired. Every refusal names the store it read
and the remedy, which differs by provider: the ChatGPT app or `codex login` for
one, running the client for the other.

##### Why

The refresh token is single-use: exchanging it rotates the stored value and the
previous one is refused afterwards. Doing that here logs the operator out of the
owning program, with the symptom appearing there.

#### Codex: `auth.json`

One grant per `CODEX_HOME`. Expiry is the access token's claim (§8). The account
id is `tokens.account_id`, falling back to the id token's `chatgpt_account_id`
claim; on three signed-in profiles the two were equal, and the field is preferred
because the owning program writes it deliberately. An empty access or refresh
token is refused by name.

A profile whose `auth_mode` is set to anything other than `chatgpt` is refused,
and the mode is checked before the tokens. An absent or empty `auth_mode` is
accepted.

##### Why

An API-key mode authenticates at another endpoint with other billing, and can
still carry a stale `tokens` block from a replaced sign-in. The field is not
always written, and its absence says nothing about the mode.

#### Claude on macOS: which keychain item depends on whether the variable is set

Unset gives `Claude Code-credentials`. Set gives
`Claude Code-credentials-<sha256(value)[..8]>`, over the value verbatim, even
when it names the directory the bare name describes.

##### Why

Three spellings of one directory produced three items (measured), so nothing
canonicalizes; canonicalizing would name an item the client never writes.

#### Claude on macOS: the item is read by spawning `security`

##### Why

The item's ACL trusts that binary. A process reading through Security.framework
is a different application and is prompted, and one client run reads the item
sixteen times.

#### Claude on macOS: the file is read when the keychain says nothing

The keychain item is tried first, then `.credentials.json` in the profile
directory. A missing item (`security` exits 44) and an unreadable keychain both
fall through to the file. Where the file answers, the keychain's failure is
logged at `debug`. Where neither answers, the keychain's failure is carried into
the refusal, which names both places.

##### Why

A daemon started as a system LaunchDaemon has no security session, so the item
reads as absent; with `SessionCreate` the login keychain is locked instead, and
unlocking it wants a password at every boot. The file holds the same JSON. A
keychain this process cannot reach and a profile nobody signed into want
different remedies.

#### Claude on Linux: `.credentials.json`; elsewhere, refused

On Linux the grant is `.credentials.json` in the profile directory. On any other
platform the daemon starts and refuses at the first profile needing a location,
naming the platform. A configuration of keys only is unaffected.

##### Why

Guessing a location reports a profile as never signed into for a reason of our
own making. Refusing at startup would refuse a valid configuration.

#### What is measured, and where

The keychain rules (item names, the digest over the verbatim value, the sixteen
reads, the blanked item) were observed on macOS against signed-in profiles, and
their tests run there. The Linux location comes from the client, not from a
machine: the parsing is exercised end to end, the location is unproven. On macOS
the fall-through to the file is measured; that the client writes that file there
is not.

#### A blanked item is a refusal

An item with an empty access token and zero expiry is refused by name.

##### Why

When the client fails to refresh it blanks the item rather than removing it,
which is indistinguishable from a profile nobody signed into and wants the same
answer.

#### A lapsed grant is renewed only when a quota figure is asked for

`usage --refresh` on a lapsed grant runs the owning program once, waits, and
reads the profile again. A turn never does: it refuses and names the program.

- Claude: `claude -p ok --model haiku`.
- Codex: `codex exec --skip-git-repo-check ok`, with no model id.

stdin is closed, and the run is killed after 60 seconds, leaving the profile
alone. The program is `claude_program` or `codex_program`, or the bare name
resolved through the daemon's `PATH`. `claude_program` also supplies the version
the second provider's quota request is made as.

##### Why

The rotation must happen inside the owning program. A turn that waited for a
client to start would spend a minute before its first byte. Without closed stdin
the client waits several seconds for input. `--skip-git-repo-check` because the
daemon's working directory is not a repository. A model id names one plan's
catalog and would go stale. A launchd daemon inherits almost no `PATH`, so there
the bare name fails with `could not run` until the path is written out
(`api.md` §4).

A borrowed profile no other session drives is never rotated by use, since a turn
through this proxy spends the token without rotating it. A Codex token lasts ten
days, so its case is rarer, not absent.

That a `codex exec` turn rotates a genuinely lapsed grant is **derived, not
confirmed**: a Codex access token is a signed JWT that cannot be backdated to
force the case (`roadmap.md` §L). The Claude path is confirmed.

##### Tried and dropped

Never running Codex, because its refresh spends quota and one failing run sent
fourteen refresh requests. That traded a standing account for a fraction of a
turn; the deadline bounds the failing case.

#### A profile whose refresh token has lapsed is never run

`refreshTokenExpiresAt` in the past means no run. Where it was never recorded,
the profile is run anyway.

##### Why

A failed client refresh blanks what is left of the grant. Unknown is not dead,
and a dead grant needs a new sign-in either way.

#### One run per sweep, and one run per profile at a time

A `usage --refresh` sweep spends at most one client run, on whichever account
needs it first; the rest are asked without a refresh and say they were not
refreshed. A run holds a per-profile lock for its whole duration, released on
failure too.

##### Why

A per-account bound is no bound: four lapsed profiles would be four minutes with
nothing to time the caller out. Ten callers at once produce one client: the rest
wait, then read what it wrote. A lock outliving a failure would make the next
caller wait for a run that is not happening.

#### Which profile serves is this daemon's only write about a borrowed account

It is kept beside the other daemon state, not in the configuration document.

##### Why

`accounts use` is a runtime verb, and the document is the operator's.

#### One declared profile serves unchosen; several need a choice

A selection naming a deleted entry is refused by name.

##### Why

The choice decides whose subscription pays. Resolving it to the first entry
spends the wrong one invisibly.

#### A declared profile is listed whatever its state, with why it is unreadable

A row that cannot be read carries `unreadable` with the refusal's own words: the
store tried and the remedy. The field is absent on a readable row and on every
key, and never carries any part of what the store holds. The table shows
`unreadable` in the state column and prints the reason under the row.

##### Why

Dropping a row reads as an entry the operator never wrote. Absence alone cannot
separate a profile nobody signed into from an unreachable keychain from a file
holding something that is not a grant, and a listing of empty columns showed
state `ok`.

#### Adding, renaming, and saving a profile are refused

The refusal names the owning program for what is inside the profile, and the
configuration file for this daemon's view of it.

#### Removing a declared profile deletes its `[profiles]` entry

The grant stays where it is. The configuration is re-read afterwards. A
discovered profile has no entry to delete and is refused, saying it was found
and that `[profiles]` is empty.

#### With nothing declared, each program's stock profile is read

`[profiles]` empty means read the profile each client uses with no variable set;
each that holds a grant is an account. A discovered profile without a grant is
not listed. Writing any entry replaces the discovered set entirely. The listing
says which set it shows.

##### Why

A first run should not make an operator write down what the programs already
know. An entry is the operator's statement about who pays, and a discovered
profile beside it would be a second opinion nobody asked for.

#### A second profile is signed in by the program that will own it

`accounts login` creates a directory, runs that program's own login with the
variable naming it, and reads the profile afterwards. Nothing here sees a token,
and a directory with no grant is declared nowhere.

- Declaring the first profile also writes down the discovered profiles that hold
  a grant.
- A directory already signed in is adopted without running anything.
- With no terminal, the command is printed with its variable attached.
- `--relogin` signs a declared profile in again: the name must be in
  `[profiles]`, the provider must match, and the directory is resolved exactly
  as the daemon reads it (no path means the stock profile, run with no
  variable). The configuration file is not opened.
- `--device-auth` asks `codex login` for a URL and code to use in a browser
  elsewhere. The other provider refuses it. Where the command is printed, the
  flag is on both printed lines.

##### Why

A written entry stops discovery, so a first login would otherwise subtract the
accounts the operator already had. A lapsed grant still reads as a grant and
would be adopted with nothing run, which is what `--relogin` exists for. Handing
an unknown flag to a client that rejects it as a misspelling helps nobody.

#### A credential the backend refuses is remembered against the account that spent it

A 401 or 403 from the backend records a status, the backend's sentence, and
when, under the account that served the turn. Any other backend answer clears
it. A refusal this side made before sending anything is not recorded.

##### Why

For a Codex profile it is the only signal that a sign-in is needed: `auth.json`
records no date, and `codex login status` reports "logged in" for a profile with
junk tokens (measured). A rate limit is not a login problem, and a warning that
outlives the problem sends an operator to renew what works. A lapsed grant this
daemon could not read is not a backend refusal.

#### A Claude login due within seven days is announced

From `refreshTokenExpiresAt`: `accounts` shows the count on the row and `status`
adds the remedy, within seven days only. A Codex profile states nothing, because
no field says when renewing stops working.

##### Why

Past that date the client cannot refresh either, and trying blanks the grant, so
without the notice the first sign is a grant that emptied itself. A date shown
all year is one the reader learns to skip.

#### A grant left in the key file is skipped, and named as skipped

##### Why

Nothing here obtains or refreshes one now. A credential that quietly stopped
counting reads as one that vanished.

#### A borrowed Claude grant has a quota endpoint

`GET /api/oauth/usage` answers it with named windows (`five_hour`, `seven_day`),
utilisation already in percent, RFC 3339 resets, a `limits` array with the
provider's severity per window, and `spend`. A setup token is refused there for
want of a scope, which is why that credential's quota comes from turn headers
alone (§9.4). Nothing in the body says a turn would be refused, so nothing
derived from it claims one would.

#### The credit balance is read from `spend`

Read only where `spend.enabled` is true. Amounts are minor units with an
exponent and a currency; the provider's percentage and severity are used as
given. An unreadable `used`, a missing exponent, or mismatched currencies or
exponents yield no credit. `extra_usage` is not parsed.

##### Why

Once plan windows are full, turns draw on the credit; measured, an account sat at
6% of its five-hour window and 98% of its credit. The stated percentage is what
the provider acts on (the amounts work out to 97.73%). A zero in place of an
unreadable amount is the reassuring error, and a guessed exponent misstates
money by orders of magnitude. `extra_usage` is the same figure in float cents,
and two answers to one question can disagree.

#### A header states an overage window; the endpoint states the balance

A relayed turn's headers carry an overage window (a percentage and a reset) and
no money, so a snapshot read from headers carries no credit.

#### A model-scoped limit is its own window

A `limits` entry whose `scope` names a model is kept as a window labeled with the
model's display name, with its own percentage, severity, and reset, and no
duration.

##### Why

Measured, a scoped entry sat at 16% against `weekly_all` at 49%. Folding either
into the other misstates both, and a duration would put a second seven-day
window where a lookup expects the account's.

#### The plan and subscription status come from the profile endpoint

Asked at most hourly. The organization type gives the plan, and for a max
organization the rate-limit tier gives the multiplier (`default_claude_max_20x`
renders `max 20x`). An unrecognized type yields no plan.
`organization.subscription_status` is remembered with the plan. `active`, or
nothing stated, is silent; any other value is shown verbatim on the account's
row and in the JSON.

##### Why

A plan changes on the scale of billing. A cancelled subscription keeps serving
quota that looks untouched while every turn is refused. Only `active` has been
observed, and sorting the rest into an invented vocabulary would put words in
the provider's mouth.

#### A credential names its provider as well as its kind

##### Why

A borrowed second-provider grant is a subscription credential that must never
reach the first provider's backend, and the relay asks about provider: a grant
and a key there are spent at the same endpoint. The second provider's quota is
asked under the owning client's user-agent.

#### Who pays is said on every surface that has room

Each listing carries the store it was read from, and the identity the credential
holds travels beside the label: the status line receives the serving account
with or without a quota figure, a launch prints it once before the client
starts, and `accounts` names the profile behind each row.

##### Why

The name in the configuration is the operator's label, and nothing about it is
the account.

#### A profile that has become a different account is marked

The identity is recorded when the profile is chosen. A later read finding a
different one says so on the serving row and in the launch line. An unreadable
profile is never marked.

##### Why

This is the one failure borrowing introduces: the operator signs the owning
program in as somebody else, the directory keeps its name, and every turn is
billed to an account nobody pointed at.

### Not done, on purpose

- No authorization flow, callback port, or setup-token login of this proxy's own
  (§8).
- No refresh of a borrowed grant by exchanging its token (§8.4).
- No keychain read through Security.framework (§8.4).
- No unlocked fallback for a filesystem that cannot lock (§8.1).
- No borrowed-profile location on Windows (§8.4).

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/auth/store.rs` | `CredentialStore`, `AccountStore`, `FileStore` (lock, replace, five attempts), key classification |
| `crates/proxy/src/auth/borrowed/mod.rs` | Parsing Codex `auth.json` and the Claude item, platform locations |
| `crates/proxy/src/auth/borrowed/read.rs` | The `security` spawn, the file fallback |
| `crates/proxy/src/auth/borrowed/store.rs` | The daemon's composed store: profiles plus keys, selection, discovery |
| `crates/proxy/src/auth/borrowed/poke.rs` | Running the owning program, the deadline, the per-profile lock |
| `crates/proxy/src/auth/grants.rs` | The expiry margin and the expired-grant refusal |
| `crates/proxy/src/auth/authorize.rs` | Headers per credential, provider and kind checks |
| `crates/proxy/src/auth/jwt.rs` | Reading `exp` and the account id claim |
| `crates/proxy/src/auth/refusals.rs` | Backend refusals remembered per account |
| `crates/proxy/src/auth/profile_login.rs` | `accounts login`, `--relogin`, `--device-auth` |
| `crates/proxy/src/auth/key_login.rs` | `accounts add-key`, the stdin prompt, the `sk-ant-oat` caution |
| `crates/proxy/src/usage.rs` | Per-account snapshots, the usage and profile endpoint parsers, credit |
| `crates/proxy/src/render/mod.rs` | The seven-day renewal notice |
| `crates/proxy/tests/borrowed.rs`, `crates/proxy/tests/credentials.rs` | Profile reading and store contracts |

---

## 9. The second provider

#### A turn for the second provider is relayed, not translated

It is forwarded as it arrived and streamed back as it returns. Confirmed live:
plain and streaming, generation and refusal, round-trip against the real
endpoint with a subscription bearer substituted. The endpoint wants the client's
own identity shape (its beta list, `x-app`, its system prompt), which the client
always sends and this path forwards.

#### The body is relayed verbatim

Routing is decided from the raw bytes, reading only the model id, before
anything is parsed. The bytes that arrive are the bytes that leave, and the
response body streams back untouched.

##### Why

This is a rule, not an observation, because the obvious implementation breaks it
quietly: round-tripping the body through this proxy's types drops every field
they do not model, somewhere no test looks.

The `relay` probe (§10.3) holds the rule. Its marker sits in a field this proxy
has no type for, so a re-encoded body fails it even though the turn succeeds.
Replayed, a stand-in backend records what it was sent and both halves are
checked. Live, only the answer half is, and the row says so. The probe
authorizes as a named account read from its own store, so the serving selection
is neither read nor changed.

#### Nothing from §2 or §3 applies

No instruction parts, no tier rewrite, no tool flattening, no effort cap, no
window guard, no baseline, no delta, no session state.

##### Why

The client sends the whole conversation every turn and the backend reads it.

### 9.1 Routing

#### A tagged turn belongs to its tag

A session started with `exec --account` carries the account name in the auth
token (`api.md` §2.3). A tagged turn is relayed when that account is on the
second provider and translated as that account otherwise; the mapping's claims
are not consulted. A tag naming nothing stored is refused before anything is
spent.

##### Why

The tag is the launch's word on who pays. Serving an unknown tag as whoever is
selected would spend a subscription nobody named.

#### An untagged turn routes by model id

The body's id is looked up among the mapped upstream ids:

- an id whose mapping's account (the pin, or the serving account where unpinned)
  is on the second provider is relayed as that account;
- an id no mapping claims for the second provider, whose authenticating account
  (the tier's pin, else the serving account) is on the second provider, is
  relayed as that account;
- anything else translates.

##### Why

By model id rather than tier name, because this path never rewrites the model:
`env` and `exec` hand the client final ids at launch, so what arrives is what the
backend must see.

#### A turn never authenticates against a backend its account was not stored for

##### Why

Translation spends a credential at the first provider's backend. Relaying an
unmapped id for a second-provider account keeps the credential at its own
endpoint, and that provider judges the id. A launch-time model override
therefore works on either provider with no mapping edit. Crossing providers
still takes a pin or a changed selection: serving an id from an account nobody
named would spend a subscription nobody pointed at the turn.

An operator who selects a second-provider key has said where turns go. Ignoring
that would send every turn to the other endpoint, refused there as a credential
of the wrong kind.

#### A tagged turn that translates uses the tagged account's mapping

It resolves the shared `[tiers]` with `[accounts.<name>.tiers]` over it, as a
switch to that account would. A tag naming the selected account keeps the
mapping in force, including anything `tiers.set` moved without persisting. The
effort ceiling is not re-resolved. None of this touches a relayed turn.

##### Why

The mapping in force is the selection's. A tagged turn reading it asked the
tagged account's credential for a model stated for somebody else, and was
refused for a reason the mapping never mentions.

#### One model id may be claimed by at most one account

Where mappings naming one id resolve to more than one account and at least one
of them relays, the turn is refused, naming the id and every claimant.

##### Why

Nothing in a request says which account it belongs to, and picking spends a
subscription silently. Two first-provider tiers sharing an upstream model decide
nothing and are not refused.

#### A pin naming an unknown account falls through to translation

§7.1 refuses it there by name.

##### Why

One mistake, one message.

#### A relayed tier is not validated against the first provider's catalog

The exclusion holds at start, `tiers.set`, and a switch, beside §7.1's exclusion
of pinned tiers, and relayed tiers are left out of the withheld-model report
(`api.md` §3).

##### Why

An id on this path is absent from that list by construction.

### 9.2 Headers

#### The request headers

- **`authorization` is replaced** with the account's credential headers (§8.2).
  The client's bearer is a placeholder, or a launch tag read for routing.
- **A borrowed grant adds `anthropic-beta: oauth-2025-04-20`**, as an additional
  header line beside the client's own `anthropic-beta`.
- **`x-api-key` is dropped.** A turn authenticated as whatever the caller held is
  a turn this proxy did not route.
- **Hop-by-hop, `host`, `content-length`, and `accept-encoding` are dropped.**
  They describe this hop, and this path does not decode a content coding, so
  asking for one would relay bytes the client never agreed to.
- **Everything else passes through**, `anthropic-version`, `anthropic-beta`,
  and the client's identifying headers included.

##### Why

The beta list is the client's statement about what it can parse in the reply;
editing it changes what comes back. The OAuth beta is what the endpoint gates a
grant behind (measured: grant plus header answers 200).

#### The query string is forwarded exactly

`?beta=true` is observed live. None is invented where the client sent none.

#### The response headers pass through, less hop-by-hop

The response status and every response header not in the hop-by-hop set reach
the client as sent.

### 9.3 Errors

#### An upstream refusal passes through untouched

Status and body arrive as sent.

##### Why

It is already an Anthropic error. Rewrapping restates a message the backend
wrote, and a rewrap that loses the error type takes the client's own retry logic
with it (`api.md` §1.1).

#### This proxy's own refusals use its own shape

An ambiguous model id, an unknown tag, an unreadable account, or a credential of
the wrong provider is a refusal in the shape of `api.md` §1.1. A connection that
never opened is `overloaded_error`, retryable because nothing was sent.

### 9.4 What a relayed turn leaves behind

#### Ingress capture records the bytes that were relayed

Headers go through the same redaction by name as everywhere else. A body that
cannot be held as raw JSON is not captured, and the turn goes anyway.

##### Why

A capture rebuilt from this proxy's types is a fixture that still replays and
passes while being wrong about every field those types do not model. Capture
never changes the turn.

#### The model id joins the served list

The id the client sent is added to the served list the quota answer states
(`api.md` §2), as on the translating path.

##### Why

A client keeps the ids it was launched with, so after a mid-run remap the mapping
names an id no running session sends, and a status line reading the mapping
would stop recognizing the session.

#### The response's quota headers become this account's figure

`anthropic-ratelimit-unified-*` response headers are parsed into a snapshot filed
under the account that made the turn. They are the same names the translating
path puts on its own responses (`api.md`).

- Utilization is a fraction on the wire and a percentage in the snapshot; that is
  the only arithmetic.
- The plan is absent: no header states one.
- `allowed_warning` is a turn that went through; only `rejected` is the limit
  reached.
- A response with no quota header yields no snapshot, not an empty one.

##### Why

For a setup token the headers are the only place the provider states headroom:
its usage endpoint refuses that credential, so `usage.refresh` does not ask for
it (§8.3). Reading them costs nothing. An empty snapshot reads as "quota known,
nothing used", the reassuring error.

#### A backend refusal of the credential is noted

A 401 or 403 on a relayed turn is recorded against the account (§8.4); any other
status clears it.

### Where it lives

| File | What it holds |
|---|---|
| `crates/proxy/src/upstream/relay.rs` | `Relay::forward`, `relayed_by`, `account_for`, `HOP_BY_HOP`, `validated_models` |
| `crates/proxy/src/ingress.rs` | The routing block in `messages`, relay capture, header snapshot, `note_credential` |
| `crates/proxy/src/policy.rs` | `snapshot_for`: a tagged account's mapping |
| `crates/proxy/src/usage.rs` | `Snapshot::from_headers`, `Snapshot::headers`, `record_model` |
| `crates/proxy/src/auth/authorize.rs` | `ANTHROPIC_OAUTH_BETA`, `for_provider` |
| `crates/proxy/tests/relay.rs`, `crates/proxy/tests/routing.rs` | Verbatim relay and routing rules |

---

## 10. Testing

Development is test-first. No test touches the network.

### 10.1 Translation

#### Every translation rule is a pure function, specified by a failing test first

Table-driven cases cover mappings; snapshots cover emitted frame sequences.

### 10.2 Upstream contract

#### Ground truth is captured, never invented

What the backend sends is recorded first, becomes a fixture, and the fixture
becomes the failing test.

##### Why

The test's content comes from observation rather than imagination, and a
fixture's payload has to be real where the probe turns on it: a probe whose
recording was written to pass tests only the recording.

### 10.3 Capabilities

#### A capability test turns on content the model could not infer

Random codes, verbatim strings.

##### Why

A model handed nothing describes a file confidently from its name, and that
output is indistinguishable from success. Plausibility is never evidence.

#### The matrix says what it did not touch

A failed row prints the probe's rationale. A live run marks the `count-tokens`
row, whose surface never reaches the backend the live header speaks for. One
line reports each path in one of three states:

- **exercised**: a row on it passed; only then is the account it spent named;
- **reached, established nothing**: its probes ran and all failed;
- **not exercised**: nothing ran on it, or every row was skipped.

Each state is a heading, each path appears under exactly one, and an empty
heading is not printed, except `Not exercised:`, which always lists the
WebSocket transport.

##### Why

Green rows say nothing about a path nothing drove, and a reader with no line to
say otherwise reads green as coverage of the whole proxy. Overstating and
understating are the same defect, so a run that did drive translation says so.

### 10.4 Transport and sessions

#### Transports are tested against a local replay server

Coverage includes connection reuse, prewarm, and fallback latching.
Cancellation (§5.3) is covered on the HTTP replay path only.

#### Incremental upload is specified by its invariants

- a valid delta contains exactly the new items
- any change to a non-input field forces a full send
- a non-extending input forces a full send
- server-returned items are never resent
- a full send is always valid

### Where it lives

| File | What it holds |
|---|---|
| `crates/core/tests/request_translation.rs` | Request mapping cases |
| `crates/core/tests/response_snapshots.rs`, `crates/core/tests/snapshots/` | Frame sequence snapshots |
| `crates/core/tests/corpus.rs`, `fixtures/` | The recorded fixture corpus |
| `crates/proxy/src/probe.rs` | Capability probes and the exercised-paths line |
| `crates/proxy/src/doctor.rs` | The matrix `doctor` prints |
| `crates/proxy/tests/transports.rs`, `crates/proxy/tests/replay.rs` | Transport tests against replayed exchanges |
| `crates/proxy/tests/ingress.rs` | Cancellation on the HTTP path |
