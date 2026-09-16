// src/speech.rs - Fixed audio capture (mono downmix, correct U16 handling,
// silence padding) and upgraded TTS (Piper neural voice, espeak-ng fallback)
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::env;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

use crate::wake::detector::high_pass;

fn strip_markdown_links(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < characters.len() {
        if characters[index] == '[' {
            if let Some(close) = (index + 1..characters.len()).find(|&j| characters[j] == ']') {
                if close + 1 < characters.len() && characters[close + 1] == '(' {
                    if let Some(end) = (close + 2..characters.len()).find(|&j| characters[j] == ')')
                    {
                        // Keep the visible label, drop the destination.
                        for character in &characters[index + 1..close] {
                            output.push(*character);
                        }

                        index = end + 1;

                        continue;
                    }
                }
            }
        }

        output.push(characters[index]);
        index += 1;
    }

    output
}

/// Turn a model reply into text that sounds natural when spoken.
///
/// Models emit Markdown (bold/italic markers, headings, bullets, code fences,
/// links, tables) and emoji. A TTS engine reads the formatting literally
/// ("asterisk asterisk"), so this strips the markup, keeps the words, and
/// expands a few symbols into words. The system prompt also asks for plain
/// spoken prose; this is the safety net for when the model uses Markdown
/// anyway.
pub fn sanitize_for_speech(text: &str) -> String {
    // 1) Drop fenced code blocks entirely: spoken source code is noise.
    let mut without_code = String::with_capacity(text.len());
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            continue;
        }

        if in_code_fence {
            continue;
        }

        // Strip list markers so the item reads as a sentence, not a bullet.
        let mut content = trimmed.to_string();

        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = content.strip_prefix(marker) {
                content = rest.to_string();
                break;
            }
        }

        if let Some((head, rest)) = content.split_once(". ") {
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
                content = rest.to_string();
            }
        }

        without_code.push_str(&content);
        without_code.push('\n');
    }

    // 2) Replace Markdown links with their label.
    let without_links = strip_markdown_links(&without_code);

    // 3) Remove decoration characters and expand speakable symbols.
    let mut spoken = String::with_capacity(without_links.len());

    for character in without_links.chars() {
        match character {
            '*' | '_' | '`' | '#' | '~' | '|' | '^' | '>' => {}

            '&' => spoken.push_str(" and "),
            '%' => spoken.push_str(" percent "),
            '+' => spoken.push_str(" plus "),
            '=' => spoken.push_str(" equals "),
            '@' => spoken.push_str(" at "),
            '\\' => {}

            // Normalize smart punctuation so the engine does not stumble.
            '\u{2018}' | '\u{2019}' => spoken.push('\''),
            '\u{201C}' | '\u{201D}' => spoken.push('"'),
            '\u{2013}' | '\u{2014}' => spoken.push_str(" - "),
            '\u{2026}' => spoken.push_str("..."),

            // Keep letters, digits, whitespace, and basic punctuation. This
            // also drops emoji and symbols an engine would not say.
            c if c.is_alphanumeric() || c.is_whitespace() => spoken.push(c),
            c if matches!(
                c,
                '.' | ',' | '!' | '?' | ':' | ';' | '\'' | '"' | '-' | '(' | ')' | '/'
            ) =>
            {
                spoken.push(c)
            }

            _ => {}
        }
    }

    // 4) Collapse whitespace runs and the gaps left by removed markers.
    let mut collapsed = String::with_capacity(spoken.len());
    let mut last_was_space = false;

    for character in spoken.chars() {
        if character.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
            }

            last_was_space = true;
        } else {
            collapsed.push(character);
            last_was_space = false;
        }
    }

    for (from, to) in [
        (" .", "."),
        (" ,", ","),
        (" !", "!"),
        (" ?", "?"),
        (" :", ":"),
        (" ;", ";"),
        (" /", "/"),
    ] {
        if collapsed.contains(from) {
            collapsed = collapsed.replace(from, to);
        }
    }

    // 5) Replace bare URLs with something speakable.
    let tokens: Vec<String> = collapsed
        .split_whitespace()
        .map(|token| {
            if token.contains("://") || token.starts_with("www.") {
                "a link".to_string()
            } else {
                token.to_string()
            }
        })
        .collect();

    tokens.join(" ").trim().to_string()
}

fn default_piper_model_path() -> String {
    let relative = "models/en_US-lessac-medium.onnx";

    if std::path::Path::new(relative).exists() {
        return relative.to_string();
    }

    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);

    if manifest_path.exists() {
        return manifest_path.to_string_lossy().to_string();
    }

    relative.to_string()
}
// use std::time::Duration;
// use tokio::time::sleep;

pub struct SpeechToText {
    context: Arc<Mutex<WhisperContext>>,
    sample_rate: u32,
}

#[derive(Clone)]
pub struct TextToSpeech;

impl SpeechToText {
    pub fn new(model_path: &str) -> Result<Self> {
        let context = match WhisperContext::new(model_path) {
            Ok(ctx) => {
                println!("✅ Loaded whisper model: {}", model_path);
                ctx
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to load whisper model '{}': {:?}\n\
                    Download with: curl -L -o {} https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
                    model_path,
                    e,
                    model_path
                ));
            }
        };

        Ok(Self {
            context: Arc::new(Mutex::new(context)),
            sample_rate: 16000,
        })
    }

    pub async fn transcribe_from_microphone(&self, duration_secs: f32) -> Result<String> {
        println!(
            "🎤 Recording up to {} seconds (stops after speech ends)...",
            duration_secs
        );

        let audio_data = self.record_audio(duration_secs).await?;

        if audio_data.is_empty() {
            return Ok(String::new());
        }

        println!(
            "📊 Recorded {} samples ({:.2}s @ {}Hz)",
            audio_data.len(),
            audio_data.len() as f32 / self.sample_rate as f32,
            self.sample_rate
        );

        self.transcribe_audio(&audio_data).await
    }

    async fn record_audio(&self, duration_secs: f32) -> Result<Vec<f32>> {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => {
                println!("🎤 Using input device: {}", d.name()?);
                d
            }
            None => return Err(anyhow::anyhow!("No input device available")),
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => return Err(anyhow::anyhow!("Failed to get input config: {}", e)),
        };

        let sample_rate = config.sample_rate().0;
        // THIS WAS THE MAIN BUG: the code never accounted for channel count.
        // If the device reports 2 channels (very common, even for "mono" mics on
        // Linux via PulseAudio/PipeWire defaults), the callback delivers
        // interleaved L,R,L,R,... samples. The old code pushed every raw sample
        // into one buffer and stopped once it hit `sample_rate * duration_secs`
        // samples total -- i.e. it captured roughly HALF the requested duration,
        // and what it did capture was alternating left/right channel data, not a
        // clean mono signal. That's enough to make Whisper produce garbage or
        // nothing at all. Fix: downmix every frame to mono *inside* the callback,
        // so timing and content are both correct regardless of channel count.
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();
        println!(
            "📊 Sample rate: {} Hz, channels: {}, format: {:?}",
            sample_rate, channels, sample_format
        );

        let frame_count = (sample_rate as f32 * duration_secs) as usize;

        // Variable-length recording: keep capturing only while the user is
        // actually talking. Speech start/end are detected on 20 ms frames
        // against an adaptive noise floor, so it works whether the microphone
        // is quiet or noisy, and stops shortly after the user stops speaking
        // instead of always waiting for the maximum duration.
        let silence_stop_ms = env::var("STT_SILENCE_MS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(900);

        let no_speech_timeout_ms = env::var("STT_NO_SPEECH_MS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4000);

        let frame_len = (sample_rate as usize / 50).max(1); // 20 ms
        let silence_stop_frames = (silence_stop_ms / 20).max(1);
        let no_speech_timeout_frames = (no_speech_timeout_ms / 20).max(1);
        let speech_start_frames = 3; // 60 ms of sustained energy
        let min_speech_rms = 0.006;
        let warmup_frames = 15; // 300 ms ignored while the capture settles

        let (tx, rx) = mpsc::channel::<f32>();

        let stream = match config.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32;
                        let _ = tx.send(mono);
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                            / channels as f32;
                        let _ = tx.send(mono);
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::U16 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        for frame in data.chunks(channels) {
                            // U16 PCM is unsigned, centered at 32768 -- the old
                            // code divided by 65535 without recentering, which
                            // produced a 0..1 signal (huge DC offset) instead of
                            // a proper -1..1 waveform.
                            let mono = frame
                                .iter()
                                .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                .sum::<f32>()
                                / channels as f32;
                            let _ = tx.send(mono);
                        }
                    },
                    |err| eprintln!("Audio stream error: {}", err),
                    None,
                )?
            }
            _ => {
                return Err(anyhow::anyhow!("Unsupported audio format"));
            }
        };

        stream.play()?;

        let mut audio_samples = Vec::with_capacity(frame_count);
        let mut collected = 0usize;

        // End-of-speech detector state.
        let mut frame_index = 0usize;
        let mut frame_energy = 0.0f32;
        let mut speech_frames = 0usize;
        let mut silence_frames = 0usize;
        let mut saw_speech = false;
        let mut noise_floor = 0.0f32;
        let mut noise_frames = 0usize;
        let mut total_frames = 0usize;
        let mut warmup_min = f32::MAX;

        // One-pole high-pass state for the endpointer's energy measurement, so
        // low-frequency room rumble does not dominate the noise floor.
        let hp_alpha = {
            let dt = 1.0 / sample_rate as f32;
            let rc = 1.0 / (2.0 * std::f32::consts::PI * 90.0);
            rc / (rc + dt)
        };
        let mut hp_prev_in = 0.0f32;
        let mut hp_prev_out = 0.0f32;

        while collected < frame_count {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(sample) => {
                    let filtered = hp_alpha * (hp_prev_out + sample - hp_prev_in);
                    hp_prev_in = sample;
                    hp_prev_out = filtered;

                    frame_energy += filtered * filtered;
                    frame_index += 1;
                    audio_samples.push(sample);
                    collected += 1;

                    if frame_index < frame_len {
                        continue;
                    }

                    // One 20 ms frame is complete: classify it.
                    let frame_rms = (frame_energy / frame_len as f32).sqrt();

                    frame_energy = 0.0;
                    frame_index = 0;
                    total_frames += 1;

                    // Ignore the first frames while the capture stream settles
                    // (opening the microphone usually produces a click); track
                    // the quietest of them as the starting noise floor.
                    if total_frames <= warmup_frames {
                        warmup_min = warmup_min.min(frame_rms);
                        noise_floor = warmup_min.max(1e-4);

                        continue;
                    }

                    if noise_frames == 0 {
                        noise_floor = warmup_min.max(1e-4);
                    }

                    noise_frames += 1;

                    let threshold = (noise_floor * 2.0).max(min_speech_rms);

                    if env::var_os("STT_DEBUG").is_some() && total_frames.is_multiple_of(5) {
                        eprintln!(
                            "[stt-debug] frame {total_frames} rms {frame_rms:.4} thr {threshold:.4} noise {noise_floor:.4} saw {saw_speech}"
                        );
                    }

                    if frame_rms >= threshold {
                        speech_frames += 1;
                        silence_frames = 0;

                        if speech_frames >= speech_start_frames {
                            saw_speech = true;
                        }
                    } else {
                        silence_frames += 1;
                        speech_frames = speech_frames.saturating_sub(1);

                        // Track the noise floor from quiet frames only, so loud
                        // speech cannot raise the threshold and cut the command.
                        noise_floor = noise_floor * 0.95 + frame_rms * 0.05;
                    }

                    if saw_speech && silence_frames >= silence_stop_frames {
                        println!(
                            "🎤 Speech ended after {:.2}s",
                            collected as f32 / sample_rate as f32
                        );

                        break;
                    }

                    if !saw_speech && total_frames - warmup_frames >= no_speech_timeout_frames {
                        println!("🎤 No speech detected; stopping early");
                        break;
                    }
                }
                Err(_) => {
                    if collected < frame_count / 4 {
                        eprintln!(
                            "⚠️ Audio capture timeout, got {} of {} samples",
                            collected, frame_count
                        );
                    }
                    break;
                }
            }
        }

        drop(stream);

        if audio_samples.is_empty() {
            return Err(anyhow::anyhow!("No audio samples captured"));
        }

        let raw_rms = (audio_samples
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / audio_samples.len() as f32)
            .sqrt();
        let raw_peak = audio_samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));

        println!(
            "📊 Captured {:.2}s: rms {:.4}, peak {:.4}",
            audio_samples.len() as f32 / sample_rate as f32,
            raw_rms,
            raw_peak
        );

        // Remove DC offset and sub-vocal rumble before anything else. Laptop
        // captures carry most of their energy below ~100 Hz, which Whisper
        // otherwise turns into hallucinated text.
        audio_samples = high_pass(&audio_samples, sample_rate as f32);

        self.normalize_audio(&mut audio_samples);

        if sample_rate != self.sample_rate {
            audio_samples = self.resample_audio(&audio_samples, sample_rate, self.sample_rate)?;
        }

        if voice_denoise_enabled() {
            audio_samples = denoise_for_stt(&audio_samples);
        }

        if !self.has_speech_energy(&audio_samples) {
            println!("🔇 Background noise / silence discarded");
            return Ok(Vec::new());
        }

        // Whisper is unreliable on very short buffers (often returns empty or
        // hallucinated text). Pad with trailing silence up to 1 second so a
        // short/clipped recording still gets a fair shot.
        let min_samples = self.sample_rate as usize;
        if audio_samples.len() < min_samples {
            audio_samples.resize(min_samples, 0.0);
        }

        Ok(audio_samples)
    }

    fn normalize_audio(&self, audio: &mut [f32]) {
        if audio.is_empty() {
            return;
        }

        // Find peak amplitude. Silence detection is handled separately by
        // `has_speech_energy`, so this only scales the signal to a level
        // Whisper likes.
        let peak = audio
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);

        if peak < 1e-6 {
            return;
        }

        // Only apply moderate gain.
        //
        // Do NOT normalize all the way to 0.9 because that
        // amplifies microphone noise.
        let gain = (0.7 / peak).min(5.0);

        for sample in audio.iter_mut() {
            *sample *= gain;
        }
    }

    /// Decide whether a recording contains speech rather than steady noise.
    ///
    /// Compares the loud part of the clip against its own background instead of
    /// a single fixed level. A fixed floor either discards quiet speakers or
    /// transcribes steady room noise depending on the microphone gain, so it
    /// cannot work across devices.
    fn has_speech_energy(&self, audio: &[f32]) -> bool {
        if audio.is_empty() {
            return false;
        }

        let frame = (self.sample_rate as usize / 50).max(1);

        let mut levels: Vec<f32> = audio
            .chunks(frame)
            .map(|chunk| {
                (chunk.iter().map(|sample| sample * sample).sum::<f32>() / chunk.len() as f32)
                    .sqrt()
            })
            .collect();

        if levels.is_empty() {
            return false;
        }

        levels.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

        let background = levels[levels.len() / 10];
        let signal = levels[levels.len() * 9 / 10];

        // Speech must be meaningfully louder than the background and above a
        // small absolute floor (the floor is on normalized audio).
        signal >= 0.01 && signal >= background * 2.5
    }

    async fn transcribe_audio(&self, audio_data: &[f32]) -> Result<String> {
        println!("🔍 Transcribing {} samples...", audio_data.len());

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Use the machine's cores (capped so we do not oversubscribe an SMT
        // machine); the old hard-coded 4 left most of a modern CPU idle.
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4)
            .clamp(1, 8);

        params.set_n_threads(threads as i32);
        params.set_translate(false);
        params.set_language(configured_stt_language());
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_suppress_blank(true);
        // Each call is an independent short recording, not a continuation of a
        // longer stream -- carrying context from a previous utterance (the old
        // `false` here) can bias/hallucinate the next transcription.
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_max_tokens(256);
        params.set_audio_ctx(0);
        params.set_offset_ms(0);
        params.set_duration_ms(0);
        params.set_thold_pt(0.02);
        params.set_thold_ptsum(0.02);
        params.set_max_len(0);
        params.set_speed_up(false);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        // Default 0.6 is conservative and can classify a quieter/farther mic as
        // "no speech" and skip it entirely. Loosen this a bit.
        params.set_no_speech_thold(0.55);
        params.set_length_penalty(-1.0);
        params.set_max_initial_ts(1.0);

        let context = Arc::clone(&self.context);
        let audio_data = audio_data.to_vec();

        // Whisper is CPU-heavy; run it (and the segment extraction, which
        // reads state written by `full`) on the blocking pool so the async
        // runtime (the event loop, tool execution, cancellation) stays live.
        tokio::task::spawn_blocking(move || {
            let mut context = context.blocking_lock();

            context
                .full(params, &audio_data)
                .map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))
                .and_then(|_| {
                    let num_segments = context.full_n_segments();
                    println!("📝 Got {} segments", num_segments);

                    if num_segments == 0 {
                        return Err(anyhow::anyhow!("No transcription segments found"));
                    }

                    let mut full_text = String::new();
                    for i in 0..num_segments {
                        match context.full_get_segment_text(i) {
                            Ok(text) => {
                                if !text.is_empty() {
                                    full_text.push_str(&text);
                                    full_text.push(' ');
                                }
                            }

                            Err(e) => {
                                return Err(anyhow::anyhow!(
                                    "Failed to get segment text: {:?}",
                                    e
                                ));
                            }
                        }
                    }

                    let result = full_text.trim().to_string();

                    if result.is_empty() {
                        return Err(anyhow::anyhow!(
                            "Transcription produced empty text. Try speaking louder or check your microphone."
                        ));
                    }

                    Ok(result)
                })
        })
        .await
        .context("Whisper task panicked")?
    }

    fn resample_audio(&self, audio: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
        let ratio = from_rate as f64 / to_rate as f64;
        let output_len = (audio.len() as f64 / ratio).round() as usize;
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = pos - idx as f64;

            if idx + 1 < audio.len() {
                let sample = audio[idx] as f64 * (1.0 - frac) + audio[idx + 1] as f64 * frac;
                output.push(sample as f32);
            } else {
                output.push(audio[idx]);
            }
        }

        Ok(output)
    }
}

fn voice_denoise_enabled() -> bool {
    let configured = env::var("STT_DENOISE")
        .or_else(|_| env::var("VOICE_DENOISE"))
        .unwrap_or_else(|_| "1".to_string());
    !matches!(
        configured.to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Whisper language code configured for short commands. `auto` is useful for
/// multilingual households; an explicit code is usually more accurate and
/// faster for short utterances. Common Indian language codes are accepted
/// directly without allocating or leaking a new string for every command.
fn configured_stt_language() -> Option<&'static str> {
    let language = env::var("STT_LANGUAGE")
        .unwrap_or_else(|_| "en".to_string())
        .trim()
        .to_ascii_lowercase();
    match language.as_str() {
        "auto" | "detect" => None,
        "en" | "english" => Some("en"),
        "hi" | "hindi" => Some("hi"),
        "bn" | "bengali" => Some("bn"),
        "gu" | "gujarati" => Some("gu"),
        "kn" | "kannada" => Some("kn"),
        "ml" | "malayalam" => Some("ml"),
        "mr" | "marathi" => Some("mr"),
        "or" | "odia" | "oriya" => Some("or"),
        "pa" | "punjabi" => Some("pa"),
        "ta" | "tamil" => Some("ta"),
        "te" | "telugu" => Some("te"),
        "ur" | "urdu" => Some("ur"),
        "ne" | "nepali" => Some("ne"),
        other => {
            eprintln!("[STT] Unknown STT_LANGUAGE '{other}'; using English");
            Some("en")
        }
    }
}

/// RNNoise reduces persistent environmental noise before Whisper transcribes a
/// command. It is denoising rather than biometric speaker separation: nearby
/// voices can still be heard by a single laptop microphone.
fn denoise_for_stt(samples: &[f32]) -> Vec<f32> {
    use nnnoiseless::DenoiseState;

    if samples.is_empty() {
        return Vec::new();
    }
    const UPSAMPLE: usize = 3;
    let mut input = Vec::with_capacity(samples.len() * UPSAMPLE);
    for (index, current) in samples.iter().copied().enumerate() {
        let next = samples.get(index + 1).copied().unwrap_or(current);
        input.extend(
            [
                current,
                current + (next - current) / 3.0,
                current + (next - current) * 2.0 / 3.0,
            ]
            .map(|sample| sample * 32768.0),
        );
    }
    let size = DenoiseState::FRAME_SIZE;
    let mut state = DenoiseState::new();
    let silence = vec![0.0; size];
    let mut frame = vec![0.0; size];
    for _ in 0..8 {
        state.process_frame(&mut frame, &silence);
    }
    let mut output = Vec::with_capacity(samples.len());
    for chunk in input.chunks_exact(size) {
        state.process_frame(&mut frame, chunk);
        for values in frame.chunks_exact(3) {
            output.push(((values[0] + values[1] + values[2]) / 3.0 / 32768.0).clamp(-1.0, 1.0));
        }
    }
    output.resize(samples.len(), 0.0);
    output
}

impl TextToSpeech {
    pub fn new() -> Self {
        Self
    }

    /// Non-Linux TTS fallback used by [`TextToSpeech::speak_cancellable`].
    #[cfg(not(target_os = "linux"))]
    pub async fn speak(&self, text: &str) -> Result<()> {
        let text = sanitize_for_speech(text);

        #[cfg(target_os = "macos")]
        {
            let output = Command::new("say")
                .arg(&text)
                .output()
                .context("Failed to execute say command")?;

            if !output.status.success() {
                return Err(anyhow::anyhow!("TTS failed"));
            }
            Ok(())
        }

        #[cfg(target_os = "windows")]
        {
            let ps_script = format!(
                "Add-Type -AssemblyName System.Speech; \
                 $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                 $synth.Rate = 0; \
                 $synth.Volume = 100; \
                 $synth.Speak('{}')",
                text.replace("'", "''")
            );

            let output = Command::new("powershell")
                .args(["-Command", &ps_script])
                .output()
                .context("Failed to execute PowerShell TTS")?;

            if !output.status.success() {
                return Err(anyhow::anyhow!("TTS failed"));
            }
            Ok(())
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Err(anyhow::anyhow!("No TTS available for this platform"))
        }
    }

    /// Speak while remaining cancellable.
    ///
    /// On Linux this uses tokio child processes with `kill_on_drop`, so a new
    /// message or wake word stops the current TTS playback. Other platforms
    /// fall back to a best-effort select around the non-Linux TTS path.
    pub async fn speak_cancellable(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        // Strip Markdown/emoji so the engine speaks words, not formatting.
        let text = sanitize_for_speech(text);

        #[cfg(target_os = "linux")]
        {
            self.speak_cancellable_linux(&text, cancel).await
        }

        #[cfg(not(target_os = "linux"))]
        {
            tokio::select! {
                _ = cancel.cancelled() => Ok(()),
                result = self.speak(&text) => result,
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn speak_cancellable_linux(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        match self.speak_piper_cancellable(text, cancel).await {
            Ok(()) => Ok(()),

            Err(error) => {
                if cancel.is_cancelled() {
                    return Ok(());
                }

                eprintln!("ℹ️ Piper unavailable ({}), falling back to espeak", error);

                self.speak_espeak_cancellable(text, cancel).await
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn speak_piper_cancellable(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command as AsyncCommand;

        let model_path = env::var("PIPER_MODEL").unwrap_or_else(|_| default_piper_model_path());
        let speaker = env::var("PIPER_SPEAKER")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());

        if !std::path::Path::new(&model_path).exists() {
            return Err(anyhow::anyhow!("Piper model not found at '{}'", model_path));
        }

        let tmp_wav = std::env::temp_dir().join("hyusk_tts_output.wav");

        let mut command = AsyncCommand::new("piper");
        command.args([
            "--model",
            &model_path,
            "--output_file",
            tmp_wav.to_str().unwrap(),
        ]);
        if let Some(speaker) = speaker.as_deref() {
            command.args(["--speaker", speaker]);
        }

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Failed to spawn 'piper' (is it installed and on PATH?)")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .context("Failed to write text to piper stdin")?;
        }

        let status = tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Ok(());
            }

            status = child.wait() => status.context("Failed waiting on piper")?,
        };

        if !status.success() {
            return Err(anyhow::anyhow!("piper exited with an error"));
        }

        let mut play = match AsyncCommand::new("paplay")
            .arg(&tmp_wav)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,

            Err(_) => AsyncCommand::new("aplay")
                .arg(&tmp_wav)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .context("Failed to play audio (install pulseaudio-utils or alsa-utils)")?,
        };

        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = play.start_kill();
                let _ = play.wait().await;
                Ok(())
            }

            status = play.wait() => {
                let status = status.context("Failed waiting for audio playback")?;

                if !status.success() {
                    return Err(anyhow::anyhow!("Audio playback failed"));
                }

                println!("🔊 Speaking (piper): {}", text);
                Ok(())
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn speak_espeak_cancellable(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        use tokio::process::Command as AsyncCommand;

        let mut child = match AsyncCommand::new("espeak-ng")
            .arg("-a")
            .arg("200")
            .arg("-s")
            .arg("150")
            .arg("-p")
            .arg("50")
            .arg("-v")
            .arg("en-us")
            .arg("-g")
            .arg("5")
            .arg(text)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,

            Err(_) => AsyncCommand::new("espeak")
                .arg("-a")
                .arg("200")
                .arg("-s")
                .arg("150")
                .arg("-p")
                .arg("50")
                .arg("-v")
                .arg("en-us")
                .arg("-g")
                .arg("5")
                .arg(text)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .context(
                    "Failed to execute espeak/espeak-ng. Install with: sudo apt-get install espeak-ng",
                )?,
        };

        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                Ok(())
            }

            status = child.wait() => {
                let status = status.context("Failed waiting for espeak")?;

                if !status.success() {
                    return Err(anyhow::anyhow!("TTS failed"));
                }

                println!("🔊 Speaking (espeak): {}", text);
                Ok(())
            }
        }
    }
}

pub async fn test_audio() -> Result<()> {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => return Err(anyhow::anyhow!("No audio input device found")),
    };

    let config = device
        .default_input_config()
        .context("Failed to get audio config")?;

    println!("✅ Audio device: {}", device.name()?);
    println!("   Sample rate: {} Hz", config.sample_rate().0);
    println!("   Channels: {}", config.channels());
    println!("   Format: {:?}", config.sample_format());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_for_speech;

    #[test]
    fn strips_markdown_emphasis() {
        assert_eq!(sanitize_for_speech("**Hello** _world_"), "Hello world");
    }

    #[test]
    fn drops_code_fences_and_backticks() {
        let input = "Here you go:\n```rust\nlet x = 1;\n```\nRun `cargo test`.";

        assert_eq!(sanitize_for_speech(input), "Here you go: Run cargo test.");
    }

    #[test]
    fn removes_emoji_and_expands_symbols() {
        assert_eq!(
            sanitize_for_speech("Done! 🎉 50% + 1 = 2"),
            "Done! 50 percent plus 1 equals 2"
        );
    }

    #[test]
    fn flattens_headings_and_lists() {
        assert_eq!(
            sanitize_for_speech("# Title\n- first\n- second"),
            "Title first second"
        );
    }

    #[test]
    fn replaces_links_with_label_and_urls_with_words() {
        assert_eq!(
            sanitize_for_speech("See [the docs](https://example.com/docs) at https://x.io"),
            "See the docs at a link"
        );
    }
}
