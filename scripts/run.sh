#!/usr/bin/env bash
# run.sh -- Unix entrypoint for the Prompt Deck toggle action.
#
# The binary's `launch` mode does the real work (find or create the deck pane, pick
# the target, start the deck); this script just locates the binary. herdr runs a
# manifest action's command with the plugin directory as cwd, so the relative
# binary path resolves on Unix.
set -u

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin="$script_dir/../target/release/prompt-deck"

if [ ! -x "$bin" ]; then
    echo "prompt-deck: $bin not found. Run: cargo build --release" >&2
    exit 1
fi

exec "$bin" launch
