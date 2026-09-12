#!/usr/bin/env bash
#
# set-mic-gain.sh - give the default microphone a sane capture level.
#
# Laptop audio stacks (PipeWire + ALSA UCM) often restore the capture mixer to
# maximum (+30 dB capture and +30 dB mic boost). That clips the analog-to-
# digital converter, and no amount of software processing can recover the
# flattened waveform, so wake-word detection and speech-to-text both degrade.
#
# The software source volume is set first: changing it after the hardware gain
# can make WirePlumber re-apply the maximum-gain profile.
#
# Usage:
#   ./scripts/set-mic-gain.sh          # 48% capture, 0% boost
#   ./scripts/set-mic-gain.sh 60 0     # louder capture, no boost
#
set -euo pipefail

CAPTURE="${1:-48}"
BOOST="${2:-0}"

SOURCE="$(pactl get-default-source 2>/dev/null || true)"

if [[ -z "$SOURCE" ]]; then
  echo "No PipeWire/PulseAudio default source found." >&2
  exit 1
fi

pactl set-source-volume "$SOURCE" 100% >/dev/null 2>&1 || true
pactl set-source-mute "$SOURCE" 0 >/dev/null 2>&1 || true

CARD="$(pactl list sources 2>/dev/null \
  | awk -v name="$SOURCE" '
      index($0, "Name: " name) > 0 { found = 1 }
      found && /alsa.card =/ { gsub(/"/, "", $3); print $3; exit }')"

if [[ -z "${CARD:-}" ]]; then
  echo "Could not determine the ALSA card for $SOURCE." >&2
  echo "Set the level manually with: alsamixer" >&2
  exit 1
fi

set_control() {
  local control="$1" value="$2"

  if amixer -c "$CARD" sget "$control" >/dev/null 2>&1; then
    amixer -c "$CARD" sset "$control" "$value" >/dev/null 2>&1 || true

    local current
    current="$(amixer -c "$CARD" sget "$control" 2>/dev/null \
      | awk '/Front Left:/{print $3, $4, $5; exit}')"

    echo "  $control -> $current"
  fi
}

echo "Microphone: $SOURCE (card $CARD)"
set_control "Capture" "${CAPTURE}%"
set_control "Mic Boost" "${BOOST}%"
echo
echo "Speak normally. If wake detection still clips, try a lower capture level,"
echo "e.g. ./scripts/set-mic-gain.sh 35 0"
