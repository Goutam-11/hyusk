#!/usr/bin/env bash
# Build, install, and enable Hyusk as a graphical-session user service.
# It starts after GNOME login and receives SIGINT when the session ends or the
# machine shuts down, allowing the Rust runtime to stop its wake/audio tasks.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
UNIT_SOURCE="$PROJECT_DIR/systemd/hyusk.service.in"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT_PATH="$UNIT_DIR/hyusk.service"
BINARY="$PROJECT_DIR/target/release/hyusk_agent"

# Optional surfaces are Cargo features, so the installer has to pass the
# choice through: `--features mobile` installs the phone link (P5), and the
# default build leaves it out.
BUILD_FEATURES=()
VOICE_AUDIO=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --voice-audio)
      VOICE_AUDIO=true
      shift
      ;;
    --features)
      if [[ -z "${2:-}" ]]; then
        echo "Missing a feature list after --features." >&2
        exit 1
      fi
      BUILD_FEATURES=(--features "$2")
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo "Usage: $0 [--voice-audio] [--features <list>]" >&2
      exit 1
      ;;
  esac
done

if [[ ! -f "$UNIT_SOURCE" ]]; then
  echo "Missing service template: $UNIT_SOURCE" >&2
  exit 1
fi

# .env holds a live provider key, so only its owner may read it. The runtime
# reads it at startup; tighten a wider mode here and say so.
ENV_FILE="$PROJECT_DIR/.env"
if [[ -f "$ENV_FILE" ]]; then
  ENV_MODE="$(stat -c '%a' "$ENV_FILE" 2>/dev/null || true)"
  if [[ -n "$ENV_MODE" && "$ENV_MODE" != "600" ]]; then
    chmod 600 "$ENV_FILE"
    echo "Restricted $ENV_FILE to mode 600 (was $ENV_MODE)."
  fi
fi

echo "Building Hyusk release binary..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" "${BUILD_FEATURES[@]}"

mkdir -p "$UNIT_DIR"
sed \
  -e "s|@WORKDIR@|$PROJECT_DIR|g" \
  -e "s|@BINARY@|$BINARY|g" \
  "$UNIT_SOURCE" > "$UNIT_PATH"

# Optional dedicated noise/echo cancellation. Existing audio overrides are
# otherwise preserved, including users who deliberately choose raw capture.
if [[ "$VOICE_AUDIO" == true ]]; then
  mkdir -p "$UNIT_DIR/hyusk.service.d"
  sed -e "s|@WORKDIR@|$PROJECT_DIR|g" \
    "$PROJECT_DIR/systemd/hyusk-audio.conf.in" \
    > "$UNIT_DIR/hyusk.service.d/10-aec.conf"
fi

systemctl --user daemon-reload
systemctl --user enable hyusk.service
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XDG_CURRENT_DESKTOP DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR || true
systemctl --user restart hyusk.service

echo "Hyusk will now start automatically after GNOME login."
echo "Status: systemctl --user status hyusk.service"
echo "Logs:   journalctl --user -u hyusk.service -f"
