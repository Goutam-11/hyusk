use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::timeout;
use tokio::{process::Command, sync::Mutex};
use tokio_util::sync::CancellationToken;

use super::{Tool, ToolResult};

pub struct TimerTool {
    timers: Arc<Mutex<HashMap<String, ActiveTimer>>>,
    next_id: AtomicU64,
}

impl Default for TimerTool {
    fn default() -> Self {
        Self {
            timers: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        }
    }
}

struct ActiveTimer {
    id: u64,
    ends_at_ms: u64,
    cancel: CancellationToken,
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
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let cancel = CancellationToken::new();
                let task_cancel = cancel.clone();
                let replaced = {
                    let mut timers = self.timers.lock().await;
                    let replaced = timers.insert(
                        label.clone(),
                        ActiveTimer {
                            id,
                            ends_at_ms,
                            cancel,
                        },
                    );
                    publish_next_timer(&timers);
                    replaced
                };
                if let Some(replaced) = replaced {
                    replaced.cancel.cancel();
                }

                let timers = Arc::clone(&self.timers);
                let handle = tokio::spawn(async move {
                    tokio::select! {
                        _ = task_cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                    }

                    let completed = {
                        let mut timers = timers.lock().await;
                        if timers
                            .get(&notify_label)
                            .is_some_and(|timer| timer.id == id)
                        {
                            timers.remove(&notify_label);
                            publish_next_timer(&timers);
                            true
                        } else {
                            false
                        }
                    };
                    if !completed {
                        return;
                    }

                    crate::status::card(
                        "timer_finished",
                        format!("{notify_label} is finished"),
                        true,
                    );
                    let notification = format!("{notify_label} is finished");
                    run_bounded_command(
                        "notify-send",
                        &["Hyusk timer", notification.as_str()],
                        Duration::from_secs(3),
                    )
                    .await;
                    run_bounded_command(
                        "canberra-gtk-play",
                        &["-i", "alarm-clock-elapsed"],
                        Duration::from_secs(3),
                    )
                    .await;
                });
                drop(handle);
                Ok(ToolResult::success(format!(
                    "Timer '{label}' set for {seconds} seconds."
                )))
            }
            "cancel" => {
                let cancelled = {
                    let mut timers = self.timers.lock().await;
                    let timer = timers.remove(&label);
                    if let Some(timer) = &timer {
                        timer.cancel.cancel();
                    }
                    publish_next_timer(&timers);
                    timer.is_some()
                };
                if cancelled {
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
                let mut entries: Vec<_> = timers
                    .iter()
                    .map(|(name, timer)| (timer.ends_at_ms, name))
                    .collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(right.1)));
                Ok(ToolResult::success(if entries.is_empty() {
                    "No active timers.".to_string()
                } else {
                    format!(
                        "Active timers: {}",
                        entries
                            .into_iter()
                            .map(|(ends_at_ms, name)| format!(
                                "{name} ({}s remaining)",
                                ends_at_ms.saturating_sub(now_ms()).div_ceil(1000)
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }))
            }
            _ => Ok(ToolResult::failure(
                "action must be set, cancel, list, or show_time",
            )),
        }
    }
}

async fn run_bounded_command(command: &str, args: &[&str], limit: Duration) -> bool {
    matches!(
        timeout(
            limit,
            Command::new(command)
                .args(args)
                .kill_on_drop(true)
                .status()
        )
        .await,
        Ok(Ok(status)) if status.success()
    )
}

fn publish_next_timer(timers: &HashMap<String, ActiveTimer>) {
    if let Some((label, timer)) = timers.iter().min_by(|left, right| {
        left.1
            .ends_at_ms
            .cmp(&right.1.ends_at_ms)
            .then(left.0.cmp(right.0))
    }) {
        crate::status::timer(label.clone(), timer.ends_at_ms);
    } else {
        crate::status::clear_timer();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{run_bounded_command, TimerTool};
    use crate::tools::{Tool, ToolResult};
    use std::time::Duration;

    #[tokio::test]
    async fn setting_same_name_replaces_timer_instead_of_orphaning_it() {
        let tool = TimerTool::default();
        let first: ToolResult = tool
            .execute(r#"{"action":"set","seconds":3600,"label":"tea"}"#)
            .await
            .unwrap();
        assert!(first.success);
        let first_id = tool.timers.lock().await["tea"].id;

        let second = tool
            .execute(r#"{"action":"set","seconds":60,"label":"tea"}"#)
            .await
            .unwrap();
        assert!(second.success);

        let timers = tool.timers.lock().await;
        assert_eq!(timers.len(), 1);
        assert_ne!(timers["tea"].id, first_id);
        assert!(timers["tea"].ends_at_ms < super::now_ms() + 120_000);
        drop(timers);

        let cancelled = tool
            .execute(r#"{"action":"cancel","label":"tea"}"#)
            .await
            .unwrap();
        assert!(cancelled.success);
        assert!(tool.timers.lock().await.is_empty());
    }

    #[tokio::test]
    async fn alarm_command_timeout_does_not_block_timer_completion() {
        let completed = run_bounded_command("sleep", &["2"], Duration::from_millis(25)).await;
        assert!(!completed);
    }
}
