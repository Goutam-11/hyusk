#!/usr/bin/env bash
set -euo pipefail

UNIT_PATH="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/hyusk.service"
systemctl --user disable --now hyusk.service 2>/dev/null || true
rm -f "$UNIT_PATH"
systemctl --user daemon-reload
echo "Hyusk automatic startup has been disabled. Journal logs remain available."
