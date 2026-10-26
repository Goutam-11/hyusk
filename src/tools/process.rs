use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::{Child, Command};
use tokio::time::sleep;

use super::{Tool, ToolResult};

pub struct ProcessTool;

#[derive(Debug, Deserialize)]
struct ProcessInput {
    action: ProcessAction,

    program: String,

    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProcessAction {
    Launch,
}

impl ProcessTool {
    pub fn new() -> Self {
        Self
    }

    async fn launch(program: &str, args: &[String]) -> Result<ToolResult> {
        // Resolve friendly names (e.g. `brave` -> `flatpak run
        // com.brave.Browser`) so the agent does not have to know which apps are
        // Flatpaks, and does not try several wrong binaries in a row.
        let (program, mut resolved_args) = crate::apps::resolve(program);

        resolved_args.extend_from_slice(args);

        let mut command = Command::new(&program);

        command.args(&resolved_args);

        // GUI apps write chatter (updates, GPU warnings, sandbox noise) to
        // stderr. Detach their stdio so it does not spill into the agent's
        // console; the tool still reports spawn failures and immediate exits.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // GUI applications must outlive this short-lived launcher future.
            // Keep this explicit so a later refactor cannot accidentally make
            // dropping the Tokio child handle terminate the application.
            .kill_on_drop(false);

        let mut child = match command.spawn().context("Failed to spawn process") {
            Ok(child) => child,

            Err(error) => {
                return Ok(ToolResult::failure(format!(
                    "Launch failed for '{}': {:#}. This is an execution failure, not an approval request. Check whether the application is installed; use friendly aliases such as 'brave' or 'chrome' for Flatpak browsers. For the separate automation browser, use browser status/start.",
                    program, error
                )));
            }
        };

        let pid = child.id();

        // Give the process a very short window to fail
        // immediately.
        sleep(Duration::from_millis(300)).await;

        match child.try_wait()? {
            Some(status) => {
                if status.success() {
                    Ok(ToolResult::success(format!(
                        "'{}' exited successfully immediately. PID: {:?}",
                        program, pid
                    )))
                } else {
                    Ok(ToolResult::failure(format!(
                        "'{}' exited immediately with status: {}. Launch failed; this is not an approval request. Check the installed application and its launch arguments.",
                        program, status
                    )))
                }
            }

            None => {
                // GUI launchers often exit after handing work to an existing
                // app instance. Keep a waiter alive so that launcher is reaped
                // instead of becoming a zombie later in Hyusk's long session.
                reap_child(child);
                Ok(ToolResult::success(format!(
                    "'{}' launched and is still running. PID: {:?}",
                    program, pid
                )))
            }
        }
    }
}

fn reap_child(mut child: Child) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = child.wait().await;
    })
}

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Launch a local application or process by name without         going through the shell. Use this for browsers, GUI apps,         editors, music players, and any long-running program. The         action is `launch`. The process does not block the agent."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["launch"],
                    "description": "What to do with the program. Currently only 'launch'."
                },
                "program": {
                    "type": "string",
                    "description": "Prefer friendly app aliases: 'brave', 'chrome', 'firefox', 'terminal', 'files'. Aliases resolve Flatpak packaging automatically; do not probe guessed browser executable names. A spawn failure means launch failed, not that permission is needed."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional command-line arguments."
                }
            },
            "required": ["action", "program"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: ProcessInput =
            serde_json::from_str(input)
                .context(
                    "Process tool input must be a JSON object with 'action' and 'program' fields. See the tool description for the schema."
                )?;

        match input.action {
            ProcessAction::Launch => Self::launch(&input.program, &input.args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawned_launcher_is_reaped_after_it_exits() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 0.35"])
            .spawn()
            .unwrap();
        assert!(child.try_wait().unwrap().is_none());
        tokio::time::timeout(Duration::from_secs(2), reap_child(child))
            .await
            .expect("reaper should finish after the launcher exits")
            .unwrap();
    }

    #[tokio::test]
    async fn missing_executable_reports_cause_not_permission_request() {
        let result = ProcessTool::launch("/hyusk-test-missing-executable", &[])
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.as_agent_message();
        assert!(message.contains("No such file or directory"));
        assert!(message.contains("not an approval request"));
    }
}
