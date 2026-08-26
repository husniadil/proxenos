#!/bin/sh
# The usage dashboard: every account the daemon holds, bars and resets per
# window, rendered from `proxenos usage --json` and redrawn every 30 seconds.
# `r` asks the providers for fresh figures (`usage --refresh`); `q` or Esc
# quits. Keys are read raw (one character, no Enter) where stty allows it.
set -u

command -v jq >/dev/null 2>&1 || { echo "proxenos usage: jq is required" >&2; exit 1; }
command -v proxenos >/dev/null 2>&1 || { echo "proxenos usage: proxenos is not on PATH" >&2; exit 1; }

# Raw terminal input, so a single keypress — Esc included — acts without
# Enter. A terminal stty cannot reshape falls back to line-buffered reads,
# where Enter is still needed and Esc cannot be seen.
saved_tty=$(stty -g 2>/dev/null || true)
restore_tty() { [ -n "$saved_tty" ] && stty "$saved_tty" 2>/dev/null; }
trap 'restore_tty' EXIT INT TERM
raw=0
if [ -n "$saved_tty" ] && stty -icanon -echo min 0 time 0 2>/dev/null; then
    raw=1
fi
esc=$(printf '\033')

# Waits up to $1 seconds and prints the first key pressed, or nothing on
# timeout. Raw mode polls once a second; the fallback is the old read.
next_key() {
    if [ "$raw" = 1 ]; then
        waited=0
        while [ "$waited" -lt "$1" ]; do
            c=$(dd bs=1 count=1 2>/dev/null)
            [ -n "$c" ] && { printf '%s' "$c"; return 0; }
            sleep 1
            waited=$((waited + 1))
        done
        return 0
    fi
    started=$(date +%s)
    key=""
    if ! read -r -t "$1" key 2>/dev/null; then
        # EOF behind a partial line still delivered a key: hand it over now
        # rather than sleeping an interval on input that has already arrived.
        [ -n "$key" ] && { printf '%s' "$key"; return 0; }
        # A shell without `read -t` returns at once; sleep the interval so
        # the loop stays a redraw rather than a spin. Quitting still works —
        # the popup closes with the pane.
        [ $(($(date +%s) - started)) -lt 2 ] && sleep "$1"
    fi
    printf '%s' "$key"
}

render() {
    proxenos usage --json 2>/dev/null | jq -r --argjson now "$(date +%s)" '
        def bar: (. / 10 | floor | if . > 10 then 10 else . end) as $f
            | ("▰" * $f) + ("▱" * (10 - $f));
        def pad5: (" " * (5 - length)) + .;
        def pct3: (" " * (3 - length)) + .;
        def until: (. - $now) as $s
            | if $s <= 0 then "already reset"
              elif $s < 3600 then "resets in \($s / 60 | floor)m"
              elif $s < 86400 then "resets in \($s / 3600 | floor)h \($s % 3600 / 60 | floor)m"
              else "resets in \($s / 86400 | floor)d \($s % 86400 / 3600 | floor)h" end;
        def age: ($now - .) as $s
            | if $s < 60 then "just now"
              elif $s < 3600 then "\($s / 60 | floor)m ago"
              elif $s < 86400 then "\($s / 3600 | floor)h ago"
              else "\($s / 86400 | floor)d ago" end;
        # Minor units as money. The exponent is the one the provider stated,
        # and the digits are padded rather than divided so nothing is rounded.
        def money($minor; $exp): ($minor | tostring) as $m
            | if $exp == 0 then $m
              else (if ($m | length) > $exp then $m
                    else ("0" * ($exp - ($m | length) + 1)) + $m end) as $p
                   | $p[0:($p | length) - $exp] + "." + $p[($p | length) - $exp:] end;
        def wname: if .window_minutes == null then (.label // "?" | ascii_downcase)
            elif .window_minutes >= 1440 then "\(.window_minutes / 1440 | floor)d"
            else "\(.window_minutes / 60 | floor)h" end;

        "proxenos usage" + (" " * 30) + "r refresh · esc quit",
        "",
        (.accounts[]
            | (if .serving then "* " else "  " end) as $mark
            | ($mark + .account + " (" + .provider + ")"
                + (if .plan then " — " + .plan else "" end)
                + (if .measured_at then "   as of " + (.measured_at | age) else "" end)),
              # Stated only when the provider stopped calling it active; an
              # active subscription is silence, here as everywhere else.
              (if .subscription_status then "    subscription " + .subscription_status
               else empty end),
              (if (.windows // [] | length) > 0 then
                  (.windows[] | "    \(wname | pad5)  \(.used_percent // 0 | floor | bar)  \(.used_percent // 0 | floor | tostring | pct3)%"
                      + (if .resets_at then "   \(.resets_at | until)" else "" end))
               else
                  "    " + (.reason // .detail // "no figure")
                      + (if .served_tokens and .served_tokens > 0 then " · \(.served_tokens) tok served" else "" end)
               end),
              # The credit balance, where the account reports one. Money rather
              # than a percentage of an entitlement, so it carries no bar.
              (if .credit then (.credit
                  | (if .currency == "USD" then "$" else "" end) as $sym
                  | "    credit " + $sym + money(.used_minor; .exponent)
                      + (if .limit_minor then " / " + $sym + money(.limit_minor; .exponent) else "" end)
                      + (if .currency and .currency != "USD" then " " + .currency else "" end)
                      + (if .percent then "  \(.percent | floor)%" else "" end)
                      + (if .severity and .severity != "normal" then " (\(.severity))" else "" end))
               else empty end),
              ""
        )'
}

while :; do
    printf '\033[2J\033[H'
    render || echo "the daemon is not answering; is it running?"

    key=$(next_key 30)
    case "$key" in
        q | Q | "$esc") exit 0 ;;
        r | R)
            printf 'asking the providers for fresh figures...\n'
            proxenos usage --refresh >/dev/null 2>&1
            ;;
    esac
done
