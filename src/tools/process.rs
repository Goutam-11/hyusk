use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;
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
            .stderr(Stdio::null());

        let mut child = match command.spawn().context("Failed to spawn process") {
            Ok(child) => child,

            Err(error) => {
                return Ok(ToolResult::failure(format!(
                    "Failed to launch '{}': {}",
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
                        "'{}' exited immediately with status: {}",
                        program, status
                    )))
                }
            }

            None => Ok(ToolResult::success(format!(
                "'{}' launched and is still running. PID: {:?}",
                program, pid
            ))),
        }
    }
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
                    "description": "The program to launch, e.g. 'firefox' or '/usr/bin/spotify'."
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
