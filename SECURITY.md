# Security

## Reporting

Report suspected vulnerabilities privately through GitHub's security advisory
form on this repository rather than in a public issue.

Include what you did, what happened, and what you expected. A proof of concept
helps; a working exploit is not required.

## Posture

**Loopback without a token, or beyond it with one.** The daemon binds
`127.0.0.1` by default and authenticates nothing there, which is safe precisely
because every caller reaching the socket is already a local process running as
the user. Binding any other address removes that assumption, so it is allowed
only with a token configured (`[listen]`, `docs/api.md` §4) and the daemon
**refuses to start** with a non-loopback address and no token.

`ANTHROPIC_AUTH_TOKEN` must be set for the client's sake. On a daemon with no
token its value is ignored — it is not a credential and protects nothing. On one
with a token it is where that token travels, as `proxenos-token:<secret>`, and
the token is compared in constant time: a short-circuiting comparison is an
oracle that gives up the secret a byte at a time.

**What the token is.** It gates this daemon and nothing else. Anyone holding it
who can reach the port can do what a local caller could — serve turns on the
accounts this daemon holds, and change its settings. It never appears in process
arguments (there is no flag for it), in logs, or in what `status`, `env` or
`settings` print. Prefer `listen.token_file`, which must be `0600` and is
refused otherwise.

**This project terminates no TLS.** A daemon reachable beyond loopback belongs
behind a private overlay network or a reverse proxy that does; over plain HTTP
the token and every turn cross the wire in the clear.

**Credentials.** Stored in a file created `0600`, from the outset rather than
tightened afterwards — writing first and adjusting permissions later leaves a
window in which the file is world-readable, and that window is enough. They
never appear in the configuration file, in process arguments, or in logs at any
level. `Debug` is implemented by hand on every type that holds one, and a test
asserts no token appears in its output.

The control socket is owner-only for the same reason: it can clear credentials,
so the filesystem is its access control.

**Refresh tokens.** This proxy runs its own authorization flow and owns its own
refresh-token family. It does not read or write credentials belonging to any
other tool. Families rotate, so sharing one means whichever client refreshes
last invalidates the other.

**Captures.** An empty upstream stream is recorded with the request that caused
it (§5.4), and `record` writes exchanges deliberately. A capture is not a
credential, but it is conversation content: the system prompt, the messages, and
whatever the tools read, file contents included. Captures are written beside the
configuration rather than in a shared temporary directory, created `0600` in a
`0700` directory, and bounded — a repeating failure cannot fill a disk, and the
oldest are pruned rather than kept forever.

**No telemetry.** Nothing is collected. Nothing is transmitted anywhere but the
backend the operator authenticated against.

## Scope

The upstream endpoint is not a published or supported API. That it may change or
be withdrawn is a stated limitation rather than a vulnerability.

Reports that the proxy allows a local user to use the local user's own
credentials are not vulnerabilities.
