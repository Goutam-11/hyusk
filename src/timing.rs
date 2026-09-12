//! Lightweight per-stage latency instrumentation.
//!
//! Enabled with `HYUSK_TIMING=1`. The goal is to see where a voice turn spends
//! its time (recording, transcription, model, tools, speech) before optimizing.

use std::time::Instant;

/// Whether timing output is enabled (`HYUSK_TIMING=1`).
pub fn enabled() -> bool {
    std::env::var("HYUSK_TIMING")
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Print how long `label` took since `start`, when timing is enabled.
pub fn mark(label: &str, start: Instant) {
    if enabled() {
        eprintln!("[timing] {label}: {} ms", start.elapsed().as_millis());
    }
}
