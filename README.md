# proxenos

Claude Code, running on OpenAI models served through a ChatGPT subscription,
without modifying Claude Code.

![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)
![Platforms: macOS, Linux, Windows](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)

proxenos is a local daemon. It presents an Anthropic Messages API to Claude
Code and serves each turn from the account you choose: a Codex subscription or
an OpenAI API key through a translation layer, or an Anthropic account relayed
untranslated.

## What it does

- **Keeps Claude Code's built-in tools working.** `Read` on images and PDFs,
  `WebSearch`, `WebFetch`, tool search and the context meter all depend on
  behaviour the server provides. A translator that only maps messages breaks
  each of them silently, with a 200 and plausible output. Preserving them is
  the point of this project.
- **Borrows the accounts you already have.** A subscription stays in the
  profile directory of the program that signed in (`codex login`,
  `claude auth login`). The daemon reads the grant there and never holds a
  copy.
- **Switches accounts per daemon or per session.** `accounts use` moves every
  turn; `exec --account` serves one session as another account.
- **Maps Claude Code's tiers to models.** `opus`, `sonnet`, `haiku` and
  `fable` each point at a model, with an optional effort ceiling, globally or
  per account.
- **Reports quota.** `usage` shows what is left per account, and
  `statusline` merges it into your own status-line script.

## Status

Released, with binaries for macOS, Linux and Windows. The upstream endpoint is
not a published or supported API: it may change or be withdrawn without
notice, and using a subscription this way is each operator's own decision.

## Requirements

- A Codex subscription signed in with `codex login`, an OpenAI API key, or an
  Anthropic account signed in with `claude auth login` or held as a key.
- Claude Code.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/husniadil/proxenos/main/install.sh | sh
```

The script picks the release for your platform, verifies it against the
release's `SHA256SUMS`, and installs into `~/.local/bin`. It has no flag to
skip the checksum. `--version <tag>`, `--bin-dir <dir>` and `--target <triple>`
override its choices, and `--dry-run` downloads nothing. Windows binaries are
on the [releases page](https://github.com/husniadil/proxenos/releases); the
script does not install them.

By hand, from the same release:

```sh
target=aarch64-apple-darwin   # x86_64-apple-darwin, x86_64-unknown-linux-gnu,
                              # aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc
base=https://github.com/husniadil/proxenos/releases/latest/download
curl -fLO "$base/proxenos-$target.tar.gz"
curl -fLO "$base/SHA256SUMS"
shasum -a 256 --ignore-missing -c SHA256SUMS   # sha256sum on Linux
tar -xzf "proxenos-$target.tar.gz"
install "proxenos-$target/proxenos" ~/.local/bin/
```

From source:

```sh
cargo install --git https://github.com/husniadil/proxenos --locked proxenos
```

## First run

```sh
proxenos doctor            # capability probes against recorded fixtures; no credentials, no quota
proxenos start             # the daemon, on 127.0.0.1:8787, in the background
proxenos accounts          # the accounts it found; * marks the one serving turns
proxenos exec claude       # Claude Code, pointed at the daemon
```

With no `[profiles]` in the configuration, the daemon uses the stock profile
of each program (`codex` and `claude`). To name profiles yourself, or add one:

```sh
proxenos accounts login work --provider codex     # runs `codex login` into a new directory and declares it
proxenos accounts login work --provider codex --relogin   # signs a declared profile back in
proxenos accounts add-key api --provider codex < key.txt  # stores a key, read from stdin
proxenos accounts use work
```

`--device-auth` prints a URL and a code instead of opening a browser; it is
for `--provider codex` only. `accounts rename OLD NEW` and `accounts remove
NAME` manage the rest.

`exec` exists because part of what Claude Code needs lives in its settings
file, not its environment. `eval "$(proxenos env)"` sets routing only, and
`proxenos settings` prints the whole document for a settings file you merge
it into.

Claude Code warns that it does not recognise the model ids and suggests a
200,000-token compaction window. Ignore it: `env` already sets the model's real
window.

## Configure

The file is `~/.config/proxenos/config.toml` (`$XDG_CONFIG_HOME/proxenos`, or
`$PROXENOS_HOME`). A missing file is a first run, and the first command that
writes it starts from a fully commented example. A short one:

```toml
port = 8787
effort = "high"            # a ceiling on what the client asks for; unset means none

[tiers]
opus   = "gpt-6-sol"       # these four are the defaults
sonnet = "gpt-5.6-terra"
haiku  = "gpt-6-luna"      # WebFetch and WebSearch run on haiku
fable  = "gpt-6-astra"

[profiles.work]
provider = "codex"
path = "/Users/me/.codex-work"

[accounts.work.tiers]      # overrides for one account; unnamed tiers fall through
opus = "gpt-5.6-sol"
```

The daemon validates each tier against the account's catalog and refuses a
model that is not there; `proxenos models` lists what is. Credentials never go
in this file.

A running daemon takes changes without a restart:

```sh
proxenos reload                          # re-reads config.toml, says what needs a restart
proxenos tiers set opus gpt-5.6-sol      # add --persist to write it to config.toml
proxenos effort set medium               # or none to remove the ceiling
```

[`docs/api.md`](docs/api.md) §4 documents every key, including `[transport]`,
`[instructions]`, `[client]` and `[listen]`.

## Keep it running

```sh
proxenos supervisor install    # a launchd agent on macOS, a systemd user unit on Linux
proxenos supervisor status
proxenos status                # connection, serving account, tier mapping
proxenos stop                  # under a supervisor, replaces the daemon with the build on disk
proxenos update --version 0.31.0   # under a supervisor, installs that release and restarts on it
```

`update` works on a daemon installed by `install.sh` into `~/.local/bin` and
kept by the supervisor. It checks the release against its `SHA256SUMS` and
keeps the old binary as `proxenos.previous`. With `PROXENOS_DAEMON` set it
updates that machine's daemon.

## Reach it from another machine

The daemon always serves `127.0.0.1` without a token. Setting
`[listen] address` opens a second listener that demands a token on every
request, and the daemon refuses to start with a reachable address and no
token. On the other machine, set `PROXENOS_DAEMON` and `PROXENOS_TOKEN` (or
`PROXENOS_TOKEN_FILE`) and use the CLI as usual. proxenos terminates no TLS,
so put a private overlay network or a reverse proxy in front. See
[`docs/api.md`](docs/api.md) §2.7 and [`SECURITY.md`](SECURITY.md).

## Learn more

| If you want to | Read |
|---|---|
| Every verb, flag, config key and error | [`docs/api.md`](docs/api.md), or `proxenos <verb> --help` |
| How translation, transports, sessions and token counts behave | [`docs/proxy-behavior.md`](docs/proxy-behavior.md) |
| Quota bars in the herdr sidebar | [`herdr-plugin/`](herdr-plugin/README.md) |
| Report a vulnerability | [`SECURITY.md`](SECURITY.md) |
| Work on the code | [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CLAUDE.md`](CLAUDE.md) |

## If you are an AI agent helping someone with proxenos

- **Check the CLI before quoting it.** `proxenos <verb> --help` matches the
  installed build; [`docs/api.md`](docs/api.md) §2 matches this checkout.
- **Launching Claude Code through it, or a second opinion from another model
  family:** read [`skills/proxenos/SKILL.md`](skills/proxenos/SKILL.md).
- **A setup problem:** run `proxenos status` and `proxenos doctor` and read
  their output before suggesting a fix.
- **Do not spend quota unasked.** `doctor --live`, `usage --refresh`, and
  `record upstream` / `record surface` contact the providers.
- **Ask the person first** before `proxenos stop`, `proxenos update`,
  `accounts use`, `accounts remove`, or anything with `--persist`. They affect every session on the
  machine.
- **Never put a key or token in argv.** Keys go on stdin; tokens go in
  `PROXENOS_TOKEN_FILE`.
- **Changing the code:** read [`CLAUDE.md`](CLAUDE.md), then the section of
  [`docs/proxy-behavior.md`](docs/proxy-behavior.md) you are touching.

## License

Apache License 2.0; see [LICENSE](LICENSE). Not affiliated with, endorsed by,
or sponsored by Anthropic or OpenAI. All trademarks belong to their owners.
