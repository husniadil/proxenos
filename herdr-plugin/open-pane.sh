#!/bin/sh
# Opens one of this plugin's popup entrypoints. herdr's popup is a session
# singleton, so a second open while one is up answers "popup already open".
# Pressing the key twice is not an error, so that answer exits 0 and every
# other failure still reaches the plugin log.
set -eu

entrypoint="${1:?usage: open-pane.sh <entrypoint-id>}"
herdr_bin="${HERDR_BIN_PATH:-herdr}"
plugin_id="${HERDR_PLUGIN_ID:-proxenos}"

if out=$("$herdr_bin" plugin pane open --plugin "$plugin_id" --entrypoint "$entrypoint" 2>&1); then
    exit 0
fi

case "$out" in
    *"popup already open"*) exit 0 ;;
esac

printf '%s\n' "$out" >&2
exit 1
