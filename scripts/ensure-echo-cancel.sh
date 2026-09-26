#!/usr/bin/env bash
# Create Hyusk's dedicated PipeWire/Pulse echo-cancel pair if it is not loaded.
# This does not change the system default source or sink.
set -euo pipefail

hyusk_source_name=hyusk_aec_source
hyusk_sink_name=hyusk_aec_sink

if pactl list short sources | rg -q "[[:space:]]${hyusk_source_name}[[:space:]]"; then
    if pactl list short sinks | rg -q "[[:space:]]${hyusk_sink_name}[[:space:]]"; then
        exit 0
    fi
    echo "Hyusk echo-cancel source exists but its sink is missing" >&2
    exit 1
fi

hyusk_master_source=${HYUSK_AEC_MASTER_SOURCE:-$(pactl get-default-source)}
hyusk_master_sink=${HYUSK_AEC_MASTER_SINK:-$(pactl get-default-sink)}
if [[ "$hyusk_master_source" == "$hyusk_source_name" || "$hyusk_master_sink" == "$hyusk_sink_name" ]]; then
    echo "Set HYUSK_AEC_MASTER_SOURCE and HYUSK_AEC_MASTER_SINK to physical devices" >&2
    exit 1
fi

pactl load-module module-echo-cancel \
    "source_name=$hyusk_source_name" \
    "sink_name=$hyusk_sink_name" \
    "source_master=$hyusk_master_source" \
    "sink_master=$hyusk_master_sink" \
    aec_method=webrtc >/dev/null
