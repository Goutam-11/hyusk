#!/usr/bin/env bash
# Download a laptop-sized public speech corpus for wake-word negatives.
set -euo pipefail

cd "$(dirname "$0")/.."

url="https://storage.googleapis.com/download.tensorflow.org/data/mini_speech_commands.zip"
destination="models/public_negatives/mini_speech_commands"
archive_size=182082353

if [[ -d "$destination" ]] &&
   [[ -n "$(find "$destination" -type f -name '*.wav' -print -quit)" ]]; then
  echo "Public negatives already exist: $destination"
  exit 0
fi

available_kb="$(df -Pk . | awk 'NR == 2 {print $4}')"
if (( available_kb < 600000 )); then
  echo "Need at least 600 MB free to download and extract public negatives." >&2
  exit 1
fi

for command in curl unzip; do
  command -v "$command" >/dev/null || {
    echo "Missing required command: $command" >&2
    exit 1
  }
done

temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT

echo "Downloading TensorFlow Mini Speech Commands (about 182 MB)..."
curl --fail --location --retry 3 --output "$temporary/commands.zip" "$url"

actual_size="$(stat -c '%s' "$temporary/commands.zip")"
if [[ "$actual_size" != "$archive_size" ]]; then
  echo "Unexpected archive size: $actual_size bytes (expected $archive_size)." >&2
  exit 1
fi

mkdir -p "$(dirname "$destination")"
unzip -q "$temporary/commands.zip" -d "$(dirname "$destination")"

count="$(find "$destination" -type f -name '*.wav' | wc -l)"
if (( count < 1000 )); then
  echo "Extraction produced only $count WAV files; refusing incomplete dataset." >&2
  exit 1
fi

printf '%s\n' \
  'TensorFlow Mini Speech Commands' \
  'Source: https://www.tensorflow.org/tutorials/audio/simple_audio' \
  'Dataset license: CC BY 4.0' \
  > "$(dirname "$destination")/SOURCE.txt"

echo "Ready: $count public speech negatives in $destination"
echo "The trainer samples 2,000 by default; it does not load every clip."
