#!/usr/bin/env bash
# Build, install, and enable Hyusk as a graphical-session user service.
# It starts after GNOME login and receives SIGINT when the session ends or the
# machine shuts down, allowing the Rust runtime to stop its wake/audio tasks.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_DIR="$(cd "$PROJECT_DIR/.." && pwd)"
UNIT_SOURCE="$PROJECT_DIR/systemd/hyusk.service.in"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT_PATH="$UNIT_DIR/hyusk.service"
BINARY="$WORKSPACE_DIR/target/release/hyusk_agent"

if [[ ! -f "$UNIT_SOURCE" ]]; then
  echo "Missing service template: $UNIT_SOURCE" >&2
  exit 1
fi

echo "Building Hyusk release binary..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$UNIT_DIR"
sed \
  -e "s|@WORKDIR@|$PROJECT_DIR|g" \
  -e "s|@BINARY@|$BINARY|g" \
  "$UNIT_SOURCE" > "$UNIT_PATH"

systemctl --user daemon-reload
systemctl --user enable hyusk.service
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XDG_CURRENT_DESKTOP DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR || true
systemctl --user restart hyusk.service

echo "Hyusk will now start automatically after GNOME login."
echo "Status: systemctl --user status hyusk.service"
echo "Logs:   journalctl --user -u hyusk.service -f"
