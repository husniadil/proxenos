---
name: proxenos
description: Run Claude Code on an OpenAI model through the proxenos daemon, mainly to spawn a second-eye agent on a different model family. Use when asked for a second opinion from gpt-5.6-sol or another OpenAI model, to launch a claude pane through proxenos, or to check proxenos status, models, accounts, or quota. Trigger words - proxenos, second eye, gpt-5.6, sol, luna, terra, OpenAI model, other model family.
---

# proxenos

`proxenos` is a local daemon that presents an Anthropic Messages API and serves it
from an OpenAI Responses backend over a ChatGPT subscription. Claude Code runs
unchanged against it: same tools, same skills, same `htask` and `herdr` access. What
changes is the model answering. The main use is a **second eye**: a reviewer on a
different model family than the one that wrote the code.

## Launch

```sh
proxenos exec --account work-codex claude --model gpt-5.6-sol --effort high
```

- `proxenos exec` applies the proxy environment (`ANTHROPIC_BASE_URL`, tier model
  ids, client policy) to one command and is consumed there; nothing leaks into the
  parent shell.
- `--account NAME` picks which stored account serves this session's turns without
  changing the daemon's default. An unknown name is refused before anything starts.
  The session is configured for that account too: its `[accounts.NAME.tiers]`
  mapping is what every turn is served on, the tier ids handed to the client are
  that account's, and the mapping applied is printed as the session starts. So an
  Anthropic account needs no `--model` to be usable, and a plain `--model` id is
  upgraded to its `[1m]` long-context variant where that account offers one.
- Everything after the program name is handed to it unchanged. `--model` accepts any
  id the backend knows (`proxenos models` lists them) or a tier name (`fable`,
  `opus`, `sonnet`, `haiku`) which the daemon maps via `[tiers]` in its config.
- `--effort` is honoured per request, capped by the `effort` ceiling in
  `~/.config/proxenos/config.toml`. If the ceiling is `low`, asking for `high` gets
  `low`; check `proxenos effort` first. `proxenos effort set high` raises it on the
  running daemon until it stops (`--persist` writes the file), and
  `proxenos tiers set opus gpt-5.6-sol` repoints one tier the same way, and
  `--as ACCOUNT` pins a tier's turns to another stored account's quota (consent
  is asked once: `--allow-cross-account`, written to config.toml).

Non-interactive works the same way: `proxenos exec --account work-codex claude -p
"..." --model gpt-5.6-sol`.

## In a Herdr pane

`herdr agent start` launches the agent binary itself, so it cannot wrap the command
in `proxenos exec`. Use a plain pane instead:

```sh
herdr pane split --current --direction right --cwd "$PWD" --no-focus   # .result.pane
herdr pane wait-output <pane> --regex '\$ $' --timeout 30000             # shell prompt
herdr pane run <pane> 'proxenos exec --account work-codex claude --model gpt-5.6-sol --effort high'
herdr agent wait <pane> --timeout 60000                                   # claude detected, idle
herdr agent prompt <pane> "<brief>" --wait --timeout 1800000
herdr pane read <pane> --source recent-unwrapped --lines 200
```

Herdr still detects the pane's agent as `claude`, so `agent prompt`, `agent wait`,
`agent read`, and the sidebar quota bar all work. Never let `--current` resolve
without a verified `HERDR_PANE_ID`; it falls back to the human's focused pane.

A `/goal` longer than one short line does not register through `agent prompt`
(the composer collapses a long paste into plain text). Send the brief as an ordinary
prompt, or hand a one-liner via `herdr pane run ... -- '/goal <one-line>'` only when
it fits on one line.

## Second-eye brief

The reviewer is a different model with the same tools. Give it what a reviewer needs
and nothing that biases it toward the author's conclusion:

- what to read: the diff (`git diff main...<branch>`), the design section, the
  acceptance criteria (`htask get <id>`), and the author's submitted report
- the stance: adversarial. For every claim in the report, try to refute it. A claim
  with no test that fails when the behaviour is deleted is unverified.
- where to work: a separate worktree, bootstrapped per the repo's rules, never the
  author's branch
- what to return: findings with severity and `file:line`, back to the orchestrator
  (`herdr-mail` or an `htask note_add`), not a verdict. `htask approve` / `reject`
  stays with the orchestrator, and the reviewer pane must not be the one that
  claimed or submitted the task.

## Status and quota

```sh
proxenos status            # base url, serving account, tier -> model, daemon, client policy
proxenos models            # ids the backend knows, with context window and tier
proxenos accounts list     # stored accounts and which one serves by default
proxenos usage             # quota as last reported; --refresh asks the backend (costs a request)
proxenos doctor            # capability probes; --live runs them against the real backend
proxenos env               # the exports, for a shell you want to configure by hand
```

`proxenos status` also prints client policy. `client claude-api denied` means the
`claude-api` skill is disabled for sessions through the proxy; a reviewer that does
not need it is unaffected.

## Limits

- Effort is a ceiling, not a floor: a turn runs at the lower of `--effort` and the
  `effort` ceiling in the config, so asking for less than the ceiling gets less.
- The system prompt Claude Code sends names Anthropic's model; `[instructions]
  identity = true` in the config prepends one line naming the actual model.
- The daemon must be running (`proxenos status` answers). It is supervised; if it
  is down, `proxenos start` brings it back. `proxenos stop` stops it for every pane
  of this user, so ask the operator before stopping it.
