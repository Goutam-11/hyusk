use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use super::{Tool, ToolResult};

/// Cap tool output so a chatty command cannot blow up the model context.
fn truncate_output(text: &str) -> String {
    const MAX_CHARS: usize = 20_000;

    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }

    let truncated: String = text.chars().take(MAX_CHARS).collect();

    format!("{truncated}\n... (output truncated)")
}

pub struct ShellTool;

#[derive(Debug, Deserialize)]
struct ShellInput {
    command: String,

    #[serde(default)]
    mode: ExecutionMode,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ExecutionMode {
    #[default]
    Foreground,

    Detached,
}

impl ShellTool {
    pub fn new() -> Self {
        Self
    }

    async fn run_foreground(command: &str) -> Result<ToolResult> {
        // A foreground command that never exits (a server, an interactive
        // prompt) used to wedge the whole turn. Bound it so the agent either
        // gets output or a clear timeout.
        const FOREGROUND_TIMEOUT: Duration = Duration::from_secs(120);

        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Failed to execute shell command")?;

        let output = match tokio::time::timeout(FOREGROUND_TIMEOUT, child.wait_with_output()).await
        {
            Ok(output) => output.context("Failed to collect shell command output")?,

            Err(_) => {
                return Ok(ToolResult::failure(format!(
                    "Command timed out after {} seconds. Run long-lived programs with mode \"detached\" instead.",
                    FOREGROUND_TIMEOUT.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);

        let stderr = String::from_utf8_lossy(&output.stderr);

        let stdout = truncate_output(&stdout);
        let stderr = truncate_output(&stderr);

        if output.status.success() {
            Ok(ToolResult::success(format!(
                "Exit status: {}\nSTDOUT:\n{}",
                output.status, stdout,
            )))
        } else {
            Ok(ToolResult::failure(format!(
                "Exit status: {}\nSTDERR:\n{}",
                output.status, stderr,
            )))
        }
    }

    async fn run_detached(command: &str) -> Result<ToolResult> {
        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Detached applications are intentionally independent of the
            // shell tool task and must remain alive after it returns.
            .kill_on_drop(false)
            .spawn();

        match child {
            Ok(child) => Ok(ToolResult::success(format!(
                "Process started. PID: {:?}",
                child.id()
            ))),

            Err(error) => Ok(ToolResult::failure(format!(
                "Failed to start process: {}",
                error
            ))),
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a shell command. Pass the command as the `command`         argument. Use mode `foreground` (default) to wait for         completion, or `detached` to launch a GUI app, browser,         editor, or long-running server and return its PID without         waiting."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run, e.g. 'ls -la' or 'firefox'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["foreground", "detached"],
                    "description": "foreground waits and returns output; detached returns the PID and does not wait."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: ShellInput =
            serde_json::from_str(input)
                .context(
                    "Shell tool input must be a JSON object with a 'command' field (and optional 'mode'). See the tool description for the schema."
                )?;

        match input.mode {
            ExecutionMode::Foreground => Self::run_foreground(&input.command).await,

            ExecutionMode::Detached => Self::run_detached(&input.command).await,
        }
    }
}
