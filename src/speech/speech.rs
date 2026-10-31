// src/speech.rs - Fixed audio capture (mono downmix, correct U16 handling,
// silence padding) and upgraded TTS (Piper neural voice, espeak-ng fallback)
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::env;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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

/// Keep combining characters attached to their base letters. Rust's
/// `char::is_alphanumeric` excludes Unicode marks, which silently removes
/// vowel signs and viramas from decomposed text (notably Indic and Arabic
/// scripts) during speech cleanup.
fn is_combining_mark(character: char) -> bool {
    matches!(
        character as u32,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06ED
            | 0x0900..=0x0903
            | 0x093A..=0x094F
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0981..=0x0983
            | 0x09BC..=0x09CD
            | 0x09D7
            | 0x09E2..=0x09E3
            | 0x0A01..=0x0A03
            | 0x0A3C
            | 0x0A3E..=0x0A4D
            | 0x0A70..=0x0A71
            | 0x0A75
            | 0x0A81..=0x0A83
            | 0x0ABC
            | 0x0ABE..=0x0ACD
            | 0x0AD7
            | 0x0AE2..=0x0AE3
            | 0x0B01..=0x0B03
            | 0x0B3C
            | 0x0B3E..=0x0B4D
            | 0x0B55..=0x0B57
            | 0x0B62..=0x0B63
            | 0x0C00..=0x0C04
            | 0x0C3C..=0x0C44
            | 0x0C46..=0x0C4D
            | 0x0C55..=0x0C56
            | 0x0C62..=0x0C63
            | 0x0C81..=0x0C83
            | 0x0CBC
            | 0x0CBE..=0x0CCD
            | 0x0CD5..=0x0CD6
            | 0x0CE2..=0x0CE3
            | 0x0D00..=0x0D03
            | 0x0D3B..=0x0D4D
            | 0x0D57
            | 0x0D62..=0x0D63
            | 0x0B82..=0x0B83
            | 0x0BBE..=0x0BCD
            | 0x0BD7
    )
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
            c if c.is_alphanumeric() || c.is_whitespace() || is_combining_mark(c) => spoken.push(c),
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

fn next_piper_output_path() -> std::path::PathBuf {
    // Created owner-only (0600) under the private runtime directory, so another
    // local account can neither read nor guess the generated speech.
    crate::paths::private_file_or_path("hyusk_tts_", "wav")
}

struct TemporaryAudioFile(std::path::PathBuf);

impl Drop for TemporaryAudioFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
// use std::time::Duration;
// use tokio::time::sleep;

#[derive(Clone)]
pub struct SpeechToText {
    context: Arc<std::sync::Mutex<Option<WhisperContext>>>,
    inference_permit: Arc<Semaphore>,
    model_path: String,
    sample_rate: u32,
}

#[derive(Clone)]
pub struct TextToSpeech;

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn stt_inference_timeout() -> std::time::Duration {
    let seconds = env::var("HYUSK_STT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=600).contains(seconds))
        .unwrap_or(30);
    std::time::Duration::from_secs(seconds)
}

#[cfg(test)]
static WHISPER_ABORT_CALLBACK_OBSERVED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static WHISPER_NATIVE_CALLBACK_ENTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static WHISPER_TEST_HOLD_ABORT_CALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "sonic")]
fn stream_pulse_input(
    device: &str,
    frames: tokio::sync::mpsc::Sender<Vec<u8>>,
    speech_onsets: tokio::sync::mpsc::Sender<()>,
    cancel: &CancellationToken,
) -> Result<()> {
    use std::io::Read;

    let mut child = std::process::Command::new("parec")
        .args(["--raw", "--format=s16le", "--rate=16000", "--channels=1"])
        .arg(format!("--device={device}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start echo-cancelled microphone capture (parec)")?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("parec has no audio output");
    };
    // `read_exact` on a pipe can block forever if the audio server stalls.
    // Keep that blocking read off this worker so cancellation can still kill
    // parec and close the pipe promptly.
    let (reader_tx, reader_rx) = mpsc::sync_channel(2);
    let reader = std::thread::spawn(move || loop {
        let mut frame = [0_u8; 1_024]; // 32 ms at 16 kHz, 16-bit mono
        let result = stdout.read_exact(&mut frame).map(|()| frame);
        let failed = result.is_err();
        if reader_tx.send(result).is_err() || failed {
            break;
        }
    });
    let mut onset_detector = SpeechOnsetDetector::new(320);
    let result = loop {
        if cancel.is_cancelled() {
            break Ok(());
        }
        let frame = match reader_rx.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(Ok(frame)) => frame,
            Ok(Err(error)) => {
                break Err(anyhow::anyhow!(
                    "Echo-cancelled microphone stopped: {error}"
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(anyhow::anyhow!("Echo-cancelled microphone reader stopped"));
            }
        };
        for sample in frame.chunks_exact(2) {
            let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0;
            if onset_detector.push(value) {
                pulse_speech_onset(&speech_onsets);
            }
        }
        match send_frame_cancellable(&frames, frame.to_vec(), Some(cancel)) {
            FrameSendOutcome::Sent => {}
            FrameSendOutcome::Cancelled => break Ok(()),
            FrameSendOutcome::ReceiverClosed => {
                if cancel.is_cancelled() {
                    break Ok(());
                }
                break Err(anyhow::anyhow!(
                    "Echo-cancelled microphone audio receiver stopped"
                ));
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    drop(reader_rx);
    let _ = reader.join();
    result
}

pub(crate) fn input_device(host: &cpal::Host) -> Result<cpal::Device> {
    if let Some(requested) = env::var_os("HYUSK_INPUT_DEVICE") {
        let requested = requested.to_string_lossy();
        let devices = host
            .input_devices()
            .context("Could not enumerate audio input devices")?;
        let mut available = Vec::new();
        let mut exact = None;
        let mut partial = None;
        for device in devices {
            let name = device.name().unwrap_or_else(|_| "<unnamed>".to_string());
            available.push(name.clone());
            if name == requested {
                exact = Some(device);
                break;
            }
            if partial.is_none() && name.to_lowercase().contains(&requested.to_lowercase()) {
                partial = Some(device);
            }
        }
        if let Some(device) = exact.or(partial) {
            return Ok(device);
        }
        anyhow::bail!(
            "HYUSK_INPUT_DEVICE '{requested}' did not match an audio input. Available inputs: {}",
            available.join(", ")
        );
    }

    host.default_input_device()
        .context("No input device available (set HYUSK_INPUT_DEVICE to select one)")
}

impl SpeechToText {
    pub fn new(model_path: &str) -> Result<Self> {
        Ok(Self {
            // Microphone capture is shared by local STT and speech-to-speech
            // providers. Defer loading Whisper until transcription is actually
            // requested so Nova Sonic does not load an unused second speech
            // recognizer at startup.
            context: Arc::new(std::sync::Mutex::new(None)),
            inference_permit: Arc::new(Semaphore::new(1)),
            model_path: model_path.to_string(),
            sample_rate: 16000,
        })
    }

    pub async fn transcribe_from_microphone(
        &self,
        duration_secs: f32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request_cancel = cancel.child_token();
        let _capture_guard = CancelOnDrop(request_cancel.clone());
        if request_cancel.is_cancelled() {
            return Ok(String::new());
        }
        println!(
            "🎤 Recording up to {} seconds (stops after speech ends)...",
            duration_secs
        );

        let recorder = self.clone();
        let capture_cancel = request_cancel.clone();
        let audio_data = tokio::task::spawn_blocking(move || {
            recorder.record_audio(duration_secs, None, Some(&capture_cancel))
        })
        .await
        .context("Microphone capture task failed")??;

        if audio_data.is_empty() || request_cancel.is_cancelled() {
            return Ok(String::new());
        }

        println!(
            "📊 Recorded {} samples ({:.2}s @ {}Hz)",
            audio_data.len(),
            audio_data.len() as f32 / self.sample_rate as f32,
            self.sample_rate
        );

        self.transcribe_audio(&audio_data, &request_cancel).await
    }

    /// Capture on a blocking worker and send 16 kHz mono PCM frames as the mic
    /// produces them. Returns whether the local endpointer found speech.
    #[cfg(feature = "sonic")]
    pub fn stream_audio_for_speech_model(
        &self,
        duration_secs: f32,
        frames: tokio::sync::mpsc::Sender<Vec<u8>>,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        Ok(!self
            .record_audio(duration_secs, Some(&frames), Some(cancel))?
            .is_empty())
    }

    /// Continuously stream microphone audio as 16 kHz mono PCM until cancelled.
    /// Unlike `stream_audio_for_speech_model`, this does not buffer an
    /// utterance or run a local endpointer.
    #[cfg(feature = "sonic")]
    pub fn stream_continuous_audio_for_speech_model(
        &self,
        frames: tokio::sync::mpsc::Sender<Vec<u8>>,
        speech_onsets: tokio::sync::mpsc::Sender<()>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        // CPAL's ALSA device enumeration does not expose PipeWire's virtual
        // echo-cancel source. Capture that source through Pulse's native
        // compatibility client, still in the same bounded frame channel.
        if let Ok(device) = env::var("HYUSK_SONIC_INPUT_DEVICE") {
            if !device.trim().is_empty() {
                return stream_pulse_input(device.trim(), frames, speech_onsets, cancel);
            }
        }
        let host = cpal::default_host();
        let device = input_device(&host)?;
        let config = device
            .default_input_config()
            .context("Failed to get input config")?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();
        // A bounded queue caps capture-side memory at roughly 250 ms. If the
        // consumer falls behind, dropping input is preferable to unbounded lag.
        let (tx, rx) = mpsc::sync_channel::<f32>(4096);
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32;
                        let _ = tx.try_send(mono);
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
                        let _ = tx.try_send(mono);
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    for frame in data.chunks(channels) {
                        let mono = frame
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .sum::<f32>()
                            / channels as f32;
                        let _ = tx.try_send(mono);
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            _ => anyhow::bail!("Unsupported audio format"),
        };
        stream.play()?;

        let hp_alpha = {
            let dt = 1.0 / sample_rate as f32;
            let rc = 1.0 / (2.0 * std::f32::consts::PI * 90.0);
            rc / (rc + dt)
        };
        let output_step = sample_rate as f64 / self.sample_rate as f64;
        let mut next_output_position = 0.0_f64;
        let mut input_position = 0usize;
        let mut previous_input = 0.0_f32;
        let mut hp_prev_in = 0.0_f32;
        let mut hp_prev_out = 0.0_f32;
        let mut live_pcm = Vec::with_capacity(1_024);
        let mut denoiser = voice_denoise_enabled().then(LiveDenoiser::new);
        let mut onset_detector = SpeechOnsetDetector::new(self.sample_rate as usize / 50);

        while !cancel.is_cancelled() {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(sample) => {
                    let filtered = hp_alpha * (hp_prev_out + sample - hp_prev_in);
                    hp_prev_in = sample;
                    hp_prev_out = filtered;
                    let position = input_position as f64;
                    while next_output_position <= position {
                        let fraction =
                            (next_output_position - (position - 1.0)).clamp(0.0, 1.0) as f32;
                        let interpolated = previous_input * (1.0 - fraction) + filtered * fraction;
                        if let Some(state) = denoiser.as_mut() {
                            if let Some(clean) = state.push(interpolated) {
                                for value in clean {
                                    if onset_detector.push(value) {
                                        pulse_speech_onset(&speech_onsets);
                                    }
                                    if !push_live_sample(
                                        value,
                                        &mut live_pcm,
                                        &frames,
                                        Some(cancel),
                                    )? {
                                        return Ok(());
                                    }
                                }
                            }
                        } else {
                            if onset_detector.push(interpolated) {
                                pulse_speech_onset(&speech_onsets);
                            }
                            if !push_live_sample(
                                interpolated,
                                &mut live_pcm,
                                &frames,
                                Some(cancel),
                            )? {
                                return Ok(());
                            }
                        }
                        next_output_position += output_step;
                    }
                    previous_input = filtered;
                    input_position += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn record_audio(
        &self,
        duration_secs: f32,
        live_frames: Option<&tokio::sync::mpsc::Sender<Vec<u8>>>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<f32>> {
        let host = cpal::default_host();
        let device = input_device(&host)?;
        println!("🎤 Using input device: {}", device.name()?);

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
        let mut live_pcm = Vec::with_capacity(1_024);
        let mut live_denoiser = voice_denoise_enabled().then(LiveDenoiser::new);
        let mut clean_audio =
            Vec::with_capacity((duration_secs * self.sample_rate as f32) as usize);
        let mut saw_neural_speech = false;
        let mut next_output_position = 0.0_f64;
        let output_step = sample_rate as f64 / self.sample_rate as f64;
        let mut previous_filtered = 0.0_f32;

        while collected < frame_count {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                break;
            }
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(sample) => {
                    let filtered = hp_alpha * (hp_prev_out + sample - hp_prev_in);
                    hp_prev_in = sample;
                    hp_prev_out = filtered;

                    if live_frames.is_some() || live_denoiser.is_some() {
                        let position = collected as f64;
                        while next_output_position <= position {
                            let fraction =
                                (next_output_position - (position - 1.0)).clamp(0.0, 1.0) as f32;
                            let interpolated =
                                previous_filtered * (1.0 - fraction) + filtered * fraction;
                            if let Some(denoiser) = live_denoiser.as_mut() {
                                if let Some(clean) = denoiser.push(interpolated) {
                                    clean_audio.extend_from_slice(&clean);
                                    if let Some(sender) = live_frames {
                                        for sample in clean {
                                            if !push_live_sample(
                                                sample,
                                                &mut live_pcm,
                                                sender,
                                                cancel,
                                            )? {
                                                return Ok(Vec::new());
                                            }
                                        }
                                    }
                                }
                            } else if let Some(sender) = live_frames {
                                if !push_live_sample(interpolated, &mut live_pcm, sender, cancel)? {
                                    return Ok(Vec::new());
                                }
                            }
                            next_output_position += output_step;
                        }
                        previous_filtered = filtered;
                    }

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
                    let probability = live_denoiser.as_ref().map(|state| state.speech_probability);
                    let neural_speech = probability.is_some_and(|value| value >= 0.6);
                    if total_frames <= warmup_frames && !neural_speech {
                        warmup_min = warmup_min.min(frame_rms);
                        noise_floor = warmup_min.max(1e-4);

                        continue;
                    }

                    if noise_frames == 0 {
                        noise_floor = warmup_min.clamp(1e-4, 0.02);
                    }

                    noise_frames += 1;

                    let threshold = (noise_floor * 2.0).max(min_speech_rms);

                    if env::var_os("STT_DEBUG").is_some() && total_frames.is_multiple_of(5) {
                        eprintln!(
                            "[stt-debug] frame {total_frames} rms {frame_rms:.4} thr {threshold:.4} noise {noise_floor:.4} saw {saw_speech}"
                        );
                    }

                    if capture_frame_is_speech(frame_rms, threshold, probability, saw_speech) {
                        speech_frames += 1;
                        silence_frames = 0;

                        if speech_frames >= speech_start_frames {
                            saw_speech = true;
                            saw_neural_speech |= neural_speech;
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

                    if !saw_speech
                        && total_frames.saturating_sub(warmup_frames) >= no_speech_timeout_frames
                    {
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

        if let Some(denoiser) = live_denoiser.as_mut() {
            let tail = denoiser.finish();
            clean_audio.extend_from_slice(&tail);
            if let Some(sender) = live_frames {
                for clean in tail {
                    if !push_live_sample(clean, &mut live_pcm, sender, cancel)? {
                        return Ok(Vec::new());
                    }
                }
            }
        }
        if let Some(sender) = live_frames {
            if !live_pcm.is_empty() {
                match send_frame_cancellable(sender, live_pcm, cancel) {
                    FrameSendOutcome::Sent => {}
                    FrameSendOutcome::Cancelled => return Ok(Vec::new()),
                    FrameSendOutcome::ReceiverClosed
                        if cancel.is_some_and(CancellationToken::is_cancelled) =>
                    {
                        return Ok(Vec::new());
                    }
                    FrameSendOutcome::ReceiverClosed => {
                        anyhow::bail!("Live microphone audio receiver stopped")
                    }
                }
            }
        }

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

        if !saw_speech || cancel.is_some_and(CancellationToken::is_cancelled) {
            println!("🔇 No sustained speech detected; skipping transcription");
            return Ok(Vec::new());
        }

        // Remove DC offset and sub-vocal rumble before anything else. Laptop
        // captures carry most of their energy below ~100 Hz, which Whisper
        // otherwise turns into hallucinated text.
        if live_denoiser.is_some() {
            audio_samples = clean_audio;
        } else {
            audio_samples = high_pass(&audio_samples, sample_rate as f32);
            if sample_rate != self.sample_rate {
                audio_samples =
                    self.resample_audio(&audio_samples, sample_rate, self.sample_rate)?;
            }
        }
        // Normalize after suppression, not before: avoid boosting room noise
        // into the denoiser and preserve a single continuous filter state.
        self.normalize_audio(&mut audio_samples);

        if !saw_neural_speech && !self.has_speech_energy(&audio_samples) {
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

    async fn transcribe_audio(
        &self,
        audio_data: &[f32],
        cancel: &CancellationToken,
    ) -> Result<String> {
        if audio_data.is_empty() || cancel.is_cancelled() {
            return Ok(String::new());
        }
        println!("🔍 Transcribing {} samples...", audio_data.len());

        let request_cancel = cancel.child_token();
        let _drop_guard = CancelOnDrop(request_cancel.clone());
        let timeout = stt_inference_timeout();
        let deadline = tokio::time::Instant::now() + timeout;
        let acquire = self.inference_permit.clone().acquire_owned();
        let permit = tokio::select! {
            _ = request_cancel.cancelled() => return Ok(String::new()),
            result = tokio::time::timeout_at(deadline, acquire) => match result {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => anyhow::bail!("Whisper inference semaphore was closed"),
                Err(_) => anyhow::bail!("Whisper inference timed out waiting for an available worker"),
            }
        };

        if request_cancel.is_cancelled() {
            return Ok(String::new());
        }

        let context = Arc::clone(&self.context);
        let model_path = self.model_path.clone();
        let audio_data = audio_data.to_vec();
        let worker_cancel = request_cancel.clone();

        // Whisper is CPU-heavy; run it (and the segment extraction, which
        // reads state written by `full`) on the blocking pool so the async
        // runtime (the event loop, tool execution, cancellation) stays live.
        let worker = tokio::task::spawn_blocking(move || {
            let _permit: OwnedSemaphorePermit = permit;
            if worker_cancel.is_cancelled() {
                return Ok(String::new());
            }

            let mut context_slot = context
                .lock()
                .map_err(|_| anyhow::anyhow!("Whisper model lock was poisoned"))?;
            if context_slot.is_none() {
                let parameters = WhisperContextParameters::new();
                let loaded = WhisperContext::new_with_params(&model_path, parameters).map_err(|error| {
                    anyhow::anyhow!(
                        "Failed to load Whisper model '{}': {:?}. Download with: curl -L -o {} https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
                        model_path,
                        error,
                        model_path
                    )
                })?;
                println!("✅ Loaded Whisper model: {model_path}");
                *context_slot = Some(loaded);
            }
            if worker_cancel.is_cancelled() {
                return Ok(String::new());
            }

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            // Use the machine's cores (capped so we do not oversubscribe an
            // SMT machine); the old hard-coded 4 left modern CPUs idle.
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
            // This is an independent short recording, not a continuation of a
            // longer stream; carrying old context can bias the next request.
            params.set_no_context(true);
            params.set_single_segment(false);
            params.set_max_tokens(256);
            params.set_audio_ctx(0);
            params.set_offset_ms(0);
            params.set_duration_ms(0);
            params.set_thold_pt(0.02);
            params.set_thold_ptsum(0.02);
            params.set_max_len(0);
            params.set_entropy_thold(2.4);
            params.set_logprob_thold(-1.0);
            // Loosen the no-speech threshold for quieter or farther mics.
            params.set_no_speech_thold(0.55);
            params.set_length_penalty(-1.0);
            params.set_max_initial_ts(1.0);

            let context = context_slot
                .as_ref()
                .expect("Whisper context was initialized above");
            let mut state = context
                .create_state()
                .map_err(|error| anyhow::anyhow!("Failed to create Whisper state: {error:?}"))?;
            // The state owns a reference to the context internals. The
            // semaphore still serializes use, so release this lock now.
            drop(context_slot);

            // whisper-rs 0.16's safe callback setter has a mismatched erased
            // callback cast and drops its ownership of the boxed closure. Use
            // the raw API with a concrete, scoped payload instead. The payload
            // remains alive until `full` returns; the callback is synchronous
            // within that call and never escapes this blocking worker.
            unsafe extern "C" fn should_abort(user_data: *mut std::ffi::c_void) -> bool {
                // SAFETY: user_data points to the CancellationToken box below,
                // which stays alive for the entire synchronous state.full call.
                let token = unsafe { &*(user_data as *const CancellationToken) };
                #[cfg(test)]
                {
                    WHISPER_NATIVE_CALLBACK_ENTERED
                        .store(true, std::sync::atomic::Ordering::Release);
                    while WHISPER_TEST_HOLD_ABORT_CALLBACK
                        .load(std::sync::atomic::Ordering::Acquire)
                        && !token.is_cancelled()
                    {
                        std::thread::yield_now();
                    }
                }
                let cancelled = token.is_cancelled();
                #[cfg(test)]
                if cancelled {
                    WHISPER_ABORT_CALLBACK_OBSERVED
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                cancelled
            }
            let abort_token = Box::new(worker_cancel.clone());
            let abort_user_data = (&*abort_token as *const CancellationToken)
                .cast_mut()
                .cast::<std::ffi::c_void>();
            // SAFETY: the callback and its matching concrete userdata are
            // installed together and used only by the synchronous full call.
            unsafe {
                params.set_abort_callback(Some(should_abort));
                params.set_abort_callback_user_data(abort_user_data);
            }

            let full_result = state.full(params, &audio_data);
            if worker_cancel.is_cancelled() {
                return Ok(String::new());
            }
            full_result.map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))?;

            let num_segments = state.full_n_segments();
            println!("📝 Got {} segments", num_segments);

            if num_segments == 0 {
                return Ok(String::new());
            }

            let mut full_text = String::new();
            for segment in state.as_iter() {
                if worker_cancel.is_cancelled() {
                    return Ok(String::new());
                }
                let text = segment
                    .to_str()
                    .map_err(|e| anyhow::anyhow!("Failed to get segment text: {e:?}"))?;
                if !text.is_empty() {
                    full_text.push_str(text);
                    full_text.push(' ');
                }
            }

            let result = clean_transcript(&full_text);
            Ok(result)
        });

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::select! {
            _ = request_cancel.cancelled() => Ok(String::new()),
            result = tokio::time::timeout(remaining, worker) => match result {
                Ok(Ok(_)) if request_cancel.is_cancelled() => Ok(String::new()),
                Ok(Ok(result)) => result,
                Ok(Err(_error)) if request_cancel.is_cancelled() => Ok(String::new()),
                Ok(Err(error)) => Err(error).context("Whisper task panicked"),
                Err(_) => {
                    request_cancel.cancel();
                    anyhow::bail!("Whisper inference timed out after {} seconds", timeout.as_secs())
                }
            }
        }
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

fn capture_frame_is_speech(
    rms: f32,
    threshold: f32,
    probability: Option<f32>,
    speaking: bool,
) -> bool {
    match probability {
        // Neural VAD rescues quieter speech that a noisy warmup can hide.
        Some(value) => rms >= 0.001 && value >= if speaking { 0.35 } else { 0.6 },
        None => rms >= threshold,
    }
}

fn clean_transcript(text: &str) -> String {
    let mut text = text.to_string();
    for marker in [
        "[BLANK_AUDIO]",
        "[NO_SPEECH]",
        "[SILENCE]",
        "[MUSIC]",
        "(silence)",
        "(music)",
    ] {
        text = text.replace(marker, "");
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

enum FrameSendOutcome {
    Sent,
    Cancelled,
    ReceiverClosed,
}

/// Send from a blocking audio worker without becoming unresponsive to task
/// cancellation when the bounded receiver stops consuming frames.
fn send_frame_cancellable(
    sender: &tokio::sync::mpsc::Sender<Vec<u8>>,
    mut frame: Vec<u8>,
    cancel: Option<&CancellationToken>,
) -> FrameSendOutcome {
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return FrameSendOutcome::Cancelled;
        }
        match sender.try_send(frame) {
            Ok(()) => return FrameSendOutcome::Sent,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return FrameSendOutcome::ReceiverClosed;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(returned_frame)) => {
                frame = returned_frame;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

fn push_live_sample(
    sample: f32,
    frame: &mut Vec<u8>,
    sender: &tokio::sync::mpsc::Sender<Vec<u8>>,
    cancel: Option<&CancellationToken>,
) -> Result<bool> {
    let pcm = (sample * 5.0).clamp(-1.0, 1.0);
    frame.extend_from_slice(&((pcm * i16::MAX as f32).round() as i16).to_le_bytes());
    if frame.len() == 1_024 {
        let complete = std::mem::replace(frame, Vec::with_capacity(1_024));
        match send_frame_cancellable(sender, complete, cancel) {
            FrameSendOutcome::Sent => Ok(true),
            FrameSendOutcome::Cancelled => Ok(false),
            FrameSendOutcome::ReceiverClosed
                if cancel.is_some_and(CancellationToken::is_cancelled) =>
            {
                Ok(false)
            }
            FrameSendOutcome::ReceiverClosed => {
                anyhow::bail!("Live microphone audio receiver stopped")
            }
        }
    } else {
        Ok(true)
    }
}

#[cfg(feature = "sonic")]
fn pulse_speech_onset(sender: &tokio::sync::mpsc::Sender<()>) {
    let _ = sender.try_send(());
}

#[cfg(feature = "sonic")]
fn align_pcm16(pending: &mut Option<u8>, pcm: &[u8]) -> Vec<u8> {
    let mut aligned = Vec::with_capacity(pcm.len() + usize::from(pending.is_some()));
    if let Some(byte) = pending.take() {
        aligned.push(byte);
    }
    aligned.extend_from_slice(pcm);
    if aligned.len() % 2 != 0 {
        *pending = aligned.pop();
    }
    aligned
}

/// Frame based onset detector for the continuous 16 kHz microphone stream.
/// Noise is tracked only during inactive periods, preventing speech from
/// raising its own threshold. Three active frames give modest transient
/// rejection while keeping onset latency near 60 ms.
#[cfg(feature = "sonic")]
struct SpeechOnsetDetector {
    frame_samples: usize,
    frame_energy: f32,
    samples_in_frame: usize,
    noise_floor: f32,
    active_frames: usize,
    quiet_frames: usize,
    speaking: bool,
}

#[cfg(feature = "sonic")]
impl SpeechOnsetDetector {
    fn new(frame_samples: usize) -> Self {
        Self {
            frame_samples: frame_samples.max(1),
            frame_energy: 0.0,
            samples_in_frame: 0,
            noise_floor: 0.002,
            active_frames: 0,
            quiet_frames: 0,
            speaking: false,
        }
    }

    /// Returns true exactly once when a new utterance is detected.
    fn push(&mut self, sample: f32) -> bool {
        self.frame_energy += sample * sample;
        self.samples_in_frame += 1;
        if self.samples_in_frame < self.frame_samples {
            return false;
        }

        let rms = (self.frame_energy / self.samples_in_frame as f32).sqrt();
        self.frame_energy = 0.0;
        self.samples_in_frame = 0;
        let threshold = (self.noise_floor * 2.8).max(0.009);

        if rms >= threshold {
            self.active_frames += 1;
            self.quiet_frames = 0;
            if !self.speaking && self.active_frames >= 3 {
                self.speaking = true;
                return true;
            }
        } else {
            self.active_frames = 0;
            if self.speaking {
                self.quiet_frames += 1;
                if self.quiet_frames >= 15 {
                    self.speaking = false;
                    self.quiet_frames = 0;
                }
            } else {
                self.noise_floor = self.noise_floor * 0.97 + rms * 0.03;
            }
        }
        false
    }
}

/// The same RNNoise preprocessing as `denoise_for_stt`, but retaining its
/// 10 ms frame state so audio can leave the microphone without waiting for the
/// whole utterance. Its output adds at most one 10 ms frame of latency.
struct LiveDenoiser {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    input: Vec<f32>,
    output: Vec<f32>,
    previous: Option<f32>,
    speech_probability: f32,
}

impl LiveDenoiser {
    fn new() -> Self {
        let mut state = nnnoiseless::DenoiseState::new();
        let silence = vec![0.0; nnnoiseless::DenoiseState::FRAME_SIZE];
        let mut output = vec![0.0; nnnoiseless::DenoiseState::FRAME_SIZE];
        for _ in 0..8 {
            state.process_frame(&mut output, &silence);
        }
        Self {
            state,
            input: Vec::with_capacity(nnnoiseless::DenoiseState::FRAME_SIZE),
            output,
            previous: None,
            speech_probability: 0.0,
        }
    }

    fn push(&mut self, current: f32) -> Option<Vec<f32>> {
        let previous = self.previous.replace(current)?;
        self.input.extend(
            [
                previous,
                previous + (current - previous) / 3.0,
                previous + (current - previous) * 2.0 / 3.0,
            ]
            .map(|sample| sample * 32768.0),
        );
        if self.input.len() != nnnoiseless::DenoiseState::FRAME_SIZE {
            return None;
        }
        self.speech_probability = self.state.process_frame(&mut self.output, &self.input);
        self.input.clear();
        Some(
            self.output
                .chunks_exact(3)
                .map(|values| {
                    ((values[0] + values[1] + values[2]) / 3.0 / 32768.0).clamp(-1.0, 1.0)
                })
                .collect(),
        )
    }

    fn finish(&mut self) -> Vec<f32> {
        let Some(last) = self.previous else {
            return Vec::new();
        };
        let mut clean = self.push(last).unwrap_or_default();
        while !self.input.is_empty() {
            if let Some(frame) = self.push(0.0) {
                clean.extend(frame);
            }
        }
        clean
    }
}

/// RNNoise reduces persistent environmental noise before Whisper transcribes a
/// command. It is denoising rather than biometric speaker separation: nearby
/// voices can still be heard by a single laptop microphone.
#[cfg(test)]
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

    /// Speak while remaining cancellable.
    ///
    /// Playback uses tokio child processes with `kill_on_drop`, so a new
    /// message or wake word stops the current TTS playback.
    pub async fn speak_cancellable(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        // Strip Markdown/emoji so the engine speaks words, not formatting.
        let text = sanitize_for_speech(text);

        match self.speak_piper_cancellable(&text, cancel).await {
            Ok(()) => Ok(()),

            Err(error) => {
                if cancel.is_cancelled() {
                    return Ok(());
                }

                eprintln!("ℹ️ Piper unavailable ({}), falling back to espeak", error);

                self.speak_espeak_cancellable(&text, cancel).await
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

        let tmp_wav = TemporaryAudioFile(next_piper_output_path());

        let mut command = AsyncCommand::new("piper");
        command.args([
            "--model",
            &model_path,
            "--output_file",
            tmp_wav.0.to_str().unwrap(),
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
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Ok(());
                }
                result = stdin.write_all(text.as_bytes()) => {
                    result.context("Failed to write text to piper stdin")?;
                }
            }
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

        let mut paplay = AsyncCommand::new("paplay");
        if let Ok(device) = env::var("HYUSK_OUTPUT_DEVICE") {
            if !device.trim().is_empty() {
                paplay.arg(format!("--device={}", device.trim()));
            }
        }
        let mut play = match paplay
            .arg(&tmp_wav.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,

            Err(_) => AsyncCommand::new("aplay")
                .arg(&tmp_wav.0)
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

/// A single player for Nova Sonic's 24 kHz mono PCM response stream.
#[cfg(feature = "sonic")]
pub struct Pcm16Player {
    child: tokio::process::Child,
    trailing_byte: Option<u8>,
}

#[cfg(feature = "sonic")]
impl Pcm16Player {
    fn spawn_player() -> Result<tokio::process::Child> {
        use tokio::process::Command as AsyncCommand;

        if let Ok(device) = env::var("HYUSK_OUTPUT_DEVICE") {
            if !device.trim().is_empty() {
                return AsyncCommand::new("paplay")
                    .args([
                        "--raw",
                        "--format=s16le",
                        "--rate=24000",
                        "--channels=1",
                        "--latency-msec=100",
                    ])
                    .arg(format!("--device={}", device.trim()))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .context(
                        "Nova Sonic audio playback requires paplay for the selected output device",
                    );
            }
        }
        AsyncCommand::new("aplay")
            .args(["-q", "-t", "raw", "-f", "S16_LE", "-r", "24000", "-c", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Nova Sonic audio playback requires aplay (ALSA utils)")
    }

    pub fn start() -> Result<Self> {
        let child = Self::spawn_player()?;
        Ok(Self {
            child,
            trailing_byte: None,
        })
    }

    pub async fn write(&mut self, pcm: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let aligned = align_pcm16(&mut self.trailing_byte, pcm);
        if !aligned.is_empty() {
            self.child
                .stdin
                .as_mut()
                .context("Nova Sonic audio player is closed")?
                .write_all(&aligned)
                .await
                .context("Could not feed audio to the PCM player")?;
        }
        Ok(())
    }

    pub async fn finish(mut self) -> Result<()> {
        if self.trailing_byte.is_some() {
            let _ = self.stop().await;
            anyhow::bail!("Nova Sonic PCM stream ended mid-sample");
        }
        self.child.stdin.take();
        let output = self
            .child
            .wait_with_output()
            .await
            .context("Could not wait for aplay")?;
        if !output.status.success() {
            anyhow::bail!(
                "aplay failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Immediately stop playback and discard audio buffered by aplay/device.
    /// The player is closed after this call; start a new player to speak again.
    pub async fn stop(&mut self) -> Result<()> {
        self.child.stdin.take();
        if self.child.try_wait()?.is_none() {
            self.child
                .start_kill()
                .context("Could not stop Nova Sonic audio playback")?;
        }
        // Reap the child so repeated stop calls are harmless and do not leave
        // an aplay process behind.
        let _ = self.child.wait().await?;
        Ok(())
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
    #[cfg(feature = "sonic")]
    use super::{align_pcm16, SpeechOnsetDetector};
    use super::{
        capture_frame_is_speech, clean_transcript, denoise_for_stt, next_piper_output_path,
        sanitize_for_speech, send_frame_cancellable, FrameSendOutcome, LiveDenoiser,
    };

    struct ResetAbortCallbackHold;

    impl Drop for ResetAbortCallbackHold {
        fn drop(&mut self) {
            super::WHISPER_TEST_HOLD_ABORT_CALLBACK
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    #[test]
    fn transcript_annotations_never_become_user_requests() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO] "), "");
        assert_eq!(clean_transcript("[MUSIC] (silence)"), "");
        assert_eq!(clean_transcript("[BLANK_AUDIO] set a timer"), "set a timer");
        assert_eq!(clean_transcript("Open [my notes]"), "Open [my notes]");
    }

    #[test]
    fn neural_capture_vad_rejects_noise_and_keeps_quiet_speech() {
        assert!(!capture_frame_is_speech(0.3, 0.02, Some(0.05), false));
        assert!(capture_frame_is_speech(0.003, 0.02, Some(0.8), false));
        assert!(!capture_frame_is_speech(0.003, 0.02, Some(0.4), false));
        assert!(capture_frame_is_speech(0.003, 0.02, Some(0.4), true));
        assert!(!capture_frame_is_speech(0.0, 0.02, Some(0.8), true));
        assert!(capture_frame_is_speech(0.03, 0.02, None, false));
    }

    #[test]
    fn piper_output_paths_are_unique_and_owner_only() {
        let first = next_piper_output_path();
        let second = next_piper_output_path();

        assert_ne!(first, second);
        assert_eq!(
            first.parent(),
            Some(crate::paths::private_dir().as_path()),
            "{}",
            first.display()
        );

        let name = first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 file name");
        assert!(name.starts_with("hyusk_tts_"), "{name}");
        assert!(name.ends_with(".wav"), "{name}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first)
                .expect("speech file exists")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    #[cfg(feature = "sonic")]
    fn push_vad_frames(vad: &mut SpeechOnsetDetector, level: f32, count: usize) -> usize {
        let mut onsets = 0;
        for _ in 0..count * vad.frame_samples {
            if vad.push(level) {
                onsets += 1;
            }
        }
        onsets
    }

    #[cfg(feature = "sonic")]
    #[test]
    fn onset_detector_rejects_short_transients_and_pulses_once_per_utterance() {
        let mut vad = SpeechOnsetDetector::new(320);
        assert_eq!(push_vad_frames(&mut vad, 0.05, 2), 0);
        assert_eq!(push_vad_frames(&mut vad, 0.05, 8), 1);
        assert_eq!(push_vad_frames(&mut vad, 0.05, 5), 0);
        assert_eq!(push_vad_frames(&mut vad, 0.0, 15), 0);
        assert_eq!(push_vad_frames(&mut vad, 0.05, 3), 1);
    }

    #[cfg(feature = "sonic")]
    #[test]
    fn onset_detector_adapts_to_quiet_noise_without_triggering() {
        let mut vad = SpeechOnsetDetector::new(320);
        assert_eq!(push_vad_frames(&mut vad, 0.001, 200), 0);
        assert_eq!(push_vad_frames(&mut vad, 0.05, 3), 1);
    }

    #[test]
    fn live_denoiser_matches_batch_preprocessing() {
        let samples: Vec<f32> = (0..1_600)
            .map(|index| (index as f32 * 0.047).sin() * 0.1)
            .collect();
        let expected = denoise_for_stt(&samples);
        let mut live = LiveDenoiser::new();
        let mut actual = Vec::new();
        for sample in samples {
            if let Some(frame) = live.push(sample) {
                actual.extend(frame);
            }
        }
        actual.extend(live.finish());
        for (left, right) in actual.iter().zip(expected.iter()).take(1_440) {
            assert!((left - right).abs() < 1e-5);
        }
    }

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
    fn preserves_unicode_combining_marks_in_spoken_text() {
        assert_eq!(
            sanitize_for_speech("cafe\u{301} क\u{93f} தமிழ் മലയാളം"),
            "cafe\u{301} क\u{93f} தமிழ் മലയാളം"
        );
    }

    #[cfg(feature = "sonic")]
    #[test]
    fn pcm16_alignment_carries_bytes_across_arbitrary_chunk_boundaries() {
        let mut pending = None;
        assert_eq!(align_pcm16(&mut pending, &[1]), Vec::<u8>::new());
        assert_eq!(pending, Some(1));
        assert_eq!(align_pcm16(&mut pending, &[2, 3, 4]), vec![1, 2, 3, 4]);
        assert_eq!(pending, None);
        assert_eq!(align_pcm16(&mut pending, &[5, 6, 7]), vec![5, 6]);
        assert_eq!(pending, Some(7));
    }

    #[test]
    fn full_audio_receiver_does_not_block_capture_after_cancellation() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(vec![0]).unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            send_frame_cancellable(&sender, vec![1], Some(&worker_cancel))
        });

        std::thread::sleep(std::time::Duration::from_millis(20));
        cancel.cancel();
        assert!(matches!(
            worker.join().unwrap(),
            FrameSendOutcome::Cancelled
        ));
    }

    #[test]
    fn full_audio_receiver_preserves_frame_order_when_it_resumes() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(vec![0]).unwrap();
        let worker = std::thread::spawn(move || send_frame_cancellable(&sender, vec![1], None));

        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(receiver.try_recv().unwrap(), vec![0]);
        assert!(matches!(worker.join().unwrap(), FrameSendOutcome::Sent));
        assert_eq!(receiver.try_recv().unwrap(), vec![1]);
    }

    #[tokio::test]
    async fn cancelled_microphone_request_never_opens_audio_or_loads_model() {
        let stt = super::SpeechToText::new("/missing/whisper-model.bin").unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            stt.transcribe_from_microphone(8.0, &cancel).await.unwrap(),
            ""
        );
        assert!(stt.context.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn queued_or_pre_cancelled_whisper_requests_do_not_load_a_model() {
        let stt = super::SpeechToText::new("/missing/whisper-model.bin").unwrap();
        let permit = stt.inference_permit.clone().acquire_owned().await.unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_stt = stt.clone();
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            worker_stt
                .transcribe_audio(&[0.0; 16_000], &worker_cancel)
                .await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert_eq!(worker.await.unwrap().unwrap(), "");
        drop(permit);
        assert!(stt.context.lock().unwrap().is_none());

        let already_cancelled = tokio_util::sync::CancellationToken::new();
        already_cancelled.cancel();
        assert_eq!(
            stt.transcribe_audio(&[0.0; 16_000], &already_cancelled)
                .await
                .unwrap(),
            ""
        );
        assert!(stt.context.lock().unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "loads the local Whisper model and runs native inference"]
    async fn whisper_native_cancel_reuses_cached_context() {
        let model =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/ggml-base.en.bin");
        assert!(
            model.exists(),
            "required Whisper model missing: {}",
            model.display()
        );

        let stt = super::SpeechToText::new(&model.to_string_lossy()).unwrap();
        // Synthetic tone avoids microphone access and gives native inference
        // enough work that the abort callback is exercised after startup.
        let audio: Vec<f32> = (0..160_000)
            .map(|sample| (sample as f32 * 0.031).sin() * 0.1)
            .collect();
        let warmup_cancel = tokio_util::sync::CancellationToken::new();
        let _ = stt.transcribe_audio(&audio[..32_000], &warmup_cancel).await;
        assert!(
            stt.context.lock().unwrap().is_some(),
            "Whisper model was not cached during warmup"
        );

        super::WHISPER_ABORT_CALLBACK_OBSERVED.store(false, std::sync::atomic::Ordering::Release);
        super::WHISPER_NATIVE_CALLBACK_ENTERED.store(false, std::sync::atomic::Ordering::Release);
        let _hold_reset = ResetAbortCallbackHold;
        super::WHISPER_TEST_HOLD_ABORT_CALLBACK.store(true, std::sync::atomic::Ordering::Release);
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker_stt = stt.clone();
        let worker_audio = audio.clone();
        let worker = tokio::spawn(async move {
            worker_stt
                .transcribe_audio(&worker_audio, &worker_cancel)
                .await
        });
        // Hold the test callback at a native graph boundary until the spawned
        // async transcription future is dropped. This makes callback
        // observation deterministic even on fast machines.
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !super::WHISPER_NATIVE_CALLBACK_ENTERED.load(std::sync::atomic::Ordering::Acquire)
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native Whisper callback was never entered");
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        super::WHISPER_TEST_HOLD_ABORT_CALLBACK.store(false, std::sync::atomic::Ordering::Release);

        // A fresh request after native abort proves the cached model remains
        // usable and that no partial transcript escaped from the cancelled run.
        let retry = stt
            .transcribe_audio(
                &audio[..32_000],
                &tokio_util::sync::CancellationToken::new(),
            )
            .await;
        assert!(
            retry.is_ok(),
            "Whisper context failed after cancellation: {retry:?}"
        );
        assert!(
            super::WHISPER_ABORT_CALLBACK_OBSERVED.load(std::sync::atomic::Ordering::Acquire),
            "Whisper's native abort callback was not observed"
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
