use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use super::{Tool, ToolResult};

pub struct MediaTool;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum MediaAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
    Volume {
        level: u32,
    },
    VolumeUp,
    VolumeDown,
    Status,
    Shuffle,
    Repeat {
        #[serde(default)]
        mode: Option<String>,
    },
    Seek {
        offset: String,
    },
    Position,
}

/// Run `playerctl` and return trimmed stdout, or an empty string on failure.
async fn playerctl_quiet(args: &[&str]) -> String {
    match Command::new("playerctl").args(args).output().await {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// Pause the active MPRIS player if it is currently playing.
///
/// Returns `true` when something was actually paused, so the caller knows to
/// resume it after the command. Used to quiet media while the agent listens.
pub async fn pause_if_playing() -> bool {
    if playerctl_quiet(&["status"])
        .await
        .eq_ignore_ascii_case("playing")
    {
        let _ = playerctl_quiet(&["pause"]).await;

        return true;
    }

    false
}

/// Resume playback (a no-op if nothing is paused or no player exists).
pub async fn resume() {
    let _ = playerctl_quiet(&["play"]).await;
}

impl MediaTool {
    pub fn new() -> Self {
        Self
    }

    async fn run(playerctl_args: &[&str]) -> Result<ToolResult> {
        let output = Command::new("playerctl")
            .args(playerctl_args)
            .output()
            .await
            .context("Failed to invoke playerctl. Is it installed?")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            let body = if stdout.trim().is_empty() {
                "Command executed. No active MPRIS player responded with output.".to_string()
            } else {
                stdout.trim().to_string()
            };
            Ok(ToolResult::success(body))
        } else {
            // playerctl returns a non-zero status when no player is running.
            // Surface that as a normal failure the model can read.
            Ok(ToolResult::failure(stderr.trim().to_string()))
        }
    }
}

#[async_trait]
impl Tool for MediaTool {
    fn name(&self) -> &str {
        "media"
    }

    fn description(&self) -> &str {
        "Control media playback on the local desktop via MPRIS.         Works with Spotify, YouTube Music in a browser, VLC, mpv,         and any other MPRIS player. Requires `playerctl` to be         installed. `status` returns the current track (artist, title,         album) and whether it is playing; `position` returns the playback         position; `shuffle`, `repeat`, and `seek` adjust playback."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "play", "pause", "play_pause", "next",
                        "previous", "stop", "status", "volume",
                        "volume_up", "volume_down", "shuffle", "repeat",
                        "seek", "position"
                    ],
                    "description": "What to do."
                },
                "level": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                    "description": "Volume level 0-100. Only used when action is 'volume'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["off", "one", "all"],
                    "description": "Repeat mode for the `repeat` action."
                },
                "offset": {
                    "type": "string",
                    "description": "Seek offset for `seek`, e.g. '+10' or '-5' seconds."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let action: MediaAction =
            serde_json::from_str(input).context("Media tool input must be valid JSON")?;

        match action {
            MediaAction::Play => Self::run(&["play"]).await,
            MediaAction::Pause => Self::run(&["pause"]).await,
            MediaAction::PlayPause => Self::run(&["play-pause"]).await,
            MediaAction::Next => Self::run(&["next"]).await,
            MediaAction::Previous => Self::run(&["previous"]).await,
            MediaAction::Stop => Self::run(&["stop"]).await,
            MediaAction::VolumeUp => Self::run(&["volume", "0.05+"]).await,
            MediaAction::VolumeDown => Self::run(&["volume", "0.05-"]).await,
            MediaAction::Volume { level } => {
                if level > 100 {
                    return Ok(ToolResult::failure(
                        "Volume level must be between 0 and 100.",
                    ));
                }
                // playerctl expects a 0.0-1.0 float. Zero-pad so level 5
                // becomes "0.05" instead of "0.5" (the old 10x-too-loud bug).
                let value = if level == 100 {
                    "1.0".to_string()
                } else {
                    format!("0.{:02}", level)
                };
                Self::run(&["volume", &value]).await
            }
            MediaAction::Status => {
                let metadata = Self::run(&[
                    "metadata",
                    "--format",
                    "{{ artist }}|{{ title }}|{{ album }}|{{ status }}",
                ])
                .await?;
                if !metadata.success {
                    return Ok(metadata);
                }
                let raw = &metadata.output;
                let mut parts = raw.splitn(4, '|');
                let artist = parts.next().unwrap_or("").to_string();
                let title = parts.next().unwrap_or("").to_string();
                let album = parts.next().unwrap_or("").to_string();
                let status = parts.next().unwrap_or("").to_string();
                Ok(ToolResult::success(format!(
                    "Artist: {}\nTitle: {}\nAlbum: {}\nStatus: {}",
                    artist, title, album, status
                )))
            }
            MediaAction::Shuffle => Self::run(&["shuffle", "toggle"]).await,
            MediaAction::Repeat { mode } => {
                let value = match mode.as_deref() {
                    Some("off") | Some("none") => "None",
                    Some("one") | Some("track") => "Track",
                    Some("all") | Some("playlist") => "Playlist",
                    _ => "Track",
                };
                Self::run(&["loop", value]).await
            }
            MediaAction::Seek { offset } => {
                let offset = offset.trim();
                if offset.is_empty() {
                    return Ok(ToolResult::failure("`seek` requires an `offset`."));
                }
                Self::run(&["position", offset]).await
            }
            MediaAction::Position => Self::run(&["position"]).await,
        }
    }
}
