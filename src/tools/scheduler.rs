//! Persistent one-shot reminders and deterministic workflow schedules.
//!
//! The scheduler deliberately owns only durable schedule state.  The main
//! runtime remains responsible for interpreting a workflow event, while this
//! tool wakes up, removes the due entry, and emits the event or notification.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    process::Command,
    sync::{mpsc::Sender, Mutex},
};

use crate::types::HyuskEvent;

use super::{Tool, ToolResult};

const MAX_SCHEDULES: usize = 500;
const MAX_REMINDER_CHARS: usize = 500;
const MAX_PROMPT_CHARS: usize = 4_000;

/// A persisted one-shot action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: u64,
    pub run_at_unix: u64,
    pub action: ScheduledAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduledAction {
    Reminder { text: String },
    Workflow { name: String },
    AgentTask { prompt: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ScheduleFile {
    schedules: Vec<Schedule>,
}

struct SchedulerState {
    path: PathBuf,
    schedules: Vec<Schedule>,
    next_id: u64,
    // Handles are retained so cancellation can stop a long sleeping task.
    tasks: HashMap<u64, tokio::task::JoinHandle<()>>,
}

/// Persistent scheduler exposed to the model as a tool.
pub struct SchedulerTool {
    state: Arc<Mutex<SchedulerState>>,
    events: Sender<HyuskEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum SchedulerInput {
    Schedule {
        #[serde(rename = "type")]
        schedule_type: String,
        text: Option<String>,
        workflow: Option<String>,
        prompt: Option<String>,
        after_seconds: Option<u64>,
        at_unix: Option<u64>,
    },
    List,
    Cancel {
        id: u64,
    },
}

impl SchedulerTool {
    /// Construct the scheduler using `$XDG_DATA_HOME/hyusk/schedules.json`.
    /// Existing future schedules are re-armed immediately.
    pub fn new(events: Sender<HyuskEvent>) -> Self {
        Self::with_path(default_path(), events)
    }

    /// Construct with an explicit path.  This is also useful for isolated
    /// tests and for embedders that provide their own data directory.
    pub fn with_path(path: PathBuf, events: Sender<HyuskEvent>) -> Self {
        let schedules = load_schedules(&path).unwrap_or_else(|error| {
            eprintln!("[Scheduler] Could not load {}: {error}", path.display());
            Vec::new()
        });
        let next_id = schedules
            .iter()
            .map(|schedule| schedule.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let state = Arc::new(Mutex::new(SchedulerState {
            path,
            schedules,
            next_id,
            tasks: HashMap::new(),
        }));
        let tool = Self { state, events };
        tool.rearm_pending();
        tool
    }

    fn rearm_pending(&self) {
        let schedules = self
            .state
            .try_lock()
            .map(|state| state.schedules.clone())
            .unwrap_or_default();
        for schedule in schedules {
            self.spawn_schedule(schedule);
        }
    }

    fn spawn_schedule(&self, schedule: Schedule) {
        let state = Arc::clone(&self.state);
        let events = self.events.clone();
        let id = schedule.id;
        let run_at = schedule.run_at_unix;
        let task = tokio::spawn(async move {
            let delay = run_at.saturating_sub(now_unix());
            tokio::time::sleep(Duration::from_secs(delay)).await;

            let action = {
                let mut state = state.lock().await;
                let Some(index) = state.schedules.iter().position(|item| item.id == id) else {
                    return;
                };
                let action = state.schedules.remove(index).action;
                if let Err(error) = save_state(&state) {
                    eprintln!("[Scheduler] Could not persist fired schedule {id}: {error}");
                }
                state.tasks.remove(&id);
                action
            };

            fire(id, action, events).await;
        });

        // The task cannot access the map until it is registered.  If a very
        // short schedule fires first, its cleanup is still harmless.
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let mut state = state.lock().await;
            if state.schedules.iter().any(|item| item.id == id) {
                state.tasks.insert(id, task);
            } else {
                // The schedule fired (or was cancelled) before registration;
                // do not retain a completed handle forever.
                task.abort();
            }
        });
    }
}

#[async_trait]
impl Tool for SchedulerTool {
    fn name(&self) -> &str {
        "scheduler"
    }

    fn description(&self) -> &str {
        "Schedule one-time reminders, named deterministic workflows, or an LLM agent task, persist them across restarts, list schedules, and cancel them. Use after_seconds or a Unix timestamp at_unix."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["schedule", "list", "cancel"] },
                "type": { "type": "string", "enum": ["reminder", "workflow", "agent_task"] },
                "text": { "type": "string", "description": "Reminder text." },
                "workflow": { "type": "string", "description": "Name of a configured deterministic workflow." },
                "prompt": { "type": "string", "description": "Self-contained instruction for a scheduled model-driven task." },
                "after_seconds": { "type": "integer", "minimum": 1 },
                "at_unix": { "type": "integer", "minimum": 1 },
                "id": { "type": "integer", "minimum": 1 }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: SchedulerInput = serde_json::from_str(input)
            .context("Scheduler input must be JSON with action schedule, list, or cancel")?;
        match input {
            SchedulerInput::Schedule {
                schedule_type,
                text,
                workflow,
                prompt,
                after_seconds,
                at_unix,
            } => {
                let run_at_unix = match (after_seconds, at_unix) {
                    (Some(_), Some(_)) => {
                        return Ok(ToolResult::failure(
                            "Provide either after_seconds or at_unix, not both.",
                        ));
                    }
                    (Some(seconds), None) if seconds > 0 => now_unix().saturating_add(seconds),
                    (Some(_), None) => {
                        return Ok(ToolResult::failure(
                            "after_seconds must be greater than zero.",
                        ));
                    }
                    (None, Some(timestamp)) if timestamp > now_unix() => timestamp,
                    (None, Some(_)) => {
                        return Ok(ToolResult::failure("at_unix must be in the future."));
                    }
                    (None, None) => {
                        return Ok(ToolResult::failure(
                            "Provide after_seconds or a future at_unix timestamp.",
                        ));
                    }
                };

                let action = match schedule_type.trim().to_ascii_lowercase().as_str() {
                    "reminder" => {
                        let Some(text) = text.filter(|value| !value.trim().is_empty()) else {
                            return Ok(ToolResult::failure("Reminder text must not be empty."));
                        };
                        if text.chars().count() > MAX_REMINDER_CHARS {
                            return Ok(ToolResult::failure("Reminder text is too long."));
                        }
                        ScheduledAction::Reminder {
                            text: text.trim().to_string(),
                        }
                    }
                    "workflow" => {
                        let Some(name) = workflow.filter(|value| !value.trim().is_empty()) else {
                            return Ok(ToolResult::failure("Workflow name must not be empty."));
                        };
                        ScheduledAction::Workflow {
                            name: name.trim().to_string(),
                        }
                    }
                    "agent_task" => {
                        let Some(prompt) = prompt.filter(|value| !value.trim().is_empty()) else {
                            return Ok(ToolResult::failure("Agent task prompt must not be empty."));
                        };
                        if prompt.chars().count() > MAX_PROMPT_CHARS {
                            return Ok(ToolResult::failure("Agent task prompt is too long."));
                        }
                        ScheduledAction::AgentTask {
                            prompt: prompt.trim().to_string(),
                        }
                    }
                    _ => {
                        return Ok(ToolResult::failure(
                            "type must be reminder, workflow, or agent_task.",
                        ))
                    }
                };

                let (id, seconds) = {
                    let mut state = self.state.lock().await;
                    if state.schedules.len() >= MAX_SCHEDULES {
                        return Ok(ToolResult::failure(
                            "The schedule limit has been reached. Cancel an existing item first.",
                        ));
                    }
                    let id = state.next_id;
                    state.next_id = state.next_id.saturating_add(1);
                    state.schedules.push(Schedule {
                        id,
                        run_at_unix,
                        action: action.clone(),
                    });
                    save_state(&state)?;
                    (id, run_at_unix.saturating_sub(now_unix()))
                };
                self.spawn_schedule(Schedule {
                    id,
                    run_at_unix,
                    action: action.clone(),
                });
                Ok(ToolResult::success(format!(
                    "Scheduled {} as #{id} in {seconds} seconds.",
                    action_label(&action)
                )))
            }
            SchedulerInput::List => {
                let state = self.state.lock().await;
                if state.schedules.is_empty() {
                    return Ok(ToolResult::success("No scheduled reminders or workflows."));
                }
                let mut entries = state.schedules.clone();
                entries.sort_by_key(|schedule| schedule.run_at_unix);
                let output = entries
                    .iter()
                    .map(|schedule| {
                        format!(
                            "#{} at {} ({})",
                            schedule.id,
                            schedule.run_at_unix,
                            action_label(&schedule.action)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(ToolResult::success(output))
            }
            SchedulerInput::Cancel { id } => {
                let mut state = self.state.lock().await;
                let Some(index) = state.schedules.iter().position(|item| item.id == id) else {
                    return Ok(ToolResult::failure(format!(
                        "No scheduled item named #{id}."
                    )));
                };
                state.schedules.remove(index);
                if let Some(task) = state.tasks.remove(&id) {
                    task.abort();
                }
                save_state(&state)?;
                Ok(ToolResult::success(format!(
                    "Cancelled scheduled item #{id}."
                )))
            }
        }
    }
}

async fn fire(id: u64, action: ScheduledAction, events: Sender<HyuskEvent>) {
    match action {
        ScheduledAction::Reminder { text } => {
            crate::status::card("reminder", &text, true);
            let _ = Command::new("notify-send")
                .args(["Hyusk reminder", &text])
                .status()
                .await;
        }
        ScheduledAction::Workflow { name } => {
            crate::status::card("scheduled_workflow", format!("Running {name}"), false);
            // The runtime queues this event behind any active foreground
            // turn, then resolves the name through the deterministic
            // workflow command path.
            let _ = events
                .send(HyuskEvent::ScheduledWorkflow { id, name })
                .await;
        }
        ScheduledAction::AgentTask { prompt } => {
            crate::status::card("scheduled_task", "Scheduled agent task is ready", false);
            let _ = events
                .send(HyuskEvent::ScheduledAgentTask { id, prompt })
                .await;
        }
    }
}

fn action_label(action: &ScheduledAction) -> String {
    match action {
        ScheduledAction::Reminder { text } => format!("reminder: {text}"),
        ScheduledAction::Workflow { name } => format!("workflow: {name}"),
        ScheduledAction::AgentTask { prompt } => format!("agent task: {prompt}"),
    }
}

fn default_path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    data.join("hyusk").join("schedules.json")
}

fn load_schedules(path: &Path) -> Result<Vec<Schedule>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let file: ScheduleFile =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    Ok(file.schedules)
}

fn save_state(state: &SchedulerState) -> Result<()> {
    let file = ScheduleFile {
        schedules: state.schedules.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&file)?;
    if let Some(parent) = state.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = state.path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &state.path)?;
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[test]
    fn schedule_file_round_trips_both_action_types() {
        let file = ScheduleFile {
            schedules: vec![
                Schedule {
                    id: 1,
                    run_at_unix: 1_900_000_000,
                    action: ScheduledAction::Reminder {
                        text: "stretch".to_string(),
                    },
                },
                Schedule {
                    id: 2,
                    run_at_unix: 1_900_000_100,
                    action: ScheduledAction::Workflow {
                        name: "start work".to_string(),
                    },
                },
                Schedule {
                    id: 3,
                    run_at_unix: 1_900_000_200,
                    action: ScheduledAction::AgentTask {
                        prompt: "summarize today's notes".to_string(),
                    },
                },
            ],
        };
        let encoded = serde_json::to_string(&file).expect("encode");
        let decoded: ScheduleFile = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded.schedules, file.schedules);
    }

    #[test]
    fn persistence_creates_parent_and_reloads() {
        let directory = std::env::temp_dir().join(format!("hyusk-scheduler-{}", now_unix()));
        let path = directory.join("nested").join("schedules.json");
        let state = SchedulerState {
            path: path.clone(),
            schedules: vec![Schedule {
                id: 7,
                run_at_unix: 1_900_000_000,
                action: ScheduledAction::Reminder {
                    text: "call home".into(),
                },
            }],
            next_id: 8,
            tasks: HashMap::new(),
        };
        save_state(&state).expect("save");
        assert_eq!(load_schedules(&path).expect("load"), state.schedules);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn schedule_and_cancel_update_persistent_state() {
        let directory = std::env::temp_dir().join(format!("hyusk-scheduler-tool-{}", now_unix()));
        let path = directory.join("schedules.json");
        let (events, _received) = mpsc::channel(2);
        let tool = SchedulerTool::with_path(path.clone(), events);
        let scheduled = tool
            .execute(r#"{"action":"schedule","type":"reminder","text":"water","after_seconds":60}"#)
            .await
            .expect("schedule result");
        assert!(scheduled.success);
        let saved = load_schedules(&path).expect("saved schedules");
        assert_eq!(saved.len(), 1);
        let id = saved[0].id;
        let cancelled = tool
            .execute(&format!(r#"{{"action":"cancel","id":{id}}}"#))
            .await
            .expect("cancel result");
        assert!(cancelled.success);
        assert!(load_schedules(&path).expect("saved schedules").is_empty());
        tokio::time::sleep(Duration::from_millis(5)).await;
        let _ = std::fs::remove_dir_all(directory);
    }
}
