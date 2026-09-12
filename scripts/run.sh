#!/usr/bin/env bash
# run.sh -- Unix launcher for the Prompt Deck pane.
#
# Toggle: focus an existing "Prompt Deck" pane, otherwise split the focused pane
# downward, run the deck binary in it, and label it.
#
# Unix is untested so far (developed on Windows). The manifest's `toggle` action
# calls this script. herdr has no focus-by-id, so focusing is a zoom on/off cycle.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
deck_bin="$script_dir/../target/release/prompt-deck"

# First pane_id whose terminal title is "Prompt Deck" (empty if none).
find_pane_by_title() {
    printf '%s' "$1" | tr '}' '\n' | grep -F '"terminal_title":"Prompt Deck"' | head -n1 |
        grep -o '"pane_id":"[^"]*"' | head -n1 | cut -d'"' -f4
}

# First focused pane's id and cwd.
focused_field() {
    printf '%s' "$1" | tr '}' '\n' | grep -F '"focused":true' | head -n1 |
        grep -o "\"$2\":\"[^\"]*\"" | head -n1 | cut -d'"' -f4
}

panes="$("$herdr_bin" pane list 2>/dev/null || true)"

existing="$(find_pane_by_title "$panes")"
if [ -n "$existing" ]; then
    "$herdr_bin" pane zoom "$existing" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$existing" --off
fi

target="$(focused_field "$panes" pane_id)"
cwd="$(focused_field "$panes" cwd)"
cwd="${cwd:-$PWD}"

config_dir="$("$herdr_bin" plugin config-dir prompt-deck 2>/dev/null || true)"
split_args=(pane split --direction down --cwd "$cwd" --ratio 0.9 --focus)
if [ -n "$config_dir" ]; then
    split_args+=(--env "HERDR_PLUGIN_CONFIG_DIR=$config_dir")
fi
out="$("$herdr_bin" "${split_args[@]}")"
new_pane="$(printf '%s' "$out" | grep -o '"pane_id":"[^"]*"' | head -n1 | cut -d'"' -f4)"
if [ -z "$new_pane" ]; then
    exit 1
fi

"$herdr_bin" pane rename "$new_pane" "Prompt Deck" >/dev/null 2>&1 || true
exec "$herdr_bin" pane run "$new_pane" "\"$deck_bin\" pane --target \"$target\""
