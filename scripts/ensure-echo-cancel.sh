#!/usr/bin/env bash
# Create Hyusk's dedicated PipeWire/Pulse echo-cancel pair if it is not loaded.
# This does not change the system default source or sink.
set -euo pipefail

hyusk_source_name=hyusk_aec_source
hyusk_sink_name=hyusk_aec_sink

hyusk_master_source=${HYUSK_AEC_MASTER_SOURCE:-$(pactl get-default-source)}
hyusk_master_sink=${HYUSK_AEC_MASTER_SINK:-$(pactl get-default-sink)}
if [[ "$hyusk_master_source" == "$hyusk_source_name" || "$hyusk_master_sink" == "$hyusk_sink_name" ]]; then
    echo "Set HYUSK_AEC_MASTER_SOURCE and HYUSK_AEC_MASTER_SINK to physical devices" >&2
    exit 1
fi

# A USB headset or a new default mic must not leave Hyusk attached to the
# previous microphone forever. Only replace the pair owned by this script.
hyusk_existing=$(pactl list short modules | awk -F '\t' '$2 == "module-echo-cancel" && $3 ~ /(^| )source_name=hyusk_aec_source( |$)/ && $3 ~ /(^| )sink_name=hyusk_aec_sink( |$)/ { print $1 "\t" $3; exit }')
hyusk_aec_args='noise_suppression=1 analog_gain_control=0 digital_gain_control=0 high_pass_filter=1'
if [[ -n "$hyusk_existing" ]]; then
    hyusk_module_id=${hyusk_existing%%$'\t'*}
    hyusk_old_args=${hyusk_existing#*$'\t'}
    if [[ " $hyusk_old_args " == *" source_master=$hyusk_master_source "* &&
          " $hyusk_old_args " == *" sink_master=$hyusk_master_sink "* &&
          "$hyusk_old_args" == *"$hyusk_aec_args"* ]] &&
       pactl list short sources | rg -q "[[:space:]]${hyusk_source_name}[[:space:]]" &&
       pactl list short sinks | rg -q "[[:space:]]${hyusk_sink_name}[[:space:]]"; then
        exit 0
    fi
    pactl unload-module "$hyusk_module_id"
elif pactl list short sources | rg -q "[[:space:]]${hyusk_source_name}[[:space:]]"; then
    echo "Hyusk source name is already used by another audio module; refusing to replace it" >&2
    exit 1
fi

pactl load-module module-echo-cancel \
    "source_name=$hyusk_source_name" \
    "sink_name=$hyusk_sink_name" \
    "source_master=$hyusk_master_source" \
    "sink_master=$hyusk_master_sink" \
    aec_method=webrtc \
    "aec_args=$hyusk_aec_args" >/dev/null
echo "[Audio] Hyusk noise/echo cancellation: $hyusk_master_source -> $hyusk_source_name"
