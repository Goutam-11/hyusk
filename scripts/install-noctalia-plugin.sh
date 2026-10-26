#!/usr/bin/env bash
#
# Install and enable the Hyusk bar widget for Noctalia v5.
#
# The widget reads $XDG_RUNTIME_DIR/hyusk-status.json (which the agent writes on
# every state change) and sends actions through hyusk-control.json, the same
# pair the GNOME indicator uses. Use this on niri, Hyprland, or Sway, where the
# GNOME Shell extension in gnome-extension/ cannot load.
set -euo pipefail

cd "$(dirname "$0")/.."

SOURCE_DIR="noctalia-plugin"
PLUGIN_ID="hyusk/indicator"
PLUGIN_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/noctalia/plugins/hyusk-indicator"

for file in plugin.toml hyusk.luau settings.luau; do
  if [[ ! -f "$SOURCE_DIR/$file" ]]; then
    echo "Plugin source is missing: $SOURCE_DIR/$file" >&2
    exit 1
  fi
done

mkdir -p "$PLUGIN_DIR"
cp "$SOURCE_DIR/plugin.toml" "$SOURCE_DIR/hyusk.luau" "$SOURCE_DIR/settings.luau" "$PLUGIN_DIR/"
echo "Installed $PLUGIN_ID to $PLUGIN_DIR"

if ! command -v noctalia >/dev/null 2>&1; then
  echo "Noctalia is not installed; skipping enable." >&2
  exit 0
fi

if ! noctalia msg plugins enable "$PLUGIN_ID" >/dev/null 2>&1; then
  echo "Could not enable $PLUGIN_ID: is Noctalia running?" >&2
  echo "Start it, then run: noctalia msg plugins enable $PLUGIN_ID" >&2
  exit 0
fi
echo "Enabled $PLUGIN_ID"

echo
echo "Add the widget to the bar once: Settings → Bar → Bar Widgets → Hyusk,"
echo "or run 'noctalia msg settings-open bar' to open that tab."
echo
echo "Left-click the state glyph to start listening or stop the agent,"
echo "click the gear for the model picker and API key form, and right-click"
echo "to dismiss the card. A keybind can drive it too:"
echo "  noctalia msg plugin $PLUGIN_ID:hyusk all start_listening"
