# Security

## Reporting

Report suspected vulnerabilities privately through GitHub's security advisory
form on this repository rather than in a public issue.

Include what you did, what happened, and what you expected. A proof of concept
helps; a working exploit is not required.

## Posture

**Two doors, one daemon.** `127.0.0.1` is always bound and always authenticates
nothing, which is safe precisely because every caller reaching that listener is
already a local process running as the user. A reachable `listen.address`
(`[listen]`, `docs/api.md` §4) opens a **second** listener beside it, where
every request — turns and the control vocabulary alike — must carry the token.
The daemon **refuses to start** with a non-loopback address and no token, and
refuses a wildcard address, which cannot be split into two doors.

**The token is a property of the door, not of the peer.** Nothing reads a
request's source address to decide whether it needs one. A peer-keyed guard
cannot be exercised from a single machine, and behind a reverse proxy or an
overlay-network daemon every request arrives from loopback — where a peer-keyed
exemption would exempt everyone.

**What this means for threat modelling.** Anything that can open a TCP
connection to `127.0.0.1:<port>` on the daemon's machine is inside the trust
boundary and always has been: local processes are the callers this daemon was
built for. Adding a remote door does not narrow that; it adds a guarded way in
from elsewhere.

`ANTHROPIC_AUTH_TOKEN` must be set for the client's sake. At the loopback door
its value is ignored except for the account tag — it is not a credential and
protects nothing there. At the remote door it is where the token travels, as
`proxenos-token:<secret>`, and the token is compared in constant time: a
short-circuiting comparison is an oracle that gives up the secret a byte at a
time.

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
