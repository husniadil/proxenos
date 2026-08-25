# proxenos herdr plugin

Quota bars for every claude pane in the [herdr](https://herdr.dev) sidebar,
and the full per-account usage table in a popup. The bars show what the pane
actually spends: a pane routed through the proxenos daemon shows the serving
account's windows, and a pane talking to Anthropic directly shows the
operator's own Claude account — model-scoped windows included.

## Requires

- herdr 0.8.0 or newer. The manifest declares it, so an older herdr refuses
  to link.
- `proxenos` and `jq` on `PATH`, and the proxenos daemon running.
- Linux or macOS. The entrypoints are POSIX shell.

## Install

```sh
herdr plugin link /path/to/proxenos/herdr-plugin
sh install.sh
herdr server reload-config
```

`install.sh` writes the two things a plugin cannot declare for itself — the
`prefix+u` keybinding that opens the dashboard, and the sidebar rows that
render the tokens — between marker comments in your herdr config.
`sh install.sh uninstall` removes exactly those blocks. A config that already
declares `[ui.sidebar.agents]` is left alone and the rows are printed for a
manual merge, because a second declaration of one TOML table is an error.

## What it runs as you

Two actions and two event hooks, all shell scripts in this directory, plus a
detached per-pane watcher:

- `proxenos.open` opens the dashboard popup (`r` refreshes, `q` or `Esc`
  quits).
- `proxenos.report` re-resolves the focused pane's routing and pushes its
  quota tokens; the same script runs on herdr's `pane.agent_detected` and
  `pane.agent_status_changed` events. Reporting a claude pane starts the
  watcher for that pane.

The watcher re-publishes the bars every 60 seconds until the pane closes,
because quota ticks with no herdr event. `usage --refresh` — the one thing
here that contacts the providers — runs from one stamp-guarded place, at most
every five minutes. The scripts write only herdr's own pane metadata plus a
stamp and one pidfile per watched pane in the plugin state directory.

## How a pane's account is resolved

The reporter reads the pane's claude process (`herdr pane process-info`), then
its environment (`ps eww`):

| The process carries | The pane shows |
|---|---|
| this daemon's `ANTHROPIC_BASE_URL` | the serving account's windows |
| no base URL, default profile | the keychain anthropic account's windows |
| a foreign base URL, or `CLAUDE_CONFIG_DIR` | nothing — no figure beats a wrong one |

## Files

| File | Role |
|---|---|
| `herdr-plugin.toml` | Manifest: one popup entrypoint, two actions, two event hooks |
| `report-usage.sh` | Resolves a pane's routing and publishes its quota bars |
| `watch-usage.sh` | Per-pane watcher re-publishing on a timer |
| `popup-usage.sh` | The dashboard the popup runs |
| `open-pane.sh` | Opens an entrypoint, treating "popup already open" as a no-op |
| `install.sh` | Appends the keybinding and sidebar rows; `uninstall` reverses it |
