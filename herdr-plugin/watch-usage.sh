#!/bin/sh
# Detached per-pane watcher, spawned by `report-usage.sh` for claude panes.
# Re-publishes the pane's quota bars on a timer: quota ticks and the serving
# account moves with no herdr event, so the timer is what keeps the bars from
# going stale. Exits once the pane is gone; the reporter spawns a fresh
# watcher for any claude pane it sees without a live one.
set -u

pane="${1:?usage: watch-usage.sh <pane-id> <pidfile>}"
pidfile="${2:?usage: watch-usage.sh <pane-id> <pidfile>}"
herdr_bin="${HERDR_BIN_PATH:-herdr}"
interval="${PROXENOS_USAGE_WATCH_INTERVAL:-60}"
case "$interval" in *[!0-9]* | '') interval=60 ;; esac
[ "$interval" -lt 5 ] && interval=5
dir=$(dirname "$0")

# Own the pidfile so the reporter sees a live watch and does not spawn a
# second one; drop it on the way out so a later run can. A pidfile left by a
# killed watch self-heals: the next spawn sees a dead pid and takes over.
echo "$$" >"$pidfile" 2>/dev/null
trap 'rm -f "$pidfile"' EXIT

fails=0
while :; do
    # A gone pane (and a down server) answers non-zero. Retry a few times so
    # a transient blip does not kill the watch, then end it once persistent.
    if ! "$herdr_bin" pane process-info --pane "$pane" >/dev/null 2>&1; then
        fails=$((fails + 1))
        if [ "$fails" -ge 3 ]; then
            break
        fi
        sleep "$interval"
        continue
    fi
    fails=0
    # Empty the event/context JSON so the report resolves the agent from
    # nothing instead of inheriting the spawn hook's stale value.
    HERDR_PANE_ID="$pane" HERDR_PLUGIN_EVENT_JSON='' HERDR_PLUGIN_CONTEXT_JSON='' \
        "$dir/report-usage.sh" >/dev/null 2>&1 || true
    sleep "$interval"
done
