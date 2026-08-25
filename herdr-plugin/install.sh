#!/bin/sh
# Writes the two things a herdr plugin cannot declare for itself: the key that
# opens the dashboard, and the sidebar rows that render the quota tokens. Both
# are appended between marker comments so `uninstall` can remove exactly what
# was added and nothing else. An existing `[ui.sidebar.agents]` table is never
# touched: appending a second one is a TOML error, so the rows are printed for
# a manual merge instead.
#
#   sh install.sh            append what is missing
#   sh install.sh uninstall  remove what install added
set -eu

config="${HERDR_CONFIG:-$HOME/.config/herdr/config.toml}"
begin="# >>> proxenos-usage (managed by install.sh; do not edit inside)"
end="# <<< proxenos-usage"

keys_block() {
    cat <<EOF
$begin
[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "proxenos.open"
description = "proxenos usage"
$end
EOF
}

rows_block() {
    cat <<EOF
$begin
[ui.sidebar.agents]
rows = [
  ["state_icon", "workspace", "tab"],
  ["agent"],
  [{ token = "\$usage_hdr", dim = true }],
  [{ token = "\$usage_1", dim = true }],
  [{ token = "\$usage_2", dim = true }],
  [{ token = "\$usage_3", dim = true }],
]
$end
EOF
}

if [ "${1:-}" = "uninstall" ]; then
    [ -f "$config" ] || exit 0
    tmp=$(mktemp)
    awk -v begin="$begin" -v end="$end" '
        $0 == begin { inside = 1; next }
        $0 == end { inside = 0; next }
        !inside { print }
    ' "$config" >"$tmp"
    mv "$tmp" "$config"
    echo "removed the proxenos-usage blocks from $config"
    exit 0
fi

mkdir -p "$(dirname "$config")"
touch "$config"

if grep -qF "$begin" "$config"; then
    echo "already installed in $config; run 'sh install.sh uninstall' first to redo"
    exit 0
fi

{
    echo ""
    keys_block
} >>"$config"

if grep -q '^\[ui\.sidebar\.agents\]' "$config"; then
    echo "$config already declares [ui.sidebar.agents]; merge these rows yourself:"
    echo ""
    rows_block
else
    {
        echo ""
        rows_block
    } >>"$config"
fi

echo "installed into $config; apply it with: herdr server reload-config"
