use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;

const PLAYERCTL_TIMEOUT: Duration = Duration::from_secs(3);

use super::{Tool, ToolResult};
use crate::tools::manifest::{RiskClass, Source, ToolManifest, CORE_VERSION};

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
    let mut command = Command::new("playerctl");
    command.args(args).kill_on_drop(true);
    match tokio::time::timeout(PLAYERCTL_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

async fn playerctl_succeeded(args: &[&str]) -> bool {
    let mut command = Command::new("playerctl");
    command.args(args).kill_on_drop(true);
    matches!(
        tokio::time::timeout(PLAYERCTL_TIMEOUT, command.status()).await,
        Ok(Ok(status)) if status.success()
    )
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
        return playerctl_succeeded(&["pause"]).await;
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
        let mut command = Command::new("playerctl");
        command.args(playerctl_args).kill_on_drop(true);
        let output = match tokio::time::timeout(PLAYERCTL_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(error).context("Failed to invoke playerctl. Is it installed?");
            }
            Err(_) => {
                return Ok(ToolResult::failure(format!(
                    "playerctl did not respond within {} seconds. The media service may be stuck.",
                    PLAYERCTL_TIMEOUT.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            let body = if stdout.trim().is_empty() {
                "Media command accepted by playerctl. Inspect status if playback needs verification.".to_string()
            } else {
                stdout.trim().to_string()
            };
            Ok(ToolResult::success(body))
        } else {
            // playerctl returns a non-zero status when no player is running.
            // Surface that as a normal failure the model can read.
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                format!("playerctl failed with {}", output.status)
            };
            Ok(ToolResult::failure(detail))
        }
    }
}

#[async_trait]
impl Tool for MediaTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "media",
            version: CORE_VERSION,
            description: "Control media playback on the local desktop via MPRIS. Works with Spotify, YouTube Music in a browser, VLC, mpv, and any other MPRIS player. Requires `playerctl` to be installed. `status` returns the current track (artist, title, album) and whether it is playing; `position` returns the playback position; `shuffle`, `repeat`, and `seek` adjust playback.",
            schema: media_schema,
            risk: &[RiskClass::Read, RiskClass::Mutate, RiskClass::Execute],
            capabilities: &[],
            source: Source::Core,
        }
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
                    None => "Track",
                    Some(_) => {
                        return Ok(ToolResult::failure("Repeat mode must be off, one, or all."))
                    }
                };
                Self::run(&["loop", value]).await
            }
            MediaAction::Seek { offset } => match normalize_seek_offset(&offset) {
                Some(position) => Self::run(&["position", &position]).await,
                None => Ok(ToolResult::failure(
                    "Seek requires finite seconds such as +10, -5, or an absolute position of 60.",
                )),
            },
            MediaAction::Position => Self::run(&["position"]).await,
        }
    }
}

fn media_schema() -> serde_json::Value {
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
                "description": "Seek '+10' or '-5' seconds relative to now; unsigned values are absolute seconds. Trailing-sign playerctl notation ('10+', '5-') is also accepted."
            }
        },
        "required": ["action"]
    })
}

/// playerctl uses trailing signs for relative seeks. Passing the documented
/// `+10` directly instead jumps to absolute second ten, not ten seconds ahead.
fn normalize_seek_offset(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (number, direction) = if let Some(value) = raw.strip_prefix('+') {
        (value, "+")
    } else if let Some(value) = raw.strip_prefix('-') {
        (value, "-")
    } else if let Some(value) = raw.strip_suffix('+') {
        (value, "+")
    } else if let Some(value) = raw.strip_suffix('-') {
        (value, "-")
    } else {
        (raw, "")
    };
    if number.starts_with(['+', '-']) || number.contains(char::is_whitespace) {
        return None;
    }
    let seconds = number.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > i64::MAX as f64 / 1_000_000.0 {
        return None;
    }
    Some(format!("{number}{direction}"))
}

#[cfg(test)]
mod tests {
    use super::normalize_seek_offset;

    #[test]
    fn relative_seek_signs_match_playerctl_notation() {
        for (input, expected) in [
            ("+10", "10+"),
            ("-5", "5-"),
            ("1.5+", "1.5+"),
            ("5-", "5-"),
            ("60", "60"),
        ] {
            assert_eq!(normalize_seek_offset(input).as_deref(), Some(expected));
        }
    }

    #[test]
    fn malformed_seek_arguments_are_not_forwarded() {
        for input in ["", "NaN", "inf", "--help", "-5+", "++5", "1e999", " 1 2 "] {
            assert!(normalize_seek_offset(input).is_none(), "accepted {input}");
        }
    }
}
