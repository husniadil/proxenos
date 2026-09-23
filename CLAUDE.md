# proxenos

Claude Code on other providers' models, without modifying Claude Code. One
daemon exposes an Anthropic Messages surface and serves each turn through the
provider of the account serving it:

- **Translate path (Codex):** Messages ⇄ OpenAI Responses, over WebSocket or
  HTTP + SSE. The translation's real job is keeping Claude Code's built-in
  tools working.
- **Relay path (Anthropic):** Messages forwarded untranslated
  (`upstream/relay.rs`, `docs/proxy-behavior.md` §9).

## Commands

- `just test`: the suite, all features. Run it on every edit.
- `just check`: **the gate**, and what CI runs: `cargo fmt --check`, `clippy
  -D warnings`, and the suite. Run it before every commit.
- `just test-one <filter>`: one test filter.
- `just snapshots`: review pending `insta` snapshot changes.
- `just run` / `just record` / `just doctor`: run the daemon, capture
  fixtures, probe capabilities.
- `just setup` / `just build` / `just fmt`: tooling, release build, format.

**No test touches the network.** Every upstream interaction in the suite runs
against a local replay server, so the suite is green without credentials or
quota. A test that needs a live backend stops running the moment quota runs
out; that is a design constraint, not a convenience.

Only `just doctor --live`, `just record upstream`, and `just record surface`
spend quota, and none is part of the gate. Plain `just doctor` answers from the
fixture corpus; `just record ingress` captures what the client sends and costs
nothing. `record surface` spends a turn per exchange, so use `--only` to add
one shape without re-buying the rest.

## Docs that bind the code

- [`docs/proxy-behavior.md`](docs/proxy-behavior.md): the normative spec for
  translation, transport, sessions, credentials, and token accounting. The code
  is measured against it; most rules exist because the obvious implementation
  fails silently. **Read the relevant section before touching any of those.**
- [`docs/api.md`](docs/api.md): what the proxy exposes: ingress, error
  vocabulary, CLI, control socket, configuration, and what is semver-bound (§6).
- [`docs/roadmap.md`](docs/roadmap.md): current state, the intended work under
  `### Next` (named, never numbered), and §L, the questions only a live backend
  settles. Release history lives in `CHANGELOG.md`, nowhere else.
- [`skills/proxenos/SKILL.md`](skills/proxenos/SKILL.md) and
  [`herdr-plugin/`](herdr-plugin/) quote the CLI. A verb, flag, or config key
  that moves, moves in both in the same commit.

If implementation disproves a spec rule, change the spec in the same commit as
the code that proved it. A drifted spec is worse than none, because it is still
believed. No dates in specs or docs.

## Non-negotiables

1. **Harness fidelity is the product.** Not a generic protocol translator. It
   keeps `Read`, `WebSearch`, `WebFetch`, tool search, and the context meter
   behaving as Claude Code expects. Each fails by returning plausible output
   instead of an error (an empty search that reads as "no results", a file
   described from its name). Never trade any of them for convenience.

2. **Never fabricate a number upstream can supply.** Token counts from a
   completed response are authoritative and never recomputed.
   `cache_creation_input_tokens` is zero because no write event exists, and
   stays zero rather than synthesized. Where a figure is genuinely unavailable
   (the two points in `proxy-behavior.md` §6.2), it is estimated, corrected
   against ground truth within the same exchange, and documented as an
   estimate. The same holds for claims: a capability verified against replayed
   fixtures is derived, not confirmed, and says so. `docs/roadmap.md` §L holds
   what only a live backend can settle.

3. **Falling back is always safe; a wrong delta is not.** Incremental upload
   asserts the conversation strictly extends what was sent before. When that
   cannot be proven, send everything. A full send costs bandwidth; a wrong
   delta silently corrupts a conversation. Every ambiguous case resolves toward
   the full send.

4. **A capability claim needs an unguessable probe.** A model handed no file
   describes one confidently from its filename, indistinguishable from success.
   Any test asserting an attachment, a search result, or a tool round-trip must
   turn on content the model could not infer: random codes, verbatim strings.
   Plausibility is never evidence.

5. **Anthropic error shapes, always.** Every failure leaves as
   `{"type":"error","error":{"type":...}}` with a type Claude Code's retry logic
   understands. Transient surfaces as retryable, terminal as terminal. No second
   retry loop on top of the client's.

6. **Two doors, one daemon. No telemetry.**
   - `127.0.0.1` is **always** bound and authenticates nothing: every caller
     there is a local process running as the user, and every local session's
     `ANTHROPIC_BASE_URL` names it with no token.
   - A reachable `listen.address` (`api.md` §4) opens a **second** listener
     over the same state, where every request must carry the token. A
     non-loopback address with no token is **refused at startup**, naming both
     keys. A wildcard is refused, because it cannot be split into two doors.
   - **The token belongs to the door, not the peer.** Nothing reads a
     request's source address to decide: a peer-keyed guard is untestable from
     one machine and exempts everyone behind a reverse proxy.
   - `ANTHROPIC_AUTH_TOKEN` is the one header the client offers, and carries
     both things read from it, whitespace-separated: `proxenos-account:<name>`
     (the `exec --account` launch tag) and `proxenos-token:<secret>`. A value
     with no token part is read whole, as the tag alone. A tag is a name; the
     credential it resolves to never leaves the daemon. The token is a secret and never
     reaches argv, a log line, or what `status`, `env`, or `settings` print.
   - Nothing is collected, nothing is transmitted.

7. **Credentials never reach argv or logs.** A key arrives on stdin and lives
   behind `CredentialStore`, in files created `0600`. A subscription grant is
   borrowed, not held: it stays in the profile directory of the program that
   signed in, and is read there. The configuration file never holds one;
   `[profiles]` names a directory, never a secret.

## Working agreements

- **Test-first.** Failing test, then the code that passes it, then refactor.
  Translation rules are pure functions over data; the expected output is a
  specification.
- **Upstream behavior is captured, never guessed.** Record it (`just record`),
  make the recording a fixture, write the failing test against it, implement.
- **Spike before proposing a fix to measured behavior.** A probe that falsifies
  the idea is a good outcome; a confident guess is not.
- **Commit at checkpoints.** Small, working, independently revertible.

## Layering

```
ingress ──── Anthropic Messages surface (axum)
                        │
core ─────── translation: Messages ⇄ Responses
             pure functions and state machines, no I/O
                        │
session ───── per-conversation state
             input baseline · transport binding · calibration
                        │
upstream ──── WebSocket │ HTTP + SSE │ relay
                        │
auth ──────── borrowed grants, stored keys, CredentialStore
```

- `crates/core` (`proxenos-core`) holds the middle layer only: no sockets, no
  clock, no filesystem, no configuration policy. That boundary keeps every
  translation rule testable as a pure function over recorded data. A rule that
  needs I/O to test is in the wrong crate.
- Transports are interchangeable below `session`. WebSocket is primary and HTTP
  its fallback, but HTTP is a normal operating mode, not an error path. The
  backend is documented to close sockets under policy conditions; no such
  close has been seen on the account tested, and one account is not evidence
  about all, so the fallback is covered as an ordinary path.
- Compression applies to both: zstd on an HTTP body, `permessage-deflate` on
  the socket. It saves bytes, never tokens.

## Map

### `crates/core/src` (pure)

- `anthropic/`: the Messages surface; `stream.rs` events, `aggregate.rs` folds
  a stream into one non-streaming body.
- `responses.rs`: the Responses surface as the backend accepts it.
- `translate/`: §2 and §5, `request.rs`, `response.rs`, `schema.rs`.
- `session.rs`: §3 and §4.3, conversation identity and the delta predicate.
- `sse.rs`: §5.0, SSE framing.
- `fixture.rs`: the recorded-exchange format the suite replays.

### `crates/proxy/src` (daemon and CLI)

- Serving: `daemon.rs` (binding both doors), `ingress.rs` (the Messages
  surface, token and account tags), `error.rs` (error vocabulary), `session.rs`
  (per-conversation state).
- Upstream: `upstream/` (`conduit.rs` chooses and keeps a transport,
  `websocket.rs`, `http.rs`, `pool.rs` connection reuse, `compression.rs`,
  `relay.rs`).
- Accounting and models: `estimate.rs` (§6.2, §6.3), `usage.rs` (quota),
  `catalog.rs` (§7.0), `incidents.rs` (provider status pages).
- Credentials: `auth/` (store, selection, authorize, key and profile login),
  `auth/borrowed/` (profiles read, never owned; `poke.rs` asks
  the owning program to refresh).
- Configuration: `config.rs`, `config/edit.rs` (one value, file kept as a
  document), `policy.rs` (what a running daemon can change).
- Operator surface: `cli.rs` (verbs, semver-bound), `commands/` (one module
  per verb family), `control/` (control socket), `render/` (terminal output
  only), `statusline.rs`, `version.rs`.
- Launching and hosting: `launch.rs` (client env and settings), `process.rs`
  (how another process was started), `supervisor.rs` (launchd / systemd user
  unit).
- Evidence: `probe.rs` and `doctor.rs` (capability probes), `recorder.rs`
  (fixture capture), `surface.rs` (real Messages surface capture).

## Naming

- Identifiers describe what they do, not who calls them. The upstream is a
  provider, not a brand; the client is a harness, not a product tier. Comments
  may name a real client or endpoint where that is the accurate explanation.
- **Operator-facing output runs the other way.** What `status`, `models`,
  `usage`, `accounts`, `reload`, and CLI errors print names the real provider
  by the store's own ids (`codex`, `anthropic`, as `accounts` lists them),
  expanded to a company or product name only where the id alone would not be
  understood. Identifiers, module names, and spec prose stay role-based.
