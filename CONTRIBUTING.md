# Contributing

How to work on this repository: set up a checkout, run the gate, and cut a
release. The rules a change has to follow (the layering, the non-negotiables,
naming) are in [CLAUDE.md](CLAUDE.md), and they apply to people as much as to
agents.

## Set up a checkout

The toolchain is pinned in `.tool-versions` and installed with mise.

```sh
just setup     # mise install, rustfmt and clippy, cargo-nextest, cargo-insta
```

## The gate

```sh
just check     # formatting, clippy -D warnings, and the whole suite
```

It is what CI runs, and it is the same command locally. `just test` runs the
suite alone, `just test-one <filter>` one test, and `just snapshots` reviews
pending `insta` snapshots.

From a checkout, `just run` starts the daemon in the foreground with debug
logging, and `just doctor` and `just record` wrap the verbs of the same name.

## No test touches the network

Every upstream interaction in the suite runs against a local replay server, so
the gate is green without credentials and without quota. A test that needs a
live backend stops running the moment quota runs out.

Only `doctor --live`, `record upstream` and `record surface` spend quota, and
none of them is part of the gate. Plain `doctor` answers from the fixture
corpus, and `record ingress` captures what the client sends at no cost.

## The specification comes first

[`docs/proxy-behavior.md`](docs/proxy-behavior.md) is normative, and
[`docs/api.md`](docs/api.md) is the contract for what the proxy exposes. Read
the relevant section before touching translation, transport, sessions or token
accounting. If implementation shows a rule is wrong, change the spec in the
same commit as the code that proved it.

A verb, flag or config key that moves has to move in
[`skills/proxenos/SKILL.md`](skills/proxenos/SKILL.md) and
[`herdr-plugin/`](herdr-plugin/) in the same commit, because both quote the
CLI.

## Development is test-first

Failing test, then the code that passes it, then refactor. Translation rules
are pure functions over data in `proxenos-core`, so the expected output is the
specification.

Upstream behaviour is captured, never guessed: record a real exchange, make it
a fixture, write the failing test against the fixture, then implement.

Before trusting a test that passed on its first run, make it fail. Break the
code it covers, watch it go red, and put the code back. The traps:

- A timing assertion whose window makes the failure impossible.
- A comparison where both sides are derived from the same source.
- An expected value recomputed the way the code computes it.
- A probe keyed on something the model could infer without the evidence.

## Say what is derived and what is confirmed

A capability verified against replayed fixtures is derived, not confirmed.
What only a live backend can settle goes in
[`docs/roadmap.md`](docs/roadmap.md) §L, and no output may read like
confirmation when it is not.

## Commits

Small, working increments, each reviewable and revertible on its own. The
message says why; the diff already says what.

## Releasing

1. Bump `version` in the workspace `Cargo.toml` (both crates inherit it) and
   move the `CHANGELOG.md` entries under the new version.
2. Run `cargo update --workspace` so `Cargo.lock` carries the new version;
   every recipe builds with `--locked`.
3. Run `just check`, commit all three files, tag `v<version>`, and push the tag.

`.github/workflows/release.yml` then runs the gate on the tag, builds the
binaries for every target `install.sh` knows, writes one `SHA256SUMS` over all
the archives, and publishes the GitHub release.

## License

By contributing you agree your work is licensed under the Apache License 2.0
([LICENSE](LICENSE)).
