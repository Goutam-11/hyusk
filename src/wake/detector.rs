use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};
use livekit_wakeword::WakeWordModel;
use tokio::sync::mpsc::Sender;

use crate::types::HyuskEvent;

const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Audio sent to the wake-word model at a time.
///
/// The LiveKit/OpenWakeWord pipeline needs roughly 2 seconds of audio to
/// produce a useful prediction, but the classifier consumes the *last* 16
/// embedding windows (76-frame window, stride 8). A 32000-sample buffer
/// yields only ~15 windows and is silently scored as zero, so the buffer
/// must be comfortably above two seconds for detection to ever fire.
const INFERENCE_BUFFER_SAMPLES: usize = 35_200;

/// Default model inference cadence in milliseconds. The buffered audio is
/// re-scored about every 200 ms. Lower values wake the agent faster but cost
/// more CPU.
const DEFAULT_INFERENCE_STEP_MS: u64 = 200;

/// New samples observed during one 50 ms poll of the microphone buffer.
const POLL_SAMPLES: usize = TARGET_SAMPLE_RATE as usize / 20;

/// Ignore detections for this long after a successful wake word.
const COOLDOWN: Duration = Duration::from_secs(2);

/// Default confidence threshold.
///
/// Start here. Tune this using real recordings from your microphone.
const DEFAULT_THRESHOLD: f32 = 0.93;

/// Accept one qualifying inference window. This keeps wake latency low and
/// avoids cutting off a spoken command while waiting for a second overlap.
const DEFAULT_MIN_HITS: u32 = 1;
const DEFAULT_HIT_WINDOW: Duration = Duration::from_millis(1_000);
const DEFAULT_STRONG_THRESHOLD: f32 = 0.94;

/// Extra confidence required to accept a wake word while the agent is speaking.
///
/// Without echo cancellation the detector hears the agent's own voice; the
/// boost keeps ordinary output from triggering it while a deliberate, close
/// "alexa" barge-in still crosses the bar.
const BARGE_IN_THRESHOLD_BOOST: f32 = 0.05;

/// Print periodic score/level diagnostics when `WAKE_DEBUG=1`, or always in
/// `HYUSK_WAKE_TEST` diagnostics mode.
fn wake_debug_enabled() -> bool {
    if std::env::var_os("HYUSK_WAKE_TEST").is_some() {
        return true;
    }

    std::env::var("WAKE_DEBUG")
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Ignore frames whose level is below this floor even after adaptive
/// noise-floor tracking. Kept low so quiet/far speech still reaches the
/// classifier.
const MIN_RMS: f32 = 0.003;

/// Hand-clap wake is disabled by default. Claps are easily triggered by
/// background noise (doors, coughs, desk bumps), so the wake word is the
/// reliable path. Set `WAKE_CLAP_ENABLED=1` to opt back in.
const DEFAULT_CLAP_ENABLED: bool = false;

/// A transient must be this many times louder than the recent background to
/// count as a hand clap.
const DEFAULT_CLAP_SENSITIVITY: f32 = 6.0;

/// Minimum peak amplitude (in `[-1.0, 1.0]` samples) a clap must reach.
const DEFAULT_CLAP_MIN_PEAK: f32 = 0.03;

/// Clap energy is measured on 10 ms windows.
const CLAP_WINDOW_SAMPLES: usize = 160;

/// One-shot pause/resume signal shared with the wake detector.
///
/// The detector pauses after emitting [`HyuskEvent::WakeWordDetected`]
/// so the speech-to-text owner can record the command using the same
/// microphone. Calling [`WakeResume::resume`] releases the detector.
#[derive(Clone, Default)]
pub struct WakeResume {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl WakeResume {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resume(&self) {
        let (lock, condvar) = &*self.inner;

        *lock.lock().expect("Wake resume mutex poisoned") = true;
        condvar.notify_all();
    }

    /// Test helper; production code always uses the bounded [`Self::wait_for`].
    #[allow(dead_code)]
    fn wait(&self) {
        self.wait_for(MAX_PAUSE);
    }

    /// Block until resumed or `max` elapses. Returns `false` on timeout.
    ///
    /// The runtime is expected to resume the detector after the speech
    /// recording finishes. If the runtime dies or hangs, a plain
    /// `condvar.wait()` would block the microphone thread forever and the
    /// detector would never recover, so every wait is bounded.
    fn wait_for(&self, max: Duration) -> bool {
        use std::time::Instant;

        let (lock, condvar) = &*self.inner;
        let mut resumed = lock.lock().expect("Wake resume mutex poisoned");
        let deadline = Instant::now() + max;

        while !*resumed {
            let now = Instant::now();

            if now >= deadline {
                return false;
            }

            let (new_state, _timed_out) = condvar
                .wait_timeout(resumed, deadline - now)
                .expect("Wake resume mutex poisoned");

            resumed = new_state;
        }

        *resumed = false;

        true
    }
}

/// Longest the detector stays paused waiting for the speech owner before it
/// resumes on its own and clears the stale buffer.
const MAX_PAUSE: Duration = Duration::from_secs(120);

pub struct WakeWordDetector {
    model_paths: Vec<PathBuf>,
    threshold: f32,
    strong_threshold: f32,
    min_hits: u32,
    hit_window: Duration,
    cooldown: Duration,
    min_rms: f32,
    denoise: bool,
    inference_step_ms: u64,
    clap_enabled: bool,
    clap_sensitivity: f32,
    clap_min_peak: f32,
}

impl WakeWordDetector {
    /// Load one or more compatible classifiers.
    ///
    /// Multiple models are scored independently and the highest confidence is
    /// used, which improves recall when several acoustic variants are
    /// available.
    ///
    /// An empty list is allowed: the detector then only listens for hand
    /// claps (see [`WakeWordDetector::with_clap`]). Any path that does not
    /// exist is still an error, so a typo is reported instead of being
    /// silently ignored.
    pub fn new_many(model_paths: Vec<PathBuf>) -> Result<Self> {
        for model_path in &model_paths {
            if !model_path.exists() {
                anyhow::bail!("Wake-word model does not exist: {}", model_path.display());
            }
        }

        Ok(Self {
            model_paths,
            threshold: DEFAULT_THRESHOLD,
            strong_threshold: DEFAULT_STRONG_THRESHOLD,
            min_hits: DEFAULT_MIN_HITS,
            hit_window: DEFAULT_HIT_WINDOW,
            cooldown: COOLDOWN,
            min_rms: MIN_RMS,
            denoise: true,
            inference_step_ms: DEFAULT_INFERENCE_STEP_MS,
            clap_enabled: DEFAULT_CLAP_ENABLED,
            clap_sensitivity: DEFAULT_CLAP_SENSITIVITY,
            clap_min_peak: DEFAULT_CLAP_MIN_PEAK,
        })
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    pub fn with_confirmation(mut self, min_hits: u32, hit_window: Duration) -> Self {
        self.min_hits = min_hits.clamp(1, 5);
        self.hit_window = hit_window.clamp(Duration::from_millis(200), Duration::from_secs(3));
        self
    }

    pub fn with_strong_threshold(mut self, threshold: f32) -> Self {
        self.strong_threshold = threshold.clamp(self.threshold, 1.0);
        self
    }

    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    pub fn with_min_rms(mut self, min_rms: f32) -> Self {
        self.min_rms = min_rms.max(0.0);
        self
    }

    pub fn with_denoise(mut self, denoise: bool) -> Self {
        self.denoise = denoise;
        self
    }

    /// How often (in milliseconds) the buffered audio is re-scored.
    ///
    /// Lower values react faster to the wake word but consume more CPU.
    /// Values are clamped to 50..1000 ms.
    pub fn with_inference_step_ms(mut self, step_ms: u64) -> Self {
        self.inference_step_ms = step_ms.clamp(50, 1_000);
        self
    }

    /// Enable or disable hand-clap wake detection.
    ///
    /// Claps are recognised by a small transient detector that runs on every
    /// new audio chunk, so a clap wakes the agent immediately, without waiting
    /// for the two-second classifier window. No model is required.
    pub fn with_clap(mut self, enabled: bool) -> Self {
        self.clap_enabled = enabled;
        self
    }

    /// A clap must be at least this many times louder than the recent
    /// background level (also bounded below by `with_clap_min_peak`).
    /// Higher values reject more false positives (music, talking, typing).
    pub fn with_clap_sensitivity(mut self, sensitivity: f32) -> Self {
        self.clap_sensitivity = sensitivity.max(1.0);
        self
    }

    /// Minimum peak amplitude a clap must reach, in `[-1.0, 1.0]` samples.
    pub fn with_clap_min_peak(mut self, min_peak: f32) -> Self {
        self.clap_min_peak = min_peak.clamp(0.001, 1.0);
        self
    }

    /// Start continuously listening for "Hey Hyusk" (and hand claps).
    ///
    /// This sends:
    ///
    ///     HyuskEvent::WakeWordDetected
    ///
    /// whenever the model confidence crosses the configured threshold or a
    /// qualifying hand clap is heard. The detector then pauses until
    /// [`WakeResume::resume`] is called so another component can record and
    /// transcribe the spoken command.
    pub async fn run(
        &self,
        event_tx: Sender<HyuskEvent>,
        resume: WakeResume,
        shutdown: Arc<AtomicBool>,
        output_active: Arc<AtomicBool>,
    ) -> Result<()> {
        println!(
            "[Wake] Loading model(s): {}",
            self.model_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let config = WakeConfig {
            model_paths: self.model_paths.clone(),
            threshold: self.threshold,
            strong_threshold: self.strong_threshold,
            min_hits: self.min_hits,
            hit_window: self.hit_window,
            cooldown: self.cooldown,
            min_rms: self.min_rms,
            denoise: self.denoise,
            inference_step_ms: self.inference_step_ms,
            clap_enabled: self.clap_enabled,
            clap_sensitivity: self.clap_sensitivity,
            clap_min_peak: self.clap_min_peak,
        };

        let event_tx = Arc::new(Mutex::new(event_tx));
        let resume = resume.clone();

        tokio::task::spawn_blocking(move || {
            run_microphone_loop(config, event_tx, resume, shutdown, output_active)
        })
        .await
        .context("Wake-word worker task failed")?
    }
}

struct WakeConfig {
    model_paths: Vec<PathBuf>,
    threshold: f32,
    strong_threshold: f32,
    min_hits: u32,
    hit_window: Duration,
    cooldown: Duration,
    min_rms: f32,
    denoise: bool,
    inference_step_ms: u64,
    clap_enabled: bool,
    clap_sensitivity: f32,
    clap_min_peak: f32,
}

struct WakeEvidence {
    min_hits: u32,
    window: Duration,
    label: String,
    hits: u32,
    started: Option<Instant>,
}

impl WakeEvidence {
    fn new(min_hits: u32, window: Duration) -> Self {
        Self {
            min_hits: min_hits.max(1),
            window,
            label: String::new(),
            hits: 0,
            started: None,
        }
    }

    fn reset(&mut self) {
        self.label.clear();
        self.hits = 0;
        self.started = None;
    }

    fn observe(
        &mut self,
        label: &str,
        score: f32,
        threshold: f32,
        strong_threshold: f32,
        now: Instant,
    ) -> bool {
        if score >= strong_threshold {
            self.reset();
            return true;
        }

        if score < threshold {
            if self
                .started
                .is_some_and(|started| now.duration_since(started) > self.window)
            {
                self.reset();
            }
            return false;
        }

        let expired = self
            .started
            .is_some_and(|started| now.duration_since(started) > self.window);
        if self.started.is_none() || expired || self.label != label {
            self.label = label.to_string();
            self.hits = 1;
            self.started = Some(now);
        } else {
            self.hits += 1;
        }

        if self.hits < self.min_hits {
            return false;
        }

        self.reset();
        true
    }
}

fn run_microphone_loop(
    config: WakeConfig,
    event_tx: Arc<Mutex<Sender<HyuskEvent>>>,
    resume: WakeResume,
    shutdown: Arc<AtomicBool>,
    output_active: Arc<AtomicBool>,
) -> Result<()> {
    let WakeConfig {
        model_paths,
        threshold,
        strong_threshold,
        min_hits,
        hit_window,
        cooldown,
        min_rms,
        denoise,
        inference_step_ms,
        clap_enabled,
        clap_sensitivity,
        clap_min_peak,
    } = config;
    println!("[Wake] Loading wake-word detector...");

    let model_path_strings: Vec<String> = model_paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    let mut model = WakeWordModel::new(&model_path_strings, TARGET_SAMPLE_RATE)
        .context("Failed to load wake-word model(s)")?;

    if model_paths.is_empty() {
        println!("[Wake] ⚠ No classifier models; listening for hand claps only");
    } else {
        println!("[Wake] ✓ Wake-word model loaded");
    }

    if clap_enabled {
        println!(
            "[Wake] ✋ Clap wake enabled (sensitivity {:.1}, min peak {:.3})",
            clap_sensitivity, clap_min_peak
        );
    }

    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .context("No default microphone found")?;

    let device_name = device
        .name()
        .unwrap_or_else(|_| "Unknown microphone".to_string());

    println!("[Wake] Microphone: {}", device_name);

    let supported_config = device
        .default_input_config()
        .context("Failed to get microphone configuration")?;

    println!(
        "[Wake] Input: {} Hz, {} channel(s), {:?}",
        supported_config.sample_rate().0,
        supported_config.channels(),
        supported_config.sample_format()
    );

    let input_sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;

    let config = supported_config.config();

    let audio_buffer = Arc::new(Mutex::new(Vec::<f32>::with_capacity(
        INFERENCE_BUFFER_SAMPLES * 2,
    )));

    /*
     * Absolute count of resampled samples pushed so far. The audio buffer is a
     * sliding window, so the clap scanner needs this to tell which samples are
     * new after the buffer starts draining.
     */
    let total_samples = Arc::new(std::sync::atomic::AtomicU64::new(0));

    let err_fn = |error| {
        eprintln!("[Wake] Microphone error: {}", error);
    };

    let stream = match supported_config.sample_format() {
        SampleFormat::F32 => {
            let callback_buffer = Arc::clone(&audio_buffer);
            let total_counter = Arc::clone(&total_samples);
            let resampler = Arc::new(Mutex::new(LinearResampler::new(
                input_sample_rate,
                TARGET_SAMPLE_RATE,
            )));

            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    push_audio(
                        data,
                        channels,
                        input_sample_rate,
                        &resampler,
                        callback_buffer.clone(),
                        total_counter.clone(),
                    );
                },
                err_fn,
                None,
            )?
        }

        SampleFormat::I16 => {
            let callback_buffer = Arc::clone(&audio_buffer);
            let total_counter = Arc::clone(&total_samples);
            let resampler = Arc::new(Mutex::new(LinearResampler::new(
                input_sample_rate,
                TARGET_SAMPLE_RATE,
            )));

            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&sample| sample as f32 / i16::MAX as f32)
                        .collect();

                    push_audio(
                        &samples,
                        channels,
                        input_sample_rate,
                        &resampler,
                        callback_buffer.clone(),
                        total_counter.clone(),
                    );
                },
                err_fn,
                None,
            )?
        }

        SampleFormat::U16 => {
            let callback_buffer = Arc::clone(&audio_buffer);
            let total_counter = Arc::clone(&total_samples);
            let resampler = Arc::new(Mutex::new(LinearResampler::new(
                input_sample_rate,
                TARGET_SAMPLE_RATE,
            )));

            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&sample| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect();

                    push_audio(
                        &samples,
                        channels,
                        input_sample_rate,
                        &resampler,
                        callback_buffer.clone(),
                        total_counter.clone(),
                    );
                },
                err_fn,
                None,
            )?
        }

        format => {
            anyhow::bail!("Unsupported microphone sample format: {:?}", format);
        }
    };

    stream.play().context("Failed to start microphone stream")?;

    println!("[Wake] 🎙 Listening for the configured wake word...");

    let mut last_detection = Instant::now()
        .checked_sub(cooldown)
        .unwrap_or_else(Instant::now);

    /*
     * Opening the capture stream usually delivers a loud startup click/pop.
     * It is not a wake event, but it poisons the clap scanner's background
     * estimate and can spuriously fire. Ignore the first second of audio.
     */
    let started = Instant::now();

    // How many samples must accumulate before the classifier runs again.
    let step_samples: usize =
        ((inference_step_ms as usize * TARGET_SAMPLE_RATE as usize) / 1000).max(POLL_SAMPLES);

    let mut samples_since_inference = 0usize;
    let mut noise_floor = min_rms;
    let mut clap_scanner = ClapScanner::new(min_rms);

    /*
     * One persistent RNNoise state for the whole session. Recreating it on
     * every inference (the old behavior, plus an 8-frame silence warmup)
     * made the adaptive filter start from scratch each time, injecting
     * level jumps into the signal the classifier sees.
     */
    let mut denoiser_state: Option<Box<nnnoiseless::DenoiseState<'static>>> = None;

    let mut last_level_log = Instant::now();

    let mut grace_steps = 0usize;

    let mut warned_clipping = false;

    let mut output_was_active = false;
    // Require the classifier to return to a quiet/low-confidence state before
    // accepting another wake. This prevents the same trailing audio window
    // from retriggering after the cooldown expires.
    let mut wake_armed = true;
    let mut evidence = WakeEvidence::new(min_hits, hit_window);

    let debug = wake_debug_enabled();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        thread::sleep(Duration::from_millis(50));

        if started.elapsed() < Duration::from_millis(1_000) {
            continue;
        }

        /*
         * While the agent is producing output (thinking, tools, or speech) the
         * user must still be able to barge in by saying the wake word. There is
         * no echo cancellation, so the detector hears the agent's own
         * text-to-speech. Two things stop that from re-triggering it:
         *   - the clap scanner is disabled during output, because speech has
         *     sharp onsets that look exactly like a clap; and
         *   - the wake-word threshold is raised, so only a clear, close
         *     "alexa" (a deliberate barge-in) crosses it.
         */
        let output_suppressed = output_active.load(Ordering::Relaxed);

        if output_suppressed {
            // Keep the clap scanner's background estimate fresh for when output
            // ends, but do not let it fire on the agent's own speech.
            clap_scanner = ClapScanner::new(min_rms);
        } else if output_was_active {
            /*
             * Output just finished. Drop the buffered agent audio so its tail
             * cannot trigger a false wake now that the stricter output
             * threshold no longer applies.
             */
            let mut buffer = audio_buffer.lock().expect("Wake audio buffer poisoned");

            buffer.clear();

            drop(buffer);

            clap_scanner = ClapScanner::new(min_rms);
            evidence.reset();
            last_detection = Instant::now();
        }

        output_was_active = output_suppressed;

        /*
         * 1) Hand-clap wake. This runs before (and independently of) the
         *    classifier buffer, so a clap wakes the agent immediately. The
         *    scanner is coalesced by the shared cooldown below.
         */
        if clap_enabled && !output_suppressed {
            let fired = {
                let buffer = audio_buffer.lock().expect("Wake audio buffer poisoned");

                let snapshot = high_pass(&buffer, TARGET_SAMPLE_RATE as f32);

                let total = total_samples.load(Ordering::Relaxed);

                clap_scanner.detect(&snapshot, total, clap_sensitivity, clap_min_peak)
            };

            if fired {
                if last_detection.elapsed() < cooldown {
                    continue;
                }

                last_detection = Instant::now();
                wake_armed = false;
                evidence.reset();

                /*
                 * Release the microphone while the runtime records the
                 * command. A second capture stream opened while this one is
                 * still running receives a heavily attenuated signal on
                 * PipeWire (about 10x quieter), which made speech-to-text
                 * discard every command as background noise.
                 */
                let _ = stream.pause();

                let woke = fire_wake(&event_tx, &resume, &audio_buffer, "clap", 1.0);

                let _ = stream.play();

                if woke {
                    break;
                }

                // The clap cleared the audio buffer; restart the scanner so
                // the background estimate and cursor are fresh for the next
                // clap.
                clap_scanner = ClapScanner::new(min_rms);

                continue;
            }
        }

        let samples = {
            let buffer = audio_buffer.lock().expect("Wake audio buffer poisoned");

            if buffer.len() < INFERENCE_BUFFER_SAMPLES {
                continue;
            }

            if samples_since_inference < step_samples {
                samples_since_inference += POLL_SAMPLES;
                continue;
            }

            samples_since_inference = 0;

            buffer.clone()
        };

        /*
         * Warn once if the microphone is clipping. A hot analog capture
         * (capture gain + mic boost) flattens the waveform and hides speech,
         * which no software filter can fully recover -- the fix is to lower
         * the hardware gain.
         */
        if !warned_clipping {
            let clipped = samples.iter().filter(|sample| sample.abs() > 0.985).count();

            if clipped * 50 > samples.len() {
                warned_clipping = true;

                eprintln!(
                    "[Wake] ⚠ Microphone is clipping: lower the input volume / \
                     mic boost (e.g. `alsamixer -c 1` -> Capture, Mic Boost)."
                );
            }
        }

        let processed = if denoise {
            let filtered = high_pass(&samples, TARGET_SAMPLE_RATE as f32);

            denoise_for_wake(&mut denoiser_state, &filtered)
        } else {
            high_pass(&samples, TARGET_SAMPLE_RATE as f32)
        };

        let level = rms(&processed);

        /*
         * Track a slowly-moving noise floor. Quiet speech still passes once
         * it rises above the floor, while steady background noise is ignored.
         */
        if level < noise_floor * 1.6 {
            noise_floor = noise_floor * 0.95 + level * 0.05;
        }

        /*
         * The gate saves CPU during true silence, but it must NOT stop
         * scoring right after a word. The classifier scores the trailing
         * ~2 seconds of the buffer and peaks when the wake word ENDS near
         * the buffer end -- exactly the window that appears just after the
         * loud part stops. Stopping at the gate-close transition (the old
         * behavior) therefore skipped the single best-aligned window, which
         * is why real wake words almost never scored. Instead, keep scoring
         * through a grace period after the audio goes quiet.
         */
        const GRACE_INFERENCE_STEPS: usize = 8;

        let gate = (noise_floor * 1.5).max(min_rms * 0.7);

        if level < gate {
            if grace_steps > 0 {
                grace_steps -= 1;
            } else {
                /*
                 * Diagnostics: gated (near-silence) windows show their level
                 * once per second, so a dead/quiet/always-loud microphone is
                 * visible without waiting for anything to pass the gate.
                 */
                if debug && last_level_log.elapsed() >= Duration::from_secs(1) {
                    last_level_log = Instant::now();

                    println!(
                        "[Wake][debug] (gated) level {level:.5} gate {gate:.5} floor {noise_floor:.5}"
                    );
                }

                continue;
            }
        } else {
            grace_steps = GRACE_INFERENCE_STEPS;
        }

        /*
         * A large maximum gain (the old 8x) amplified denoiser residue and
         * changed the loudness profile the classifier was trained on. A
         * gentle 2x cap keeps normalization useful without distorting the
         * signal.
         */
        let normalized = normalize_peak(&processed, 0.7, 2.0);

        /*
         * Score only the classifier's trailing window. The classifier reads
         * the LAST 16 embeddings of whatever it is given, so feeding the full
         * buffer only wastes mel/embedding compute on audio the classifier
         * would ignore anyway.
         */
        let slice_start = normalized.len().saturating_sub(INFERENCE_BUFFER_SAMPLES);
        let pcm = f32_to_i16(&normalized[slice_start..]);

        let predictions = match model.predict(&pcm) {
            Ok(predictions) => predictions,

            Err(error) => {
                eprintln!("[Wake] Model inference error: {}", error);

                continue;
            }
        };

        let effective_threshold = if output_suppressed {
            (threshold + BARGE_IN_THRESHOLD_BOOST).min(0.99)
        } else {
            threshold
        };
        let effective_strong_threshold = if output_suppressed {
            (strong_threshold + BARGE_IN_THRESHOLD_BOOST).min(0.99)
        } else {
            strong_threshold.max(effective_threshold)
        };

        if debug {
            let scores = predictions
                .iter()
                .map(|(label, score)| format!("{label}={score:.3}"))
                .collect::<Vec<_>>()
                .join(" ");

            println!(
                "[Wake][debug] level {level:.4} gate {gate:.4} threshold {effective_threshold:.2} strong {effective_strong_threshold:.2} | {scores}"
            );
        }

        let (label, confidence) = predictions
            .iter()
            .map(|(label, score)| (label.as_str(), *score))
            .max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(("wake word", 0.0));

        if !wake_armed {
            evidence.reset();
            let quiet_reset = confidence < effective_threshold * 0.5 && level < gate * 1.5;
            if quiet_reset {
                wake_armed = true;
            }
            continue;
        }

        if !evidence.observe(
            label,
            confidence,
            effective_threshold,
            effective_strong_threshold,
            Instant::now(),
        ) {
            continue;
        }

        if last_detection.elapsed() < cooldown {
            continue;
        }

        last_detection = Instant::now();
        wake_armed = false;

        // Release the microphone for the recording (see the clap path).
        let _ = stream.pause();

        let woke = fire_wake(&event_tx, &resume, &audio_buffer, label, confidence);

        let _ = stream.play();

        if woke {
            break;
        }
    }

    drop(stream);

    Ok(())
}

/// Emit a wake event, clear the audio history, then block until the speech
/// owner is done recording the command.
///
/// The caller pauses the microphone stream first, so no audio accumulates
/// while the command is recorded. The buffer is still cleared again after the
/// resume signal in case any callback was in flight when the stream paused.
///
/// Returns `true` when the event channel failed and the detector should stop.
fn fire_wake(
    event_tx: &Arc<Mutex<Sender<HyuskEvent>>>,
    resume: &WakeResume,
    audio_buffer: &Arc<Mutex<Vec<f32>>>,
    label: &str,
    confidence: f32,
) -> bool {
    println!("[Wake] ✓ Wake detected: {label} ({confidence:.3})");

    let sender = { event_tx.lock().expect("Wake event sender poisoned").clone() };

    if let Err(error) = sender.blocking_send(HyuskEvent::WakeWordDetected) {
        eprintln!("[Wake] Failed to send wake event: {}", error);

        return true;
    }

    {
        let mut buffer = audio_buffer.lock().expect("Wake audio buffer poisoned");

        buffer.clear();
    }

    if !resume.wait_for(MAX_PAUSE) {
        eprintln!("[Wake] Resume signal timed out; continuing without the runtime");
    }

    {
        let mut buffer = audio_buffer.lock().expect("Wake audio buffer poisoned");

        buffer.clear();
    }

    false
}

/// Detects hand claps in the audio buffer and keeps a slow estimate of the
/// background energy level.
struct ClapScanner {
    /// Absolute stream index of the last sample that has been fully scanned
    /// (including the lookahead a clap decision needs).
    scanned_total: u64,

    /// Slow-moving estimate of the typical energy (mean squared sample) of
    /// the background.
    background_energy: f32,
}

impl ClapScanner {
    fn new(min_rms: f32) -> Self {
        Self {
            scanned_total: 0,
            background_energy: (min_rms * min_rms).max(1e-6),
        }
    }

    /// Search newly arrived audio for a hand-clap transient.
    ///
    /// A clap is a sharp, short acoustic burst: the level in a 10 ms window
    /// jumps well above the recent background, the window before it was
    /// quiet, and the peak reaches an absolute floor. Returns `true` once per
    /// qualifying transient.
    ///
    /// The audio buffer is a sliding window that drains from the front once it
    /// fills, so an absolute cursor (rather than a buffer-relative one) is the
    /// only way to know which samples are new. The old buffer-relative cursor
    /// stopped matching as soon as the buffer reached its cap, so claps worked
    /// for the first seconds after startup and then silently stopped.
    fn detect(
        &mut self,
        buffer: &[f32],
        total_samples: u64,
        sensitivity: f32,
        min_peak: f32,
    ) -> bool {
        if buffer.len() < CLAP_WINDOW_SAMPLES * 2 {
            self.scanned_total = self
                .scanned_total
                .max(total_samples.saturating_sub(buffer.len() as u64));
            return false;
        }

        let lookahead = (CLAP_WINDOW_SAMPLES * 2) as u64;
        let buffer_start = total_samples.saturating_sub(buffer.len() as u64);

        let mut absolute = self.scanned_total.max(buffer_start);

        // Nothing to scan until a full decision window has arrived.
        if absolute + lookahead > total_samples {
            return false;
        }

        let mut found = false;

        while absolute + lookahead <= total_samples {
            let index = (absolute - buffer_start) as usize;

            let (energy, peak) = window_energy_peak(buffer, index, CLAP_WINDOW_SAMPLES);

            let level = energy.sqrt();
            let background = self.background_energy.sqrt().max(1e-4);

            let is_loud = level > background * sensitivity && peak >= min_peak;

            if is_loud {
                // Require a sharp onset and a fast decay. Sustained loud noise
                // (music, typing, fan rushes) fails one of these checks. The
                // onset ratio is generous (0.45): a real hand clap spreads its
                // attack over more than one 10 ms window, so a strict ratio
                // (the old 0.2) rejected almost every real clap while the
                // synthetic unit test passed.
                let (_, previous_peak) = if index >= CLAP_WINDOW_SAMPLES {
                    window_energy_peak(buffer, index - CLAP_WINDOW_SAMPLES, CLAP_WINDOW_SAMPLES)
                } else {
                    (0.0, 0.0)
                };
                let (_, next_peak) =
                    window_energy_peak(buffer, index + CLAP_WINDOW_SAMPLES, CLAP_WINDOW_SAMPLES);

                let sharp_onset = index >= CLAP_WINDOW_SAMPLES && previous_peak <= peak * 0.45;
                let fast_decay = next_peak <= peak * 0.8;

                if sharp_onset && fast_decay {
                    found = true;
                    break;
                }

                // Loud but not clap-shaped windows still describe the
                // background (music, TV). Adapt slowly so steady noise does
                // not permanently deafen the clap detector, but do adapt, or
                // the sensitivity ratio becomes unreachable during playback.
                self.background_energy = self.background_energy * 0.995 + energy * 0.005;
            } else {
                // Adapt the floor from quiet windows so a clap does not make
                // the next clap harder to detect.
                self.background_energy = self.background_energy * 0.98 + energy * 0.02;
            }

            absolute += CLAP_WINDOW_SAMPLES as u64;
        }

        if found {
            // Consume the whole decision window so the same clap cannot fire
            // again on the next poll.
            self.scanned_total = absolute + lookahead;
        } else {
            self.scanned_total = total_samples - lookahead;
        }

        found
    }
}

/// Average squared amplitude and peak of `window` samples starting at
/// `start`, in `[-1.0, 1.0]` sample units.
fn window_energy_peak(buffer: &[f32], start: usize, window: usize) -> (f32, f32) {
    let mut energy = 0.0f32;
    let mut peak = 0.0f32;

    for &sample in &buffer[start..start + window] {
        energy += sample * sample;
        peak = peak.max(sample.abs());
    }

    (energy / window as f32, peak)
}

/// Convert interleaved microphone audio to mono and resample to 16 kHz.
///
/// Example:
///
///     L R L R L R
///
/// becomes:
///
///     (L+R)/2 ...
fn push_audio(
    data: &[f32],
    channels: usize,
    input_sample_rate: u32,
    resampler: &Arc<Mutex<LinearResampler>>,
    buffer: Arc<Mutex<Vec<f32>>>,
    total_samples: Arc<std::sync::atomic::AtomicU64>,
) {
    if channels == 0 {
        return;
    }

    let mut mono = Vec::with_capacity(data.len() / channels);

    for frame in data.chunks(channels) {
        if frame.is_empty() {
            continue;
        }

        let sum: f32 = frame.iter().copied().sum();

        mono.push(sum / frame.len() as f32);
    }

    let resampled = if input_sample_rate == TARGET_SAMPLE_RATE {
        mono
    } else {
        resampler
            .lock()
            .expect("Wake resampler poisoned")
            .resample(&mono)
    };

    // Diagnostics: HYUSK_WAKE_CAPTURE=<path> records the exact 16 kHz mono
    // stream the classifier consumes, for offline analysis.
    if let Ok(path) = std::env::var("HYUSK_WAKE_CAPTURE") {
        use std::io::Write;

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let mut bytes = Vec::with_capacity(resampled.len() * 2);

            for &sample in &resampled {
                let quantized = (sample.clamp(-1.0, 1.0) * 32_767.0) as i16;

                bytes.extend_from_slice(&quantized.to_le_bytes());
            }

            let _ = file.write_all(&bytes);
        }
    }

    let mut buffer = buffer.lock().expect("Wake audio buffer poisoned");

    buffer.extend_from_slice(&resampled);

    total_samples.fetch_add(resampled.len() as u64, std::sync::atomic::Ordering::Relaxed);

    /*
     * Keep approximately the latest 2.5 seconds.
     *
     * This prevents the buffer from growing forever.
     */
    let max_samples = TARGET_SAMPLE_RATE as usize * 5 / 2;

    if buffer.len() > max_samples {
        let excess = buffer.len() - max_samples;

        buffer.drain(0..excess);
    }
}

/// Small phase-continuous linear resampler.
///
/// The microphone callback delivers fixed-size chunks. Resampling each chunk
/// independently (the old behavior) restarted the interpolation phase at
/// every chunk boundary, adding timing jitter that grows with the chunk rate
/// and degrades the mel spectrogram the classifier consumes. This version
/// tracks the absolute sample clock, so output positions stay exact across
/// chunk boundaries.
struct LinearResampler {
    ratio: f64,

    /// Absolute stream position of the next output sample.
    next_output: f64,

    /// Absolute stream index of the first sample of the next chunk.
    consumed: u64,

    /// The last sample of the previous chunk, used to interpolate for an
    /// output position that falls between chunks.
    tail: Option<f32>,
}

impl LinearResampler {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            ratio: input_rate as f64 / output_rate as f64,
            next_output: 0.0,
            consumed: 0,
            tail: None,
        }
    }

    fn resample(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }

        if self.ratio == 1.0 {
            self.tail = Some(*input.last().expect("non-empty checked above"));
            self.consumed += input.len() as u64;

            return input.to_vec();
        }

        let base = self.consumed as f64;
        let len = input.len() as f64;

        let mut output = Vec::new();

        // Interpolation needs both neighbors inside the current chunk (or the
        // previous chunk's tail), so stop one sample before the end. A pending
        // position in the final unit interval is picked up by the next chunk,
        // whose tail provides the missing left neighbor.
        while self.next_output < base + len - 1.0 {
            let index = self.next_output.floor() as i64;
            let fraction = (self.next_output - index as f64) as f32;

            let a = if index < base as i64 {
                self.tail
                    .expect("tail is set whenever a position precedes the chunk")
            } else {
                input[((index as f64 - base) as usize).min(input.len() - 1)]
            };

            let b = input[(((index + 1) as f64 - base) as usize).min(input.len() - 1)];

            output.push(a + (b - a) * fraction);

            self.next_output += self.ratio;
        }

        self.tail = Some(*input.last().expect("non-empty checked above"));
        self.consumed += input.len() as u64;

        output
    }
}

/// Remove DC offset and sub-vocal low-frequency rumble.
///
/// Laptop analog captures frequently carry most of their energy below ~100 Hz
/// (DC offset, mains hum, handling rumble), especially with high hardware
/// capture gain and mic boost. The wake-word classifier is trained on
/// speech-band audio, so that energy only masks the word (and trips the clap
/// scanner). A one-pole DC blocker plus a ~90 Hz high-pass removes it before
/// denoising and classification.
pub(crate) fn high_pass(samples: &[f32], sample_rate: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let dt = 1.0 / sample_rate;
    let rc = 1.0 / (2.0 * std::f32::consts::PI * 90.0);
    let alpha = rc / (rc + dt);

    let mut previous_input = samples[0];
    let mut previous_output = 0.0f32;

    let mut output = Vec::with_capacity(samples.len());

    for &sample in samples {
        let filtered = alpha * (previous_output + sample - previous_input);

        previous_input = sample;
        previous_output = filtered;

        output.push(filtered);
    }

    output
}

fn f32_to_i16(audio: &[f32]) -> Vec<i16> {
    audio
        .iter()
        .map(|&sample| {
            let sample = sample.clamp(-1.0, 1.0);

            (sample * i16::MAX as f32) as i16
        })
        .collect()
}

/// Denoise 16 kHz mono audio with RNNoise.
///
/// RNNoise expects 48 kHz, i16-range `f32` frames of 480 samples. The buffer is
/// upsampled, denoised, and downsampled back to 16 kHz. A short silence warmup
/// is fed first so the adaptive filter does not consume the real audio.
fn denoise_for_wake(
    state: &mut Option<Box<nnnoiseless::DenoiseState<'static>>>,
    samples: &[f32],
) -> Vec<f32> {
    use nnnoiseless::DenoiseState;

    if samples.is_empty() {
        return Vec::new();
    }

    const UPSAMPLE: usize = 3;
    const WARMUP_FRAMES: usize = 8;

    let mut upsampled = Vec::with_capacity(samples.len() * UPSAMPLE);

    for index in 0..samples.len() {
        let current = samples[index];
        let next = samples.get(index + 1).copied().unwrap_or(current);
        let delta = next - current;

        upsampled.push(current);
        upsampled.push(current + delta / 3.0);
        upsampled.push(current + delta * 2.0 / 3.0);
    }

    for sample in &mut upsampled {
        *sample *= 32768.0;
    }

    let frame_size = DenoiseState::FRAME_SIZE;
    let silence = vec![0.0_f32; frame_size];

    let first_use = state.is_none();

    let mut denoiser = match state.take() {
        Some(denoiser) => denoiser,

        None => DenoiseState::new(),
    };

    let mut denoised = Vec::with_capacity(upsampled.len());
    let mut output = vec![0.0_f32; frame_size];

    // Feed silence once so the very first real frame is not swallowed while
    // the adaptive filter initializes. Later calls keep the filter state.
    if first_use {
        for _ in 0..WARMUP_FRAMES {
            denoiser.process_frame(&mut output, &silence);
        }
    }

    for chunk in upsampled.chunks_exact(frame_size) {
        denoiser.process_frame(&mut output, chunk);
        denoised.extend_from_slice(&output);
    }

    // Downsample 48 kHz back to 16 kHz.
    let mut result = Vec::with_capacity(samples.len());
    let mut index = 0usize;

    while index + 2 < denoised.len() {
        let value = (denoised[index] + denoised[index + 1] + denoised[index + 2]) / 3.0 / 32768.0;

        result.push(value.clamp(-1.0, 1.0));
        index += 3;
    }

    // Preserve the original length for the wake model's 2-second requirement.
    while result.len() < samples.len() {
        result.push(0.0);
    }

    result.truncate(samples.len());

    *state = Some(denoiser);

    result
}

fn normalize_peak(audio: &[f32], target_peak: f32, max_gain: f32) -> Vec<f32> {
    let peak = audio
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);

    if peak <= f32::EPSILON {
        return audio.to_vec();
    }

    let gain = (target_peak / peak).min(max_gain);

    audio
        .iter()
        .map(|sample| (sample * gain).clamp(-1.0, 1.0))
        .collect()
}

fn rms(audio: &[f32]) -> f32 {
    if audio.is_empty() {
        return 0.0;
    }

    let sum = audio.iter().map(|sample| sample * sample).sum::<f32>();

    (sum / audio.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::{ClapScanner, LinearResampler, WakeEvidence, WakeResume};

    #[test]
    fn wake_evidence_rejects_isolated_spikes_and_accepts_two_hits() {
        let start = std::time::Instant::now();
        let mut evidence = WakeEvidence::new(2, std::time::Duration::from_secs(1));

        assert!(!evidence.observe("hey_hyusk", 0.95, 0.6, 0.99, start));
        assert!(!evidence.observe(
            "hey_hyusk",
            0.20,
            0.6,
            0.99,
            start + std::time::Duration::from_millis(200)
        ));
        assert!(evidence.observe(
            "hey_hyusk",
            0.80,
            0.6,
            0.99,
            start + std::time::Duration::from_millis(600)
        ));

        assert!(!evidence.observe("hey_hyusk", 0.95, 0.6, 0.99, start));
        assert!(!evidence.observe(
            "hey_hyusk",
            0.95,
            0.6,
            0.99,
            start + std::time::Duration::from_millis(1_100)
        ));

        assert!(evidence.observe("hey_hyusk", 0.762, 0.6, 0.75, start));
    }

    /// Manual probe: scores a raw 16 kHz i16 clip (e.g. a TTS-generated
    /// "alexa") through the classifier. Run with:
    ///   cargo test wake_score_probe -- --nocapture
    /// Input: /tmp/hyusk_wake_probe.raw
    #[test]
    fn wake_score_probe() {
        /*
         * Offline diagnostic only. It scores arbitrary multi-second clips and
         * can take minutes on the ONNX models, so it is opt-in instead of
         * running as part of the normal suite.
         */
        if std::env::var_os("HYUSK_WAKE_PROBE").is_none() {
            eprintln!("note: set HYUSK_WAKE_PROBE=1 to run the wake score probe");
            return;
        }

        let path = std::path::Path::new("/tmp/hyusk_wake_probe.raw");

        let model_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/alexa.onnx");

        let mut model = livekit_wakeword::WakeWordModel::new(&[model_path], 16_000).unwrap();

        if path.exists() {
            let bytes = std::fs::read(path).unwrap();
            let pcm: Vec<i16> = bytes
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();

            // Sliding 2-second windows to find the best score in the clip.
            let window = 32_000;
            let step = 4_000;

            let mut start = 0usize;

            while start + window <= pcm.len() {
                let predictions = model.predict(&pcm[start..start + window]).unwrap();
                println!(
                    "probe window @{:.2}s..{:.2}s -> {:?}",
                    start as f32 / 16_000.0,
                    (start + window) as f32 / 16_000.0,
                    predictions
                );

                start += step;
            }
        } else {
            eprintln!("note: no /tmp/hyusk_wake_probe.raw (TTS probe skipped)");
        }

        // Score a captured live-mic stream the same way the runtime does.
        let capture = std::path::Path::new("/tmp/hyusk_wake_capture.raw");

        if capture.exists() {
            let bytes = std::fs::read(capture).unwrap();
            let live: Vec<f32> = bytes
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();

            println!("--- live capture: {:.1}s ---", live.len() as f32 / 16_000.0);

            let mut state = None;
            let filtered = super::high_pass(&live, 16_000.0);
            let denoised = super::denoise_for_wake(&mut state, &filtered);
            let normalized = super::normalize_peak(&denoised, 0.7, 2.0);

            let window = super::INFERENCE_BUFFER_SAMPLES;
            let step = 4_000;

            let mut start = 0usize;
            let mut best = 0.0f32;

            while start + window <= normalized.len() {
                let slice = &normalized[start..start + window];
                let level = super::rms(slice);

                // Skip near-silence: only speech-band energy is worth scoring,
                // and silence dominates the timeline (and the runtime time).
                if level < 0.004 {
                    start += step;
                    continue;
                }

                let pcm: Vec<i16> = slice
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect();

                let predictions = model.predict(&pcm).unwrap();
                let score = predictions.values().cloned().fold(0.0f32, f32::max);
                best = best.max(score);

                println!(
                    "live window @{:.2}s level {:.4} -> {:?}",
                    start as f32 / 16_000.0,
                    level,
                    predictions
                );

                start += step;
            }

            println!("-- best live voice score: {best:.3} --");
        } else {
            println!("(no /tmp/hyusk_wake_capture.raw; skipping live capture analysis)");
        }

        let window = 32_000;
        let step = 4_000;

        // The runtime's preprocessing pipeline: denoise + peak normalize.
        if path.exists() {
            let bytes = std::fs::read(path).unwrap();
            let pcm: Vec<i16> = bytes
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();

            let mut state = None;

            let raw: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
            let filtered = super::high_pass(&raw, 16_000.0);
            let denoised = super::denoise_for_wake(&mut state, &filtered);

            let normalized = super::normalize_peak(&denoised, 0.7, 2.0);

            let processed_pcm: Vec<i16> = normalized
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .collect();

            println!("--- denoised + normalized (probe file) ---");

            let mut start = 0usize;

            while start + window <= processed_pcm.len() {
                let predictions = model
                    .predict(&processed_pcm[start..start + window])
                    .unwrap();
                println!(
                    "processed window @{:.2}s..{:.2}s -> {:?}",
                    start as f32 / 16_000.0,
                    (start + window) as f32 / 16_000.0,
                    predictions
                );

                start += step;
            }
        }
    }

    #[test]
    fn chunked_resampling_matches_whole_buffer() {
        let input_rate = 44_100u32;
        let output_rate = 16_000u32;

        // A deterministic pseudo-signal with content at many frequencies.
        let input: Vec<f32> = (0..44_100)
            .map(|index| {
                ((index as f32 * 0.013).sin() * 0.4 + ((index * 7919) % 1000) as f32 / 1000.0 * 0.1)
                    .clamp(-1.0, 1.0)
            })
            .collect();

        let mut chunked = LinearResampler::new(input_rate, output_rate);
        let mut chunked_output = Vec::new();

        for chunk in input.chunks(441) {
            chunked_output.extend(chunked.resample(chunk));
        }

        let mut whole = LinearResampler::new(input_rate, output_rate);
        let whole_output = whole.resample(&input);

        assert_eq!(chunked_output.len(), whole_output.len());

        for (chunked, whole) in chunked_output.iter().zip(whole_output.iter()) {
            assert!(
                (chunked - whole).abs() < 1e-4,
                "chunked {chunked} != whole {whole}"
            );
        }
    }

    #[test]
    fn integer_ratio_resampling_is_exact() {
        let mut resampler = LinearResampler::new(48_000, 16_000);
        let mut output = Vec::new();

        for chunk_index in 0..10 {
            let chunk: Vec<f32> = (0..480)
                .map(|index| (chunk_index * 480 + index) as f32)
                .collect();

            output.extend(resampler.resample(&chunk));
        }

        // Exactly every third sample across the whole stream.
        for (index, sample) in output.iter().enumerate() {
            assert_eq!(*sample, (index * 3) as f32);
        }
    }

    #[test]
    fn resume_before_wait_is_not_lost() {
        let resume = WakeResume::new();

        resume.resume();
        resume.wait();
    }

    #[test]
    fn cloned_resume_releases_original() {
        let resume = WakeResume::new();
        let clone = resume.clone();

        clone.resume();
        resume.wait();
    }

    #[test]
    fn clap_scanner_fires_on_impulse() {
        let mut scanner = ClapScanner::new(0.003);
        let mut buffer = Vec::<f32>::with_capacity(32_000);

        // One second of quiet background.
        buffer.extend(std::iter::repeat_n(0.001, 16_000));

        assert!(
            !scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "quiet audio should not clap"
        );

        // A sharp attack followed by a fast decay, as from a hand clap.
        buffer.extend(std::iter::repeat_n(0.6, super::CLAP_WINDOW_SAMPLES));
        buffer.extend(std::iter::repeat_n(0.05, super::CLAP_WINDOW_SAMPLES));

        assert!(
            scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "a loud transient after quiet should fire"
        );

        // The same audio must only be scanned once.
        assert!(
            !scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "the same transient should not fire twice"
        );
    }

    #[test]
    fn clap_scanner_fires_after_buffer_wrap() {
        /*
         * Regression: with a buffer-relative cursor, the scanner stopped
         * advancing as soon as the sliding buffer reached its cap, so claps
         * were only detected for the first seconds after startup.
         */
        let cap = 40_000usize;
        let mut scanner = ClapScanner::new(0.003);
        let mut buffer = Vec::<f32>::new();
        let mut total: u64 = 0;

        // Fill well past the buffer cap with quiet background.
        for _ in 0..60 {
            buffer.extend(std::iter::repeat_n(0.001, 1_000));
            total += 1_000;

            if buffer.len() > cap {
                let excess = buffer.len() - cap;
                buffer.drain(0..excess);
            }

            assert!(!scanner.detect(&buffer, total, 6.0, 0.03));
        }

        // A clap arriving long after the buffer started draining.
        buffer.extend(std::iter::repeat_n(0.6, super::CLAP_WINDOW_SAMPLES));
        buffer.extend(std::iter::repeat_n(0.05, super::CLAP_WINDOW_SAMPLES));
        total += (super::CLAP_WINDOW_SAMPLES * 2) as u64;

        if buffer.len() > cap {
            let excess = buffer.len() - cap;
            buffer.drain(0..excess);
        }

        assert!(
            scanner.detect(&buffer, total, 6.0, 0.03),
            "a clap after the buffer has wrapped must still be detected"
        );
    }

    #[test]
    fn clap_scanner_fires_again_after_quiet() {
        let mut scanner = ClapScanner::new(0.003);
        let mut buffer = Vec::<f32>::with_capacity(64_000);

        buffer.extend(std::iter::repeat_n(0.001, 16_000));
        buffer.extend(std::iter::repeat_n(0.6, super::CLAP_WINDOW_SAMPLES));
        buffer.extend(std::iter::repeat_n(0.05, super::CLAP_WINDOW_SAMPLES));

        assert!(scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03));

        buffer.extend(std::iter::repeat_n(0.001, 16_000));
        buffer.extend(std::iter::repeat_n(0.6, super::CLAP_WINDOW_SAMPLES));
        buffer.extend(std::iter::repeat_n(0.05, super::CLAP_WINDOW_SAMPLES));

        assert!(
            scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "a second clap after a quiet gap should fire"
        );
    }

    #[test]
    fn clap_scanner_ignores_sustained_noise() {
        let mut scanner = ClapScanner::new(0.003);
        let mut buffer = Vec::<f32>::with_capacity(32_000);

        // Ramp linearly from silence to a loud tone over two seconds. There is
        // never a sharp onset, so this must not count as a clap.
        let total = 32_000;

        for index in 0..total {
            let progress = index as f32 / total as f32;
            let sample = ((index * 997) % 1001) as f32 / 500.0 - 1.0;

            buffer.push(sample * progress * 0.6);
        }

        assert!(
            !scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "a slowly growing signal has no sharp onset and must not fire"
        );
    }

    #[test]
    fn clap_scanner_fires_on_quiet_clap() {
        let mut scanner = ClapScanner::new(0.003);
        let mut buffer = Vec::<f32>::with_capacity(32_000);

        // Room-tone background, then a clap that is loud relative to it but
        // not near full scale. This catches the old bug where min_peak was
        // multiplied by sensitivity.
        buffer.extend(std::iter::repeat_n(0.001, 16_000));
        buffer.extend(std::iter::repeat_n(0.12, super::CLAP_WINDOW_SAMPLES));
        buffer.extend(std::iter::repeat_n(0.01, super::CLAP_WINDOW_SAMPLES));

        assert!(
            scanner.detect(&buffer, buffer.len() as u64, 6.0, 0.03),
            "a moderate clap above the noise floor should fire"
        );
    }

    #[test]
    fn denoise_preserves_length_and_is_finite() {
        let samples: Vec<f32> = (0..32_000)
            .map(|index| {
                let tone = (index as f32 * 0.05).sin() * 0.08;
                let noise = ((index * 7919) % 101) as f32 / 101.0 - 0.5;
                tone + noise * 0.02
            })
            .collect();

        let mut state = None;

        let output = super::denoise_for_wake(&mut state, &samples);

        assert_eq!(output.len(), samples.len());
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn hey_livekit_classifier_loads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join("hey_livekit.onnx");

        if !path.exists() {
            eprintln!(
                "skipping hey_livekit_classifier_loads: {} is missing",
                path.display()
            );
            return;
        }

        let mut model = livekit_wakeword::WakeWordModel::new(&[path], 16_000)
            .expect("load hey_livekit classifier");

        let audio = vec![0_i16; 32_000];
        let predictions = model.predict(&audio).expect("run classifier");

        assert!(
            predictions.contains_key("hey_livekit"),
            "prediction keys: {:?}",
            predictions.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn alexa_classifier_loads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join("alexa.onnx");

        if !path.exists() {
            eprintln!(
                "skipping alexa_classifier_loads: {} is missing",
                path.display()
            );
            return;
        }

        let mut model =
            livekit_wakeword::WakeWordModel::new(&[path], 16_000).expect("load alexa classifier");

        let audio = vec![0_i16; 32_000];
        let predictions = model.predict(&audio).expect("run alexa classifier");

        assert!(
            predictions.contains_key("alexa"),
            "prediction keys: {:?}",
            predictions.keys().collect::<Vec<_>>()
        );
    }
}
