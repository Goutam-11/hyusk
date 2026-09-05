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

    async fn launch(
        program: &str,
        args: &[String],
    ) -> Result<ToolResult> {
        let mut command = Command::new(program);

        command.args(args);

        let mut child = match command
            .spawn()
            .context("Failed to spawn process")
        {
            Ok(child) => child,

            Err(error) => {
                return Ok(ToolResult::failure(
                    format!(
                        "Failed to launch '{}': {}",
                        program,
                        error
                    ),
                ));
            }
        };

        let pid = child.id();

        // Give the process a very short window to fail
        // immediately.
        sleep(Duration::from_millis(300)).await;

        match child.try_wait()? {
            Some(status) => {
                if status.success() {
                    Ok(ToolResult::success(
                        format!(
                            "'{}' exited successfully immediately. PID: {:?}",
                            program,
                            pid
                        ),
                    ))
                } else {
                    Ok(ToolResult::failure(
                        format!(
                            "'{}' exited immediately with status: {}",
                            program,
                            status
                        ),
                    ))
                }
            }

            None => {
                Ok(ToolResult::success(
                    format!(
                        "'{}' launched and is still running. PID: {:?}",
                        program,
                        pid
                    ),
                ))
            }
        }
    }
}

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        r#"Launch and manage local applications and processes.

Input MUST be valid JSON.

Launch an application:

{
  "action": "launch",
  "program": "PROGRAM_NAME",
  "args": []
}

Use this tool for:
- Browsers
- GUI applications
- Editors
- Music applications
- Long-running programs
- Servers

Do not use the shell tool when simply launching
an application. Use this process tool instead.

The process tool does not wait for the application
to finish.
"#
    }

    async fn execute(
        &self,
        input: &str,
    ) -> Result<ToolResult> {
        let input: ProcessInput =
            serde_json::from_str(input)
                .context(
                    "Process tool input must be valid JSON"
                )?;

        match input.action {
            ProcessAction::Launch => {
                Self::launch(
                    &input.program,
                    &input.args,
                )
                .await
            }
        }
    }
}