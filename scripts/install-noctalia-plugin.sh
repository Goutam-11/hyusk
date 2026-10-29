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

# The panel's Device tab runs these two; the plugin directory is the only place
# it can look for them, exactly as the GNOME indicator finds them beside itself.
for script in scripts/setup-mobile-link.sh scripts/start-browser-control.sh; do
  if [[ ! -f "$script" ]]; then
    echo "Companion script is missing: $script" >&2
    exit 1
  fi
done

mkdir -p "$PLUGIN_DIR"
cp "$SOURCE_DIR/plugin.toml" "$SOURCE_DIR/hyusk.luau" "$SOURCE_DIR/settings.luau" "$PLUGIN_DIR/"
cp scripts/setup-mobile-link.sh scripts/start-browser-control.sh "$PLUGIN_DIR/"
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
echo "Left-click the state glyph to start listening or stop the agent, and"
echo "right-click to dismiss the card. The gear opens the Hyusk panel, whose"
echo "tabs hold the transport keys, the reply card with approve/deny, the model"
echo "picker, workflow quick-run, and phone pairing. A keybind can drive the"
echo "widget too:"
echo "  noctalia msg plugin $PLUGIN_ID:hyusk all start_listening"
