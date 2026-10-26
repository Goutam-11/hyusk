#!/usr/bin/env bash
set -euo pipefail

configured_url="${HYUSK_BROWSER_CDP_URL:-}"
configured_port="${HYUSK_BROWSER_CDP_PORT:-}"
if [[ -n "$configured_url" ]]; then
  if [[ ! "$configured_url" =~ ^http://(127\.0\.0\.1|localhost|\[::1\])(:([0-9]+))?/?$ ]]; then
    echo "HYUSK_BROWSER_CDP_URL must be a root URL on http://127.0.0.1, localhost, or [::1], with an optional port." >&2
    exit 2
  fi
  cdp_host="${BASH_REMATCH[1]}"
  url_port="${BASH_REMATCH[3]:-80}"
  if [[ -n "$configured_port" && "$configured_port" != "$url_port" ]]; then
    echo "HYUSK_BROWSER_CDP_PORT must match the port in HYUSK_BROWSER_CDP_URL." >&2
    exit 2
  fi
  port="${configured_port:-$url_port}"
else
  port="${configured_port:-9222}"
  configured_url="http://127.0.0.1:${port}"
  cdp_host="127.0.0.1"
fi

if [[ ! "$port" =~ ^[0-9]+$ ]] || (( port < 1 || port > 65535 )); then
  echo "The CDP port must be a valid TCP port." >&2
  exit 2
fi

case "${1:-}" in
  ''|--worker) ;;
  *) echo "Usage: $0 [--worker]" >&2; exit 2 ;;
esac
probe_url="http://${cdp_host}:${port}/json/version"
if curl --noproxy '*' --connect-timeout 1 --max-time 1 --silent --fail "$probe_url" >/dev/null; then
  echo "Configured CDP endpoint is already available; no browser was restarted."
  exit 0
fi

# Keep the browser out of hyusk.service's cgroup: cancelling a tool or
# restarting the voice service must not close the automation browser.
if [[ "${1:-}" != --worker ]]; then
  if ! command -v systemd-run >/dev/null 2>&1; then
    echo "Run this script with --worker in a separate terminal; managed startup requires systemd-run." >&2
    exit 1
  fi
  if ! systemctl --user is-active --quiet hyusk-browser-control.service; then
    systemctl --user reset-failed hyusk-browser-control.service >/dev/null 2>&1 || true
    browser_env=("--setenv=HYUSK_BROWSER_CDP_URL=$configured_url")
    for browser_variable in HYUSK_BROWSER_BIN HYUSK_BROWSER_PROFILE; do
      if [[ -n "${!browser_variable:-}" ]]; then
        browser_env+=("--setenv=$browser_variable=${!browser_variable}")
      fi
    done
    systemd-run --user --unit=hyusk-browser-control --collect "${browser_env[@]}" \
      /usr/bin/bash "$(readlink -f "$0")" --worker
  fi
  for ((attempt=0; attempt<15; attempt++)); do
    if curl --noproxy '*' --connect-timeout 1 --max-time 1 --silent --fail "$probe_url" >/dev/null; then
      echo "Hyusk automation browser is ready at $probe_url"
      exit 0
    fi
    sleep 0.3
  done
  echo "Automation browser did not expose CDP. Inspect: journalctl --user -u hyusk-browser-control.service. Existing browsers were not killed." >&2
  exit 1
fi

browser_cmd=()
browser_label=""
flatpak_app=""
if [[ -n "${HYUSK_BROWSER_BIN:-}" ]]; then
  browser_cmd=("$HYUSK_BROWSER_BIN")
  browser_label="$HYUSK_BROWSER_BIN"
else
  for candidate in brave-browser brave chromium chromium-browser google-chrome google-chrome-stable; do
    if command -v "$candidate" >/dev/null 2>&1; then
      browser_cmd=("$(command -v "$candidate")")
      browser_label="$candidate"
      break
    fi
  done
  if [[ ${#browser_cmd[@]} -eq 0 ]] && command -v flatpak >/dev/null 2>&1; then
    for app in com.brave.Browser com.google.Chrome; do
      if flatpak info "$app" >/dev/null 2>&1; then
        flatpak_app="$app"
        browser_cmd=(flatpak run "$app")
        browser_label="Flatpak $app"
        break
      fi
    done
  fi
fi

if [[ ${#browser_cmd[@]} -eq 0 ]]; then
  echo "Could not find Brave, Chromium, or Chrome (including supported Flatpak apps). Set HYUSK_BROWSER_BIN to an executable path." >&2
  exit 1
fi

if [[ -n "${HYUSK_BROWSER_PROFILE:-}" ]]; then
  profile="$HYUSK_BROWSER_PROFILE"
elif [[ -n "$flatpak_app" ]]; then
  profile="$HOME/.var/app/$flatpak_app/data/hyusk-browser-control-profile"
else
  profile="${XDG_DATA_HOME:-$HOME/.local/share}/hyusk/browser-control-profile"
fi

listen_address="$cdp_host"
if [[ "$listen_address" == "[::1]" ]]; then
  listen_address="::1"
fi

mkdir -p "$profile"
echo "Starting ${browser_label} with the dedicated Hyusk profile at ${profile} (CDP ${cdp_host}:${port})."
exec "${browser_cmd[@]}" \
  --user-data-dir="$profile" \
  --remote-debugging-address="$listen_address" \
  --remote-debugging-port="$port" \
  --no-first-run \
  --no-default-browser-check
