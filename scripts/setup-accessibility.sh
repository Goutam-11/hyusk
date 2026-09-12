#!/usr/bin/env bash
#
# setup-accessibility.sh - make more apps visible to AT-SPI2.
#
# AT-SPI only sees an app if that app publishes an accessibility tree:
#   - GTK apps do it only when org.gnome.desktop.interface
#     toolkit-accessibility is true;
#   - Chromium/Electron apps (Chrome, Brave, VS Code, Slack, ...) also need
#     ACCESSIBILITY_ENABLED=1 in their environment.
#
# Usage:
#   ./scripts/setup-accessibility.sh          # enable, print next steps
#   ./scripts/setup-accessibility.sh --check  # show current state
#
set -euo pipefail

check() {
  echo "toolkit-accessibility: $(gsettings get org.gnome.desktop.interface toolkit-accessibility 2>/dev/null || echo unknown)"

  if command -v flatpak >/dev/null 2>&1; then
    echo
    echo "Flatpak apps with ACCESSIBILITY_ENABLED set:"
    found=0

    while read -r app; do
      [[ -z "$app" ]] && continue

      if flatpak override --user --show "$app" 2>/dev/null | grep -qi accessibility_enabled; then
        echo "  $app"
        found=1
      fi
    done < <(flatpak list --app --columns=application 2>/dev/null)

    [[ "$found" -eq 0 ]] && echo "  (none)"
  fi

  echo
  echo "Apps currently registered with AT-SPI:"
  python3 - <<'PY'
try:
    import pyatspi
except Exception as error:
    print("  pyatspi unavailable:", error)
    raise SystemExit(0)

for index, app in enumerate(pyatspi.Registry.getDesktop(0)):
    try:
        windows = [w.name for w in app if w.getRoleName() in ("frame", "window", "dialog")]
    except Exception:
        windows = []
    print(f"  {index}: {app.name} {windows}")
PY
}

if [[ "${1:-}" == "--check" ]]; then
  check
  exit 0
fi

echo "Enabling GNOME toolkit accessibility..."
gsettings set org.gnome.desktop.interface toolkit-accessibility true

# Persist ACCESSIBILITY_ENABLED for apps launched from the session. Files in
# ~/.config/environment.d are read by systemd --user at login.
ENV_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d"
mkdir -p "$ENV_DIR"

cat > "$ENV_DIR/hyusk-accessibility.conf" <<'EOF'
# Read by systemd --user at login. Makes Chromium/Electron apps (Chrome,
# Brave, VS Code, Slack, ...) publish their accessibility tree to AT-SPI.
ACCESSIBILITY_ENABLED=1
GTK_MODULES=gail:atk-bridge
EOF

echo "Wrote $ENV_DIR/hyusk-accessibility.conf"
echo
echo "Next steps:"
echo "  1. Restart the apps you want to control, and re-login once so the"
echo "     environment file applies to newly launched apps."
echo "  2. Flatpak apps (e.g. Brave) need the variable inside the sandbox:"
echo "       flatpak override --user --env=ACCESSIBILITY_ENABLED=1 com.brave.Browser"
echo "  3. Verify with: ./scripts/setup-accessibility.sh --check"
