#!/usr/bin/env bash
#
# Install and enable the Hyusk GNOME top-bar indicator.
#
# The extension reads $XDG_RUNTIME_DIR/hyusk-state, which the agent writes on
# every state change. Once installed, the agent skips its separate orb window
# by default; use HYUSK_ORB=1 to force the orb.
#
set -euo pipefail

cd "$(dirname "$0")/.."

UUID="hyusk@hyusk.local"
SOURCE_DIR="gnome-extension/$UUID"
DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

if [[ ! -f "$SOURCE_DIR/metadata.json" || ! -f "$SOURCE_DIR/extension.js" || ! -f "$SOURCE_DIR/hyusk_butterfly_mark.png" ]]; then
  echo "Extension sources are missing: $SOURCE_DIR" >&2
  exit 1
fi

installed=false

if command -v gnome-extensions >/dev/null 2>&1 && command -v zip >/dev/null 2>&1; then
  bundle="$(mktemp --suffix=.zip)"
  trap 'rm -f "$bundle"' EXIT

  # mktemp creates an empty file; zip refuses to update it as a zip archive.
  rm -f "$bundle"

  (
    cd "$SOURCE_DIR"
    zip -q -j "$bundle" metadata.json extension.js hyusk_butterfly_mark.png ../../scripts/setup-mobile-link.sh
  )

  if gnome-extensions install --force "$bundle" >/dev/null 2>&1; then
    installed=true
  fi
fi

if [[ "$installed" != true ]]; then
  mkdir -p "$DEST_DIR"
  cp "$SOURCE_DIR/metadata.json" "$SOURCE_DIR/extension.js" "$SOURCE_DIR/hyusk_butterfly_mark.png" "$DEST_DIR/"
  cp scripts/setup-mobile-link.sh "$DEST_DIR/setup-mobile-link.sh"
  echo "Copied $UUID to $DEST_DIR"
else
  echo "Installed $UUID with gnome-extensions"
fi

if command -v gnome-extensions >/dev/null 2>&1; then
  gnome-extensions enable "$UUID" >/dev/null 2>&1 || true
fi

echo
echo "GNOME Shell only scans extensions when the session starts."
echo "Log out and back in, then enable 'Hyusk Agent Indicator' in the"
echo "Extensions app if it is not already enabled."
echo
echo "After that the agent uses the top-bar indicator instead of its own orb."
echo "Use HYUSK_ORB=1 to show the orb anyway, or HYUSK_ORB=0 to force"
echo "indicator-only mode."
