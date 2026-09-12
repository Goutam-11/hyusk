#!/usr/bin/env bash
#
# Download wake-word classifiers for hyusk_agent.
#
# Two sources are supported:
#
#   hey_livekit  LiveKit's temporary "hey livekit" classifier.
#   alexa, hey_jarvis, hey_mycroft, hey_rhasspy
#                Pre-trained OpenWakeWord classifiers. These need one rename
#                (embeddings -> score) which is done by
#                scripts/convert-openwakeword-model.py.
#
# The detectors auto-discover these files in models/.
#
# Usage:
#   ./scripts/download-wake-word.sh            # hey_livekit + alexa
#   ./scripts/download-wake-word.sh alexa
#   ./scripts/download-wake-word.sh hey_livekit alexa hey_jarvis
#
set -euo pipefail

cd "$(dirname "$0")/.."

mkdir -p models

declare -A SOURCE_URLS
SOURCE_URLS=(
  [hey_livekit]="https://raw.githubusercontent.com/livekit/livekit-wakeword/main/examples/resources/hey_livekit.onnx"
  [alexa]="https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/alexa_v0.1.onnx"
  [hey_jarvis]="https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/hey_jarvis_v0.1.onnx"
  [hey_mycroft]="https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/hey_mycroft_v0.1.onnx"
  [hey_rhasspy]="https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/hey_rhasspy_v0.1.onnx"
)

if [[ "$#" -ge 1 ]]; then
  WORDS=("$@")
else
  WORDS=(hey_livekit alexa)
fi

for word in "${WORDS[@]}"; do
  url="${SOURCE_URLS[$word]:-}"

  if [[ -z "$url" ]]; then
    echo "Unknown wake word: $word (choose one of: ${!SOURCE_URLS[@]})" >&2
    exit 1
  fi

  out="models/${word}.onnx"
  tmp="models/.${word}.download"

  if [[ -f "$out" ]]; then
    echo "Already present: $out"
    continue
  fi

  echo "Downloading $word -> $out"
  curl --fail --location --silent --show-error --output "$tmp" "$url"

  if [[ "$word" == "hey_livekit" ]]; then
    mv "$tmp" "$out"
  else
    if ! python3 scripts/convert-openwakeword-model.py "$tmp" "$out"; then
      rm -f "$tmp"
      echo "Conversion failed. Install onnx with: python3 -m pip install --user onnx" >&2
      exit 1
    fi
    rm -f "$tmp"
  fi
done

echo
echo "Done. Say the wake word to trigger, or set WAKE_WORD_MODEL explicitly."
echo "Tip: combine several models with WAKE_WORD_MODELS=models/alexa.onnx,models/hey_livekit.onnx"
