use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use tokio::{process::Command, sync::Mutex};

use super::{Tool, ToolResult};

#[derive(Default)]
pub struct TimerTool {
    timers: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

#[derive(Deserialize)]
struct Input {
    action: String,
    seconds: Option<u64>,
    label: Option<String>,
}

#[async_trait]
impl Tool for TimerTool {
    fn name(&self) -> &str {
        "timer"
    }

    fn description(&self) -> &str {
        "Set, cancel, or list Hyusk timers. A finished timer shows a desktop notification."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["set", "cancel", "list", "show_time"] },
                "seconds": { "type": "integer", "minimum": 1 },
                "label": { "type": "string" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: Input = serde_json::from_str(input)?;
        let label = input.label.unwrap_or_else(|| "Timer".to_string());
        match input.action.as_str() {
            "set" => {
                let seconds = input.seconds.unwrap_or(0);
                if seconds == 0 {
                    return Ok(ToolResult::failure("seconds must be greater than zero"));
                }
                let notify_label = label.clone();
                let ends_at_ms = now_ms().saturating_add(seconds.saturating_mul(1000));
                crate::status::timer(label.clone(), ends_at_ms);
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    crate::status::clear_timer();
                    crate::status::card(
                        "timer_finished",
                        format!("{notify_label} is finished"),
                        true,
                    );
                    let _ = Command::new("notify-send")
                        .args(["Hyusk timer", &format!("{notify_label} is finished")])
                        .status()
                        .await;
                    let _ = Command::new("canberra-gtk-play")
                        .args(["-i", "alarm-clock-elapsed"])
                        .status()
                        .await;
                });
                self.timers.lock().await.insert(label.clone(), handle);
                Ok(ToolResult::success(format!(
                    "Timer '{label}' set for {seconds} seconds."
                )))
            }
            "cancel" => {
                if let Some(handle) = self.timers.lock().await.remove(&label) {
                    handle.abort();
                    crate::status::clear_timer();
                    Ok(ToolResult::success(format!("Cancelled timer '{label}'.")))
                } else {
                    Ok(ToolResult::failure(format!("No timer named '{label}'.")))
                }
            }
            "show_time" => {
                let time = chrono::Local::now().format("%H:%M").to_string();
                crate::status::card("clock", &time, false);
                Ok(ToolResult::success(format!("It is {time}.")))
            }
            "list" => {
                let timers = self.timers.lock().await;
                let names: Vec<_> = timers.keys().cloned().collect();
                Ok(ToolResult::success(if names.is_empty() {
                    "No active timers.".to_string()
                } else {
                    format!("Active timers: {}", names.join(", "))
                }))
            }
            _ => Ok(ToolResult::failure("action must be set, cancel, or list")),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
