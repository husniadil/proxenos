#!/bin/bash
# Installs what `just check` needs in a Claude Code cloud session, at the
# versions .tool-versions pins. A local checkout has mise for that.
#
# The SessionStart hook in .claude/settings.json runs it against the checkout,
# so a bump to .tool-versions applies at the next session. Each step is skipped
# when its tool is already in place, so a second run touches no network.
set -euo pipefail

[[ ${CLAUDE_CODE_REMOTE:-} == true ]] || exit 0

bin=/root/.cargo/bin
repo=$CLAUDE_PROJECT_DIR
tool_versions=$(<"$repo/.tool-versions")
fetch() { curl -fsSL --retry 3 --retry-all-errors "$1"; }

rust=$(awk '$1 == "rust" { print $2 }' <<<"$tool_versions")
just=$(awk '$1 == "just" { print $2 }' <<<"$tool_versions")
: "${rust:?no rust line in .tool-versions}" "${just:?no just line in .tool-versions}"

if [[ $("$bin/just" --version 2>/dev/null) != "just $just" ]]; then
  fetch "https://github.com/casey/just/releases/download/$just/just-$just-x86_64-unknown-linux-musl.tar.gz" \
    | tar xz -C "$bin" just
fi
if [[ ! -x $bin/cargo-nextest ]]; then
  fetch https://get.nexte.st/latest/linux | tar xz -C "$bin"
fi
if [[ ! -x $bin/cargo-insta ]]; then
  fetch https://github.com/mitsuhiko/insta/releases/latest/download/cargo-insta-x86_64-unknown-linux-musl.tar.xz \
    | tar xJ -C "$bin" --strip-components=1 cargo-insta-x86_64-unknown-linux-musl/cargo-insta
fi

# A path override, so anything else in the session keeps the image's default.
if [[ $("$bin/rustup" toolchain list) != *"$rust-"* ]]; then
  "$bin/rustup" toolchain install "$rust" --profile minimal --component rustfmt,clippy --no-self-update
fi
override=$("$bin/rustup" override list | awk -v repo="$repo" '$1 == repo { print $2 }')
if [[ $override != "$rust-"* ]]; then
  "$bin/rustup" override set "$rust" --path "$repo"
fi
