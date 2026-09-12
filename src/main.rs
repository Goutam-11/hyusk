mod agent;
mod model;
mod speech;
mod system_info;
mod tools;
mod types;
mod ui;
mod wake;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use dotenvy::dotenv;
use tokio::sync::mpsc;

use agent::Agent;
use model::openrouter::OpenRouterClient;

use tools::{
    computer::ComputerTool, media::MediaTool, memory::MemoryTool, process::ProcessTool,
    shell::ShellTool, ToolRegistry,
};

#[cfg(target_os = "linux")]
use tools::accessibility::AccessibilityTool;

use speech::{test_audio, SpeechToText};

use types::{HyuskEvent, HyuskState};

use crate::wake::detector::{WakeResume, WakeWordDetector};

fn model_exists(relative: &str) -> bool {
    if std::path::Path::new(relative).exists() {
        return true;
    }

    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(relative)
        .exists()
}

fn default_model_path(relative: &str) -> String {
    if std::path::Path::new(relative).exists() {
        return relative.to_string();
    }

    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);

    if manifest_path.exists() {
        return manifest_path.to_string_lossy().to_string();
    }

    relative.to_string()
}

fn default_wake_model_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    for candidate in [
        "models/hey_hyusk.onnx",
        "models/hey_livekit.onnx",
        "models/alexa.onnx",
        "models/nihao_livekit.onnx",
        "models/hey_jarvis.onnx",
        "models/hey_mycroft.onnx",
        "models/hey_rhasspy.onnx",
    ] {
        if model_exists(candidate) {
            paths.push(std::path::PathBuf::from(default_model_path(candidate)));
        }
    }

    if paths.is_empty() {
        paths.push(std::path::PathBuf::from(default_model_path(
            "models/hey_hyusk.onnx",
        )));
    }

    paths
}

fn configured_wake_model_paths() -> Vec<std::path::PathBuf> {
    if let Ok(models) = std::env::var("WAKE_WORD_MODELS") {
        let paths: Vec<_> = models
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .collect();

        if !paths.is_empty() {
            return paths;
        }
    }

    if let Ok(model) = std::env::var("WAKE_WORD_MODEL") {
        let model = model.trim();

        if !model.is_empty() {
            return vec![std::path::PathBuf::from(model)];
        }
    }

    default_wake_model_paths()
}

fn should_run_orb() -> bool {
    if let Ok(value) = std::env::var("HYUSK_ORB") {
        return !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        );
    }

    !gnome_indicator_enabled()
}

fn gnome_indicator_enabled() -> bool {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output();

    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).contains("hyusk@hyusk.local"),
        Err(_) => false,
    }
}

/// Standalone microphone/classifier diagnostic: `HYUSK_WAKE_TEST=1 cargo run`.
///
/// Runs only the wake detector (with debug output) for N seconds and prints
/// every detection event, so a dead, quiet, or misbehaving microphone and a
/// mis-tuned classifier are visible without starting the whole agent.
async fn wake_test_mode() -> Result<()> {
    let seconds = std::env::var("HYUSK_WAKE_TEST")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(25);

    let wake_model_paths: Vec<_> = configured_wake_model_paths()
        .into_iter()
        .filter(|path| path.exists())
        .collect();

    let wake_threshold = std::env::var("WAKE_WORD_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.35);

    let detector = WakeWordDetector::new_many(wake_model_paths)?
        .with_threshold(wake_threshold)
        .with_cooldown(Duration::from_millis(1_500))
        .with_denoise(
            std::env::var("WAKE_WORD_DENOISE")
                .map(|value| {
                    !matches!(
                        value.to_ascii_lowercase().as_str(),
                        "0" | "false" | "off" | "no"
                    )
                })
                .unwrap_or(true),
        )
        .with_clap(
            std::env::var("WAKE_CLAP_ENABLED")
                .map(|value| {
                    !matches!(
                        value.to_ascii_lowercase().as_str(),
                        "0" | "false" | "off" | "no"
                    )
                })
                .unwrap_or(false),
        );

    let (event_tx, mut event_rx) = mpsc::channel::<HyuskEvent>(8);

    let resume = WakeResume::new();
    let shutdown = Arc::new(AtomicBool::new(false));

    let test_resume = resume.clone();

    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            println!(
                "
>>> [TEST] Detection event received: {event:?}\n"
            );

            // Let the detector keep listening after each hit.
            test_resume.resume();
        }
    });

    println!("🎙 Wake test: clap and say the wake word now ({seconds} seconds)...\n");

    let test_shutdown = shutdown.clone();

    let run = detector.run(event_tx, resume, shutdown, Arc::new(AtomicBool::new(false)));

    let _ = tokio::time::timeout(Duration::from_secs(seconds), run).await;

    test_shutdown.store(true, Ordering::SeqCst);

    println!("\n🎙 Wake test finished.");

    Ok(())
}

/// Standalone speech-to-text diagnostic: `HYUSK_STT_TEST=1 cargo run`.
///
/// Records from the default microphone and prints the captured level and the
/// transcription, so a silent or discarded recording is visible without a wake
/// word.
async fn stt_test_mode() -> Result<()> {
    let model = std::env::var("STT_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_model_path("models/ggml-base.en.bin"));

    println!("🎤 STT test using {}", model);

    let stt = SpeechToText::new(&model)?;

    println!("🗣 Speak now (up to 6 seconds)...");

    let text = stt.transcribe_from_microphone(6.0).await?;

    println!("📝 Transcription: {:?}", text);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    if std::env::var_os("HYUSK_WAKE_TEST").is_some() {
        return wake_test_mode().await;
    }

    if std::env::var_os("HYUSK_STT_TEST").is_some() {
        return stt_test_mode().await;
    }

    println!();
    println!("        🦋 HYUSK");
    println!("   Local AI Computer Agent");
    println!();

    tools::computer::cleanup_screenshots();

    // ==========================================
    // OpenRouter
    // ==========================================

    let api_key = std::env::var("OPENROUTER_API_KEY").context("OPENROUTER_API_KEY is missing")?;

    let model =
        std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());

    let client = OpenRouterClient::new(api_key, base_url);

    // ==========================================
    // Tools
    // ==========================================

    let mut tools = ToolRegistry::new();

    tools.register(ComputerTool::new());

    #[cfg(target_os = "linux")]
    tools.register(AccessibilityTool::new());

    tools.register(MemoryTool::new());

    tools.register(ShellTool::new());

    tools.register(ProcessTool::new());

    tools.register(MediaTool::new());

    let agent = Agent::new(client, model, tools);

    // ==========================================
    // Audio
    // ==========================================

    println!("🔊 Testing audio...");

    match test_audio().await {
        Ok(_) => {
            println!("✅ Audio device found!");
        }

        Err(error) => {
            eprintln!("⚠️ Audio test failed: {}", error);
        }
    }

    // ==========================================
    // Channels
    // ==========================================

    /*
     * Anything that wants the Agent to do
     * something sends through agent_tx.
     */
    let (agent_tx, agent_rx) = mpsc::channel::<HyuskEvent>(32);

    /*
     * Agent/runtime sends UI events through
     * ui_tx.
     */
    let (ui_tx, mut ui_rx) = mpsc::channel::<HyuskEvent>(32);

    // ==========================================
    // Speech-to-text
    // ==========================================

    let stt_model = std::env::var("STT_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_model_path("models/ggml-base.en.bin"));

    let stt = match SpeechToText::new(&stt_model) {
        Ok(stt) => {
            println!("✅ Speech-to-text model loaded: {}", stt_model);

            Some(Arc::new(stt))
        }

        Err(error) => {
            eprintln!("⚠️ Speech-to-text disabled: {}", error);

            None
        }
    };

    /*
     * The wake detector pauses after a detection so STT can use the
     * microphone. Runtime calls `resume` when transcription is done.
     */
    let wake_resume = WakeResume::new();
    let shutdown = Arc::new(AtomicBool::new(false));

    /*
     * Set while the agent is speaking or otherwise playing audio, so the wake
     * detector ignores its own output (there is no echo cancellation).
     */
    let output_active = Arc::new(AtomicBool::new(false));

    // ==========================================
    // Agent task
    // ==========================================

    let agent_ui_tx = ui_tx.clone();

    let agent_event_tx = agent_tx.clone();

    let runtime_wake_resume = wake_resume.clone();

    let runtime_stt = stt.clone();

    let runtime_output_active = output_active.clone();

    tokio::spawn(async move {
        if let Err(error) = agent::run_agent_task(
            agent,
            agent_rx,
            agent_event_tx,
            agent_ui_tx,
            runtime_wake_resume,
            runtime_stt,
            runtime_output_active,
        )
        .await
        {
            eprintln!("❌ Agent task stopped: {}", error);
        }
    });

    // ==========================================
    // Text input
    // ==========================================

    let stdin_agent_tx = agent_tx.clone();

    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let text = line.trim().to_string();

            if text.is_empty() {
                continue;
            }

            if stdin_agent_tx
                .send(HyuskEvent::UserInput(text))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // ==========================================
    // Wake task
    // ==========================================

    let wake_enabled = std::env::var("WAKE_WORD_ENABLED")
        .map(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true);

    let mut wake_started = false;

    if wake_enabled {
        let wake_clap_enabled = std::env::var("WAKE_CLAP_ENABLED")
            .map(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);

        /*
         * Only keep classifiers that actually exist. When clap wake is on and
         * nothing is downloaded yet, the detector still starts so a hand clap
         * always works. With clap wake off, the original model discovery
         * (including its "model missing" error) is preserved.
         */
        let configured: Vec<_> = configured_wake_model_paths();
        let mut wake_model_paths = configured
            .iter()
            .filter(|path| path.exists())
            .cloned()
            .collect::<Vec<_>>();

        for path in &configured {
            if !path.exists() {
                eprintln!("⚠️ Wake model not found (skipped): {}", path.display());
            }
        }

        if wake_model_paths.is_empty() && !wake_clap_enabled {
            wake_model_paths = configured_wake_model_paths();
        }

        let wake_threshold = std::env::var("WAKE_WORD_THRESHOLD")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.4);

        let wake_min_rms = std::env::var("WAKE_WORD_MIN_RMS")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.003);

        let wake_cooldown = std::env::var("WAKE_WORD_COOLDOWN_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_500);

        let wake_denoise = std::env::var("WAKE_WORD_DENOISE")
            .map(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);

        let wake_inference_step_ms = std::env::var("WAKE_WORD_STEP_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(200);

        let wake_clap_sensitivity = std::env::var("WAKE_CLAP_SENSITIVITY")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(6.0);

        let wake_clap_min_peak = std::env::var("WAKE_CLAP_MIN_PEAK")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.03);

        let wake_model_summary = {
            let joined = wake_model_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");

            if joined.is_empty() {
                "(clap only)".to_string()
            } else {
                joined
            }
        };

        match WakeWordDetector::new_many(wake_model_paths.clone()) {
            Ok(wake_detector) => {
                let wake_detector = wake_detector
                    .with_threshold(wake_threshold)
                    .with_cooldown(Duration::from_millis(wake_cooldown))
                    .with_min_rms(wake_min_rms)
                    .with_denoise(wake_denoise)
                    .with_inference_step_ms(wake_inference_step_ms)
                    .with_clap(wake_clap_enabled)
                    .with_clap_sensitivity(wake_clap_sensitivity)
                    .with_clap_min_peak(wake_clap_min_peak);

                let wake_agent_tx = agent_tx.clone();
                let detector_resume = wake_resume.clone();
                let detector_shutdown = shutdown.clone();
                let detector_output_active = output_active.clone();

                wake_started = true;

                println!(
                    "🎙 Wake-word detector enabled: {} (threshold {:.2}, min RMS {:.3}, cooldown {} ms, denoise {}, step {} ms, clap {})",
                    wake_model_summary,
                    wake_threshold,
                    wake_min_rms,
                    wake_cooldown,
                    wake_denoise,
                    wake_inference_step_ms,
                    wake_clap_enabled
                );

                tokio::spawn(async move {
                    if let Err(error) = wake_detector
                        .run(
                            wake_agent_tx,
                            detector_resume,
                            detector_shutdown,
                            detector_output_active,
                        )
                        .await
                    {
                        eprintln!("[Wake] Detector stopped: {}", error);
                    }
                });
            }

            Err(error) => {
                eprintln!("⚠️ Wake-word detector disabled: {}", error);
            }
        }
    } else {
        println!("🎙 Wake-word detector disabled by WAKE_WORD_ENABLED");
    }

    // ==========================================
    // UI
    // ==========================================

    HyuskState::Hidden.publish();

    println!();
    println!("🦋 Hyusk is running.");

    let vision = std::env::var("MODEL_VISION")
        .map(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true);

    println!(
        "👁 Screenshot images: {}",
        if vision {
            "attached to the model"
        } else {
            "disabled (OCR mode)"
        }
    );

    if wake_started {
        println!("🎙 Listening for the configured wake word...");
    }

    println!("⌨️  Type a message in this terminal and press Enter.");

    println!();

    if should_run_orb() {
        ui::run(ui_rx).map_err(|error| anyhow::anyhow!("UI error: {}", error))?;
    } else {
        println!("🦋 GNOME top-bar indicator mode (HYUSK_ORB=0). Press Ctrl-C to quit.");

        /*
         * Nothing consumes the UI event channel in indicator mode (the
         * indicator reads the state file directly). Drain it so senders can
         * never block on a full channel.
         */
        tokio::spawn(async move { while ui_rx.recv().await.is_some() {} });

        tokio::signal::ctrl_c().await?;
    }

    // ==========================================
    // Shutdown
    // ==========================================

    /*
     * Stop the microphone loop before the runtime tears down. It waits on the
     * wake resume signal while paused, so release it as well.
     */
    shutdown.store(true, Ordering::SeqCst);
    wake_resume.resume();

    let _ = agent_tx.send(HyuskEvent::Shutdown).await;

    // Give the wake thread and runtime a moment to unwind, then exit hard so a
    // detached blocking microphone task cannot keep the process alive.
    tokio::time::sleep(Duration::from_millis(250)).await;
    std::process::exit(0);
}
