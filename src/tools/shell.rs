use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use super::{Tool, ToolResult};

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

    async fn run_foreground(
        command: &str,
    ) -> Result<ToolResult> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
            .context("Failed to execute shell command")?;

        let stdout =
            String::from_utf8_lossy(
                &output.stdout
            );

        let stderr =
            String::from_utf8_lossy(
                &output.stderr
            );

        if output.status.success() {
            Ok(ToolResult::success(
                format!(
                    "Exit status: {}\nSTDOUT:\n{}",
                    output.status,
                    stdout,
                ),
            ))
        } else {
            Ok(ToolResult::failure(
                format!(
                    "Exit status: {}\nSTDERR:\n{}",
                    output.status,
                    stderr,
                ),
            ))
        }
    }

    async fn run_detached(
        command: &str,
    ) -> Result<ToolResult> {
        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .spawn();
    
        match child {
            Ok(child) => {
                Ok(ToolResult::success(
                    format!(
                        "Process started. PID: {:?}",
                        child.id()
                    ),
                ))
            }
    
            Err(error) => {
                Ok(ToolResult::failure(
                    format!(
                        "Failed to start process: {}",
                        error
                    ),
                ))
            }
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        r#"When calling the shell tool, the tool input must itself
        be JSON.
        
        Example foreground command:
        
        {
          "tool": "shell",
          "input": "{\"command\":\"ls -la\",\"mode\":\"foreground\"}"
        }
        
        Example detached application:
        
        {
          "tool": "shell",
          "input": "{\"command\":\"brave-browser\",\"mode\":\"detached\"}"
        }
        
        Use detached mode for:
        - GUI applications
        - browsers
        - editors
        - servers
        - long-running programs
        
        Never wait for a GUI application to close.
"#
    }

    async fn execute(
        &self,
        input: &str,
    ) -> Result<ToolResult> {
        let input: ShellInput =
            serde_json::from_str(input)
                .context(
                    "Shell input must be valid JSON"
                )?;

        match input.mode {
            ExecutionMode::Foreground => {
                Self::run_foreground(
                    &input.command
                )
                .await
            }

            ExecutionMode::Detached => {
                Self::run_detached(
                    &input.command
                )
                .await
            }
        }
    }
}