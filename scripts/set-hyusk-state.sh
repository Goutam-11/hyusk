#!/usr/bin/env bash
#
# Write a Hyusk state file for the GNOME top-bar indicator.
#
# Usage:
#   ./scripts/set-hyusk-state.sh Working
#   ./scripts/set-hyusk-state.sh Hidden
#
# Valid states: Hidden, Waking, Listening, Thinking, Working, Speaking.
#
set -euo pipefail

state="${1:-Hidden}"
runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
path="$runtime_dir/hyusk-state"

printf '%s' "$state" > "$path"
echo "$state" > "$path"

echo "Set $path to $state"
