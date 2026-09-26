mod agent;
mod apps;
mod credentials;
mod link;
mod model;
mod settings;
mod speech;
mod status;
mod system_info;
mod timing;
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
use serde::Deserialize;
use tokio::sync::mpsc;

use agent::Agent;
use model::openrouter::OpenRouterClient;
use model::SharedActiveModel;

use tools::{
    computer::ComputerTool, media::MediaTool, memory::MemoryTool, mobile::MobileTool,
    process::ProcessTool, shell::ShellTool, ToolRegistry,
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

fn emergency_stop_path() -> std::path::PathBuf {
    let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&directory).join("hyusk-stop")
}

fn control_path() -> std::path::PathBuf {
    let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&directory).join("hyusk-control.json")
}

fn pairing_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|directory| std::path::PathBuf::from(directory).join("hyusk-pairing.json"))
}

fn publish_pairing(payload: &link::PairingPayload) -> Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let path = pairing_path().context("XDG_RUNTIME_DIR is required for secure phone pairing")?;
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temp)?;
    file.write_all(&serde_json::to_vec(payload)?)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    Ok(())
}

fn catalog_cache_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| {
            std::env::var("HOME").map(|home| std::path::PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    base.join("hyusk").join("models.json")
}

fn load_cached_catalogs() -> Option<Vec<status::Catalog>> {
    let mut catalogs: Vec<status::Catalog> =
        serde_json::from_str(&std::fs::read_to_string(catalog_cache_path()).ok()?).ok()?;
    for catalog in &mut catalogs {
        catalog.fresh = false;
    }
    Some(catalogs)
}

#[derive(Deserialize)]
struct ExtensionControl {
    revision: u64,
    action: String,
    provider: Option<String>,
    model: Option<String>,
    /// Preferred workflow name field for standalone clients. `text` remains
    /// supported for the GNOME extension and older control writers.
    workflow: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ScheduleControl {
    #[serde(default)]
    workflow: Option<String>,
    #[serde(default)]
    text: Option<String>,
    delay_seconds: u64,
}

fn control_schedule(control: &ExtensionControl) -> Option<ScheduleControl> {
    let payload = control.text.as_deref()?;
    let parsed: ScheduleControl = serde_json::from_str(payload).ok()?;
    (parsed.delay_seconds > 0).then_some(parsed)
}

fn control_workflow_name(control: &ExtensionControl) -> Option<&str> {
    control
        .workflow
        .as_deref()
        .or(control.text.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

async fn refresh_catalogs(
    openrouter: Option<OpenRouterClient>,
    openai: Option<OpenRouterClient>,
    bedrock: Option<OpenRouterClient>,
) {
    let mut catalogs = Vec::new();
    let cached = load_cached_catalogs().unwrap_or_default();
    for (provider, client) in [
        ("openrouter", openrouter),
        ("openai", openai),
        ("bedrock", bedrock),
    ] {
        let Some(client) = client else {
            if provider != "openrouter" {
                catalogs.push(status::Catalog {
                    provider: format!("{provider} (key needed)"),
                    models: Vec::new(),
                    fresh: true,
                });
            }
            continue;
        };
        let (models, fresh) = match client.list_models().await {
            Ok(models) => (
                models
                    .into_iter()
                    .map(|(id, label)| status::Model { id, label })
                    .collect(),
                true,
            ),
            Err(error) => {
                eprintln!("[Model] Could not refresh {provider} catalog: {error:#}");
                (
                    cached
                        .iter()
                        .find(|catalog| catalog.provider == provider)
                        .map(|catalog| catalog.models.clone())
                        .unwrap_or_default(),
                    false,
                )
            }
        };
        catalogs.push(status::Catalog {
            provider: provider.to_string(),
            models,
            fresh,
        });
    }
    catalogs.push(status::Catalog {
        provider: "bedrock-sonic".to_string(),
        models: vec![status::Model {
            id: "amazon.nova-2-sonic-v1:0".to_string(),
            label: "Nova 2 Sonic · Speech to speech".to_string(),
        }],
        fresh: true,
    });
    catalogs.push(status::Catalog {
        provider: "codex".to_string(),
        models: vec![status::Model {
            id: "default".to_string(),
            label: "Codex CLI workspace mode".to_string(),
        }],
        fresh: std::process::Command::new("codex")
            .arg("--version")
            .output()
            .is_ok(),
    });
    let path = catalog_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec(&catalogs) {
        let _ = std::fs::write(path, json);
    }
    status::catalogs(catalogs);
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
        .unwrap_or(0.93);
    let wake_strong_threshold = std::env::var("WAKE_WORD_STRONG_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.94);
    let wake_min_rms = std::env::var("WAKE_WORD_MIN_RMS")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.003);
    let wake_cooldown = std::env::var("WAKE_WORD_COOLDOWN_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_500);
    let wake_min_hits = std::env::var("WAKE_WORD_MIN_HITS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1);
    let wake_hit_window = std::env::var("WAKE_WORD_HIT_WINDOW_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);

    let detector = WakeWordDetector::new_many(wake_model_paths)?
        .with_threshold(wake_threshold)
        .with_strong_threshold(wake_strong_threshold)
        .with_confirmation(wake_min_hits, Duration::from_millis(wake_hit_window))
        .with_cooldown(Duration::from_millis(wake_cooldown))
        .with_min_rms(wake_min_rms)
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
        .unwrap_or_else(|| default_model_path("models/ggml-large-v3-turbo.bin"));

    println!("🎤 STT test using {}", model);

    let stt = SpeechToText::new(&model)?;

    println!("🗣 Speak now (up to 6 seconds)...");

    let text = stt.transcribe_from_microphone(6.0).await?;

    println!("📝 Transcription: {:?}", text);

    Ok(())
}

/// End-to-end laptop smoke test for Nova Sonic, independent of the selected
/// chat-completions provider: `HYUSK_NOVA_SONIC_TEST=1 cargo run --bin hyusk_agent`.
async fn nova_sonic_test_mode() -> Result<()> {
    let client = model::bedrock_sonic::BedrockSonicClient::from_aws_config().await?;
    let test_text = std::env::var("HYUSK_NOVA_SONIC_TEST_TEXT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let test_wav = std::env::var("HYUSK_NOVA_SONIC_TEST_WAV").ok();
    let stt = if test_text.is_none() && test_wav.is_none() {
        let model = std::env::var("STT_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_model_path("models/ggml-base.en.bin"));
        Some(std::sync::Arc::new(SpeechToText::new(&model)?))
    } else {
        None
    };

    let test_tool = std::env::var_os("HYUSK_NOVA_SONIC_TEST_TOOL").is_some();
    let tools = if test_tool {
        vec![
            serde_json::json!({"type":"function","function":{"name":"get_time","description":"Read the current local date and time","parameters":{"type":"object","properties":{},"required":[]}}}),
        ]
    } else {
        Vec::new()
    };
    let system_prompt = if test_tool {
        "You are Hyusk, a concise spoken computer assistant. When asked for the time, call get_time and answer using its result."
    } else {
        "You are Hyusk, a concise spoken computer assistant. Reply naturally in one or two sentences. Do not claim to have performed computer actions unless a tool was actually used."
    };
    let mut session = client.start_session(system_prompt, &tools).await?;
    let mut audio_send = None;
    let mut capture_task = None;
    if let Some(text) = test_text.as_deref() {
        println!("⌨️ Testing Nova Sonic with text input (no microphone or tools)…");
        session.send_text(text).await?;
    } else if let Some(path) = test_wav {
        let wav = std::fs::read(&path)?;
        anyhow::ensure!(
            wav.len() > 44
                && &wav[0..4] == b"RIFF"
                && &wav[8..12] == b"WAVE"
                && u16::from_le_bytes([wav[22], wav[23]]) == 1
                && u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]) == 16_000
                && u16::from_le_bytes([wav[34], wav[35]]) == 16,
            "Test WAV must be standard 16 kHz mono PCM16"
        );
        let (frames_tx, frames_rx) = tokio::sync::mpsc::channel(32);
        audio_send = Some(session.send_live_audio_in_background(frames_rx)?);
        println!("🎤 Streaming test recording: {path}");
        capture_task = Some(tokio::spawn(async move {
            let mut cadence = tokio::time::interval(std::time::Duration::from_millis(32));
            for frame in wav[44..].chunks(1_024) {
                cadence.tick().await;
                frames_tx.send(frame.to_vec()).await?;
            }
            Ok::<bool, anyhow::Error>(true)
        }));
    } else if let Some(stt) = stt {
        let (frames_tx, frames_rx) = tokio::sync::mpsc::channel(32);
        audio_send = Some(session.send_live_audio_in_background(frames_rx)?);
        println!("🎤 Speak a short request (up to 8 seconds)…");
        capture_task = Some(tokio::task::spawn_blocking(move || {
            stt.stream_audio_for_speech_model(
                8.0,
                frames_tx,
                &tokio_util::sync::CancellationToken::new(),
            )
        }));
    }
    let mut heard = test_text.unwrap_or_default();
    let mut said = String::new();
    let mut audio_player = None;
    let mut tool_calls = 0usize;
    let started = std::time::Instant::now();
    loop {
        use model::bedrock_sonic::SonicEvent;
        let event = tokio::select! {
            captured = async {
                if let Some(task) = capture_task.as_mut() {
                    Some(task.await)
                } else {
                    std::future::pending().await
                }
            } => {
                capture_task.take();
                if !captured.expect("capture branch requires a task")?? {
                    anyhow::bail!("No speech was captured; check the microphone and try again");
                }
                continue;
            }
            result = session.recv() => result?,
        };
        match event {
            SonicEvent::UserText(text) => heard.push_str(&text),
            SonicEvent::UserTranscriptEnd => {}
            SonicEvent::AssistantText(text) => said.push_str(&text),
            SonicEvent::AssistantAudio(chunk) => {
                if audio_player.is_none() {
                    println!(
                        "🔊 First reply audio after {:.2}s",
                        started.elapsed().as_secs_f32()
                    );
                    audio_player = Some(speech::speech::Pcm16Player::start()?);
                }
                if let Some(player) = audio_player.as_mut() {
                    player.write(&chunk).await?;
                }
            }
            SonicEvent::ToolUse(call) => {
                if let Some(task) = audio_send.take() {
                    task.await??;
                }
                tool_calls += 1;
                let now = chrono::Local::now().to_rfc3339();
                println!("🛠 Nova requested {}: {now}", call.name);
                session.send_tool_result(&call, &now, true).await?;
            }
            SonicEvent::CompletionEnd(reason) if reason != "TOOL_USE" => break,
            SonicEvent::Interrupted => {
                if let Some(mut player) = audio_player.take() {
                    player.stop().await?;
                }
            }
            SonicEvent::StreamEnd => break,
            SonicEvent::CompletionEnd(_) | SonicEvent::Other => {}
        }
    }
    if let Some(task) = audio_send {
        task.await??;
    }
    if let Some(task) = capture_task {
        task.await??;
    }
    session.close().await?;
    println!("📝 Nova heard: {}", heard.trim());
    println!("💬 Nova said: {}", said.trim());
    println!("⏱ Turn finished in {:.2}s", started.elapsed().as_secs_f32());
    if test_tool {
        println!("🛠 Tool calls: {tool_calls}");
    }
    if let Some(player) = audio_player {
        player.finish().await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    if std::env::var_os("HYUSK_NOVA_SONIC_TEST").is_some() {
        return nova_sonic_test_mode().await;
    }

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
    let _ = std::fs::remove_file(emergency_stop_path());

    // Keys saved from the extension take effect after its service restart,
    // even when older environment keys are still present.
    let openrouter_key = credentials::lookup("openrouter")
        .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
        .filter(|key| !key.trim().is_empty());

    let model =
        std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());

    let openrouter_client = openrouter_key.map(|key| OpenRouterClient::new(key, base_url));
    let openai_client = credentials::lookup("openai")
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .filter(|key| !key.trim().is_empty())
        .map(|key| OpenRouterClient::new(key, "https://api.openai.com/v1".to_string()));
    let bedrock_client = credentials::lookup("bedrock")
        .or_else(|| std::env::var("BEDROCK_API_KEY").ok())
        .or_else(|| std::env::var("AWS_BEARER_TOKEN_BEDROCK").ok())
        .or_else(|| std::env::var("AWS_API_KEY").ok())
        .filter(|key| !key.trim().is_empty())
        .map(|key| {
            let region =
                std::env::var("BEDROCK_REGION").unwrap_or_else(|_| "us-east-1".to_string());
            OpenRouterClient::new(key, format!("https://bedrock-mantle.{region}.api.aws/v1"))
        });
    let bedrock_sonic_client =
        match model::bedrock_sonic::BedrockSonicClient::from_aws_config().await {
            Ok(client) => Some(Arc::new(client)),
            Err(error) => {
                eprintln!("[Nova Sonic] Provider unavailable: {error:#}");
                None
            }
        };
    let client = openrouter_client
        .clone()
        .or_else(|| openai_client.clone())
        .or_else(|| bedrock_client.clone())
        .or_else(|| {
            bedrock_sonic_client
                .as_ref()
                .map(|_| OpenRouterClient::new("".into(), "https://invalid.local/v1".into()))
        })
        .context(
            "Configure an OpenRouter, OpenAI, Bedrock API key, or AWS CLI profile for Nova Sonic",
        )?;
    let default_provider = if openrouter_client.is_some() {
        "openrouter"
    } else if openai_client.is_some() {
        "openai"
    } else if bedrock_client.is_some() {
        "bedrock"
    } else {
        "bedrock-sonic"
    };
    let default_model = match default_provider {
        "openai" => "gpt-4o-mini".to_string(),
        "bedrock" => {
            std::env::var("BEDROCK_MODEL").unwrap_or_else(|_| "openai.gpt-oss-20b".to_string())
        }
        "bedrock-sonic" => "amazon.nova-2-sonic-v1:0".to_string(),
        _ => model.clone(),
    };
    let active_api_model =
        SharedActiveModel::new(default_provider, default_model.clone(), client.clone());

    // ==========================================
    // Tools
    // ==========================================

    let mut tools = ToolRegistry::new();

    tools.register(ComputerTool::new());

    #[cfg(target_os = "linux")]
    tools.register(AccessibilityTool::new());

    #[cfg(target_os = "linux")]
    tools.register(tools::gnome_doctor::GnomeDoctorTool::new());

    #[cfg(target_os = "linux")]
    tools.register(tools::window::WindowTool::new());

    tools.register(MemoryTool::new());

    tools.register(ShellTool::new());

    tools.register(ProcessTool::new());

    tools.register(MediaTool::new());
    tools.register(tools::timer::TimerTool::default());

    let brave_search_key = std::env::var("BRAVE_SEARCH_API_KEY")
        .ok()
        .or_else(|| credentials::lookup("brave_search"));
    match tools::web_search::WebSearchTool::new(brave_search_key) {
        Ok(tool) => tools.register(tool),
        Err(error) => eprintln!("[Web] Native search unavailable: {error}"),
    }

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

    // ==========================================
    // Android/mobile link
    // ==========================================

    let link_enabled = std::env::var("HYUSK_LINK_ENABLED")
        .map(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(false);
    if let Some(path) = pairing_path() {
        let _ = std::fs::remove_file(path);
    }
    let mut link_server: Option<Arc<link::LinkServer>> = None;
    if link_enabled {
        let (command_tx, mut command_rx) = mpsc::channel(32);
        let (outbound_tx, _) = tokio::sync::broadcast::channel(32);
        let channels = link::LinkChannels::new(agent_tx.clone(), command_tx, outbound_tx.clone());
        match link::start_link_server(channels).await {
            Ok(server) => {
                let server = Arc::new(server);
                println!("[Link] Listening on {}", server.local_addr());
                if let Err(error) = publish_pairing(&server.pairing_payload()) {
                    eprintln!("[Link] Could not publish pairing code: {error:#}");
                }
                tools.register(MobileTool::new(Arc::clone(&server)));
                link_server = Some(server);

                let mut status_updates = status::subscribe();
                let status_outbound = outbound_tx.clone();
                tokio::spawn(async move {
                    while let Ok(snapshot) = status_updates.recv().await {
                        let _ = status_outbound.send(link::OutboundNotification::event(
                            serde_json::json!({"kind": "status", "status": snapshot}),
                        ));
                    }
                });

                let link_agent_tx = agent_tx.clone();
                tokio::spawn(async move {
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            link::LinkCommand::Approval(value) => {
                                let answer = if value.approved { "yes" } else { "no" };
                                let _ = link_agent_tx
                                    .send(HyuskEvent::UserInput(answer.to_string()))
                                    .await;
                            }
                            link::LinkCommand::Invoke(value) => {
                                eprintln!(
                                    "[Link] Ignored unsupported inbound device.invoke: {}",
                                    value.name
                                );
                            }
                            link::LinkCommand::InvocationResult(value) => {
                                let message = if value.success {
                                    format!("Phone action {} completed", value.invocation_id)
                                } else {
                                    format!(
                                        "Phone action {} failed: {}",
                                        value.invocation_id,
                                        value.error.as_deref().unwrap_or("unknown error")
                                    )
                                };
                                status::card("mobile", message, false);
                            }
                            link::LinkCommand::MemorySync(value) => {
                                eprintln!("[Link] Memory sync received ({} item(s)); merge support is pending", value.items.len());
                            }
                            link::LinkCommand::WorkflowSync(value) => {
                                eprintln!("[Link] Workflow sync received ({} item(s)); merge support is pending", value.workflows.len());
                            }
                        }
                    }
                });
            }
            Err(error) => eprintln!("[Link] Mobile link disabled: {error}"),
        }
    }

    // The task tool needs the runtime channel so a completed background task
    // can wake the main assistant without replacing the active user turn.
    tools.register(tools::scheduler::SchedulerTool::new(agent_tx.clone()));
    tools.register(tools::task::TaskTool::new(
        agent_tx.clone(),
        active_api_model.clone(),
    ));

    let saved_selection = settings::load();
    let selected_provider = saved_selection
        .as_ref()
        .map(|s| s.provider.clone())
        .unwrap_or_else(|| default_provider.to_string());
    let selected_model = saved_selection
        .as_ref()
        .map(|s| s.model.clone())
        .unwrap_or_else(|| default_model.clone());
    let mut agent = Agent::new(client.clone(), model, tools)
        .with_openrouter(openrouter_client.clone())
        .with_openai(openai_client.clone())
        .with_bedrock(bedrock_client.clone())
        .with_bedrock_sonic(bedrock_sonic_client.clone())
        .with_active_api_model(active_api_model);
    let active_selection =
        if let Err(error) = agent.restore_model(&selected_provider, &selected_model) {
            eprintln!(
                "[Model] Saved selection unavailable ({error}); using {default_provider} default"
            );
            agent.restore_model(default_provider, &default_model)?;
            settings::ModelSelection {
                provider: default_provider.to_string(),
                model: default_model,
            }
        } else {
            settings::ModelSelection {
                provider: selected_provider,
                model: selected_model,
            }
        };
    status::model(&active_selection.provider, &active_selection.model);
    settings::save(&active_selection);

    if let Some(cached) = load_cached_catalogs() {
        status::catalogs(cached);
    }
    let catalog_openrouter = openrouter_client.clone();
    let catalog_openai = openai_client.clone();
    let catalog_bedrock = bedrock_client.clone();
    tokio::spawn(async move {
        refresh_catalogs(catalog_openrouter, catalog_openai, catalog_bedrock).await;
    });

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
            println!("✅ Speech input ready (Whisper loads only when local STT is used)");

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

    // The GNOME indicator writes this small runtime marker from its Stop
    // action. Polling avoids giving a Shell extension broad control over the
    // process while still working reliably on a locked-down Wayland session.
    let stop_tx = agent_tx.clone();
    tokio::spawn(async move {
        let path = emergency_stop_path();
        loop {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if path.exists() {
                let _ = std::fs::remove_file(&path);
                if stop_tx.send(HyuskEvent::StopRequested).await.is_err() {
                    break;
                }
            }
        }
    });

    let control_tx = agent_tx.clone();
    let control_openrouter = openrouter_client.clone();
    let control_openai = openai_client.clone();
    let control_bedrock = bedrock_client.clone();
    let control_link = link_server.clone();
    tokio::spawn(async move {
        let path = control_path();
        // Do not replay the last menu action after a service restart. The
        // extension uses wall-clock revisions so actions remain monotonic
        // across extension reloads as well.
        let mut seen = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str::<ExtensionControl>(&contents).ok())
            .map(|control| control.revision)
            .unwrap_or(0);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(control) = serde_json::from_str::<ExtensionControl>(&contents) else {
                continue;
            };
            if control.revision <= seen {
                continue;
            }
            seen = control.revision;
            match control.action.as_str() {
                "set_model" => {
                    if let (Some(provider), Some(model)) = (control.provider, control.model) {
                        let _ = control_tx
                            .send(HyuskEvent::ModelSelected { provider, model })
                            .await;
                    }
                }
                "refresh_models" => {
                    let a = control_openrouter.clone();
                    let b = control_openai.clone();
                    let c = control_bedrock.clone();
                    tokio::spawn(async move {
                        refresh_catalogs(a, b, c).await;
                    });
                }
                "pair_phone" => {
                    if let Some(server) = control_link.as_ref() {
                        if let Err(error) = publish_pairing(&server.refresh_pairing()) {
                            eprintln!("[Link] Could not refresh pairing code: {error:#}");
                        }
                    } else {
                        status::card(
                            "device",
                            "Laptop link is not enabled. Run setup-mobile-link.sh first.",
                            false,
                        );
                    }
                }
                "dismiss_card" => status::dismiss(),
                "approve" => {
                    let _ = control_tx
                        .send(HyuskEvent::UserInput("yes".to_string()))
                        .await;
                }
                "deny" => {
                    let _ = control_tx
                        .send(HyuskEvent::UserInput("no".to_string()))
                        .await;
                }
                "stop_listening" => {
                    let _ = control_tx.send(HyuskEvent::StopListening).await;
                }
                "run_workflow" => {
                    if let Some(name) = control_workflow_name(&control) {
                        // `commands::parse` reads commands.json for each
                        // request, so a standalone editor can save the file
                        // and run the workflow without a service restart.
                        let _ = control_tx
                            .send(HyuskEvent::UserInput(format!("run {name}")))
                            .await;
                    } else {
                        status::card("workflow", "A workflow name is required.", false);
                    }
                }
                "reload_workflows" => {
                    // Workflow configuration is intentionally not cached;
                    // the next run observes the latest commands.json. This
                    // action exists so editors can explicitly acknowledge a
                    // save through the same control channel.
                    status::card("workflow", "Workflow configuration reloaded.", false);
                }
                "schedule_workflow" => {
                    match control_schedule(&control).and_then(|schedule| {
                        schedule
                            .workflow
                            .filter(|name| !name.trim().is_empty())
                            .map(|name| (name, schedule.delay_seconds))
                    }) {
                        Some((name, seconds)) => {
                            let _ = control_tx
                                .send(HyuskEvent::UserInput(format!(
                                    "schedule workflow {} in {seconds} seconds",
                                    name.trim()
                                )))
                                .await;
                        }
                        None => status::card(
                            "schedule",
                            "Choose a workflow and a valid delay first.",
                            false,
                        ),
                    }
                }
                "schedule_reminder" => {
                    match control_schedule(&control).and_then(|schedule| {
                        schedule
                            .text
                            .filter(|text| !text.trim().is_empty())
                            .map(|text| (text, schedule.delay_seconds))
                    }) {
                        Some((text, seconds)) => {
                            let _ = control_tx
                                .send(HyuskEvent::UserInput(format!(
                                    "remind me in {seconds} seconds to {}",
                                    text.trim()
                                )))
                                .await;
                        }
                        None => status::card(
                            "schedule",
                            "Enter a reminder and a valid delay first.",
                            false,
                        ),
                    }
                }
                _ => {}
            }
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
            .unwrap_or(0.93);

        let wake_strong_threshold = std::env::var("WAKE_WORD_STRONG_THRESHOLD")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.94);

        let wake_min_rms = std::env::var("WAKE_WORD_MIN_RMS")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.003);

        let wake_cooldown = std::env::var("WAKE_WORD_COOLDOWN_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_500);

        let wake_min_hits = std::env::var("WAKE_WORD_MIN_HITS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);

        let wake_hit_window = std::env::var("WAKE_WORD_HIT_WINDOW_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_000);

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
                    .with_strong_threshold(wake_strong_threshold)
                    .with_confirmation(wake_min_hits, Duration::from_millis(wake_hit_window))
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
                    "🎙 Wake-word detector enabled: {} (threshold {:.2}, strong {:.2}, confirmation {} hits/{} ms, min RMS {:.3}, cooldown {} ms, denoise {}, step {} ms, clap {})",
                    wake_model_summary,
                    wake_threshold,
                    wake_strong_threshold,
                    wake_min_hits,
                    wake_hit_window,
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

    if let Some(server) = link_server.and_then(Arc::into_inner) {
        server.shutdown().await;
    }
    if let Some(path) = pairing_path() {
        let _ = std::fs::remove_file(path);
    }

    // Give the wake thread and runtime a moment to unwind, then exit hard so a
    // detached blocking microphone task cannot keep the process alive.
    tokio::time::sleep(Duration::from_millis(250)).await;
    std::process::exit(0);
}

#[cfg(test)]
mod control_tests {
    use super::{control_schedule, control_workflow_name, ExtensionControl};

    #[test]
    fn standalone_workflow_field_is_preferred() {
        let control: ExtensionControl = serde_json::from_str(
            r#"{"revision":7,"action":"run_workflow","workflow":" Focus ","text":"legacy"}"#,
        )
        .expect("valid workflow control payload");

        assert_eq!(control_workflow_name(&control), Some("Focus"));
    }

    #[test]
    fn legacy_text_field_remains_supported() {
        let control: ExtensionControl =
            serde_json::from_str(r#"{"revision":8,"action":"run_workflow","text":"  Focus  "}"#)
                .expect("valid legacy workflow control payload");

        assert_eq!(control_workflow_name(&control), Some("Focus"));
    }

    #[test]
    fn blank_workflow_payload_is_rejected() {
        let control: ExtensionControl = serde_json::from_str(
            r#"{"revision":9,"action":"run_workflow","workflow":"  ","text":""}"#,
        )
        .expect("valid empty workflow control payload");

        assert_eq!(control_workflow_name(&control), None);
    }

    #[test]
    fn schedule_payload_is_decoded_from_control_text() {
        let control: ExtensionControl = serde_json::from_str(
            r#"{"revision":10,"action":"schedule_workflow","text":"{\"workflow\":\"Focus\",\"delay_seconds\":900}"}"#,
        )
        .expect("valid schedule control payload");
        let schedule = control_schedule(&control).expect("decoded schedule");
        assert_eq!(schedule.workflow.as_deref(), Some("Focus"));
        assert_eq!(schedule.delay_seconds, 900);
    }
}
