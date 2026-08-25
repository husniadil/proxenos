#!/bin/sh
# Publishes quota bars for one claude pane as pane metadata, so the sidebar's
# $usage_* rows can render them. Runs from the agent event hooks and from the
# `proxenos.report` action, which is why the pane comes from the injected
# context rather than an argument.
#
# Routing decides whose quota a pane shows. A pane whose claude process
# carries this daemon's ANTHROPIC_BASE_URL spends the serving account; one
# with no base URL at all spends the operator's own Claude login, which is the
# store's keychain-sourced anthropic account; anything else — a foreign proxy,
# a custom CLAUDE_CONFIG_DIR — gets no tokens rather than a wrong figure.
set -u

herdr_bin="${HERDR_BIN_PATH:-herdr}"
pane="${HERDR_PANE_ID:-}"
[ -n "$pane" ] || exit 0

command -v jq >/dev/null 2>&1 || exit 0
command -v proxenos >/dev/null 2>&1 || exit 0

# The hooks fire for every agent herdr detects; only claude panes spend
# anything this daemon meters. An action carries `focused_pane_agent` instead
# of `agent`, and the watcher clears both so this resolves from nothing.
agent=$(printf '%s' "${HERDR_PLUGIN_EVENT_JSON:-}" | sed -n 's/.*"agent":"\([^"]*\)".*/\1/p')
[ -n "$agent" ] || agent=$(printf '%s' "${HERDR_PLUGIN_CONTEXT_JSON:-}" | sed -n 's/.*"focused_pane_agent":"\([^"]*\)".*/\1/p')
case "$agent" in
    "" | claude) ;;
    *) exit 0 ;;
esac

# The claude process in this pane, and the routing its environment states.
pid=$("$herdr_bin" pane process-info --pane "$pane" 2>/dev/null |
    jq -r '.result.process_info.foreground_processes[]?
           | select(.argv0 == "claude" or .name == "claude" or (.argv[0]? == "claude"))
           | .pid' | head -n 1)
[ -n "$pid" ] || exit 0

child_env=$(ps eww -o command= -p "$pid" 2>/dev/null | tr ' ' '\n')
base_url=$(printf '%s\n' "$child_env" | sed -n 's/^ANTHROPIC_BASE_URL=//p' | head -n 1)
config_dir=$(printf '%s\n' "$child_env" | sed -n 's/^CLAUDE_CONFIG_DIR=//p' | head -n 1)

clear_tokens() {
    "$herdr_bin" pane report-metadata "$pane" --source proxenos.usage --seq "$(date +%s)" \
        --clear-token usage_hdr --clear-token usage_1 --clear-token usage_2 --clear-token usage_3 \
        >/dev/null 2>&1
    exit 0
}

daemon_url=$(proxenos env 2>/dev/null | sed -n 's/^export ANTHROPIC_BASE_URL=//p' | tr -d '"' | head -n 1)

# Which stored account this pane spends.
if [ -n "$base_url" ]; then
    [ "$base_url" = "$daemon_url" ] || clear_tokens
    selector='.accounts[] | select(.serving)'
else
    # A direct pane on a custom profile spends a credential this daemon does
    # not meter. No figure beats a wrong one.
    [ -z "$config_dir" ] || clear_tokens
    # The keychain-sourced anthropic account is the default profile's own
    # credential — the same grant the pane spends. Provider alone is not
    # enough: a second anthropic account would make provider ambiguous.
    keychain=$(proxenos accounts --json 2>/dev/null |
        jq -r '[.accounts[] | select(.provider == "anthropic" and ((.source // "") | test("keychain"; "i")))] | if length == 1 then .[0].name else empty end')
    [ -n "$keychain" ] || clear_tokens
    selector=".accounts[] | select(.account == \"$keychain\")"
fi

# Up to three windows as aligned bars: label right-padded to five, ten cells,
# percent right-aligned to three. Values stay under herdr's 80-char cap.
lines=$(proxenos usage --json 2>/dev/null | jq -r "
    [$selector] | if length == 0 then empty else .[0] end | .windows[]? |
    [(if .window_minutes == null then (.label // \"?\" | ascii_downcase)
      elif .window_minutes >= 1440 then \"\(.window_minutes / 1440 | floor)d\"
      else \"\(.window_minutes / 60 | floor)h\" end),
     (.used_percent // 0 | floor | tostring)] | @tsv" |
    awk -F'\t' '{
        filled = int($2 / 10); if (filled > 10) filled = 10
        bar = ""
        for (i = 0; i < 10; i++) bar = bar (i < filled ? "\342\226\260" : "\342\226\261")
        printf "- %5s: %s %3s%%\n", $1, bar, $2
    }' | head -n 3)
[ -n "$lines" ] || clear_tokens

line() { printf '%s\n' "$lines" | sed -n "${1}p"; }
"$herdr_bin" pane report-metadata "$pane" --source proxenos.usage --seq "$(date +%s)" \
    --token usage_hdr="usage:" \
    --token usage_1="$(line 1)" \
    --token usage_2="$(line 2)" \
    --token usage_3="$(line 3)" \
    --ttl-ms 300000 >/dev/null 2>&1

# `usage --refresh` keeps the direct account's figure moving — no relayed turn
# updates it — but it asks the providers, so it runs from this one place and
# at most every five minutes across every reporter and watcher.
state_dir="${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}/proxenos-plugin}"
mkdir -p "$state_dir" 2>/dev/null || exit 0
stamp="$state_dir/refresh.stamp"
now=$(date +%s)
last=$(cat "$stamp" 2>/dev/null || echo 0)
case "$last" in *[!0-9]* | '') last=0 ;; esac
if [ $((now - last)) -ge 300 ] && mkdir "$state_dir/refresh.lock" 2>/dev/null; then
    echo "$now" >"$stamp"
    proxenos usage --refresh >/dev/null 2>&1
    rmdir "$state_dir/refresh.lock" 2>/dev/null
fi

# Quota ticks with no herdr event, so a detached per-pane watcher re-reports
# on a timer. The pidfile makes later invocations skip the spawn while that
# watch lives; a dead pid self-heals on the next report.
[ "$agent" = claude ] || exit 0
pidfile="$state_dir/watch-$pane.pid"
if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile" 2>/dev/null)" 2>/dev/null; then
    exit 0
fi
dir=$(dirname "$0")
"$dir/watch-usage.sh" "$pane" "$pidfile" </dev/null >/dev/null 2>&1 &
