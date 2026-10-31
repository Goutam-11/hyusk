use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use super::{Tool, ToolResult};
use crate::tools::manifest::{RiskClass, Schema, Source, ToolManifest, CORE_VERSION};

/// Cap tool output so a chatty command cannot blow up the model context.
fn truncate_output(text: &str) -> String {
    const MAX_CHARS: usize = 20_000;

    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }

    let truncated: String = text.chars().take(MAX_CHARS).collect();

    format!("{truncated}\n... (output truncated)")
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<(Vec<u8>, bool)> {
    // Keep a bounded amount in memory while continuing to drain the pipe so
    // large command output cannot stall the child or consume unbounded RAM.
    const MAX_BYTES: usize = 80_000;
    let mut output = Vec::with_capacity(MAX_BYTES);
    let mut buffer = [0; 8192];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }

        let remaining = MAX_BYTES.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }

    Ok((output, truncated))
}

fn captured_output(bytes: Vec<u8>, truncated: bool) -> String {
    let text = String::from_utf8_lossy(&bytes);
    let mut output = truncate_output(&text);
    if truncated && !output.ends_with("... (output truncated)") {
        output.push_str("\n... (output truncated)");
    }
    output
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

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Failed to execute shell command")?;

        let mut stdout = child
            .stdout
            .take()
            .context("Failed to capture shell stdout")?;
        let mut stderr = child
            .stderr
            .take()
            .context("Failed to capture shell stderr")?;
        let output = match tokio::time::timeout(FOREGROUND_TIMEOUT, async {
            let (stdout, stderr, status) = tokio::join!(
                read_bounded(&mut stdout),
                read_bounded(&mut stderr),
                child.wait(),
            );
            Ok::<_, anyhow::Error>((stdout?, stderr?, status?))
        })
        .await
        {
            Ok(output) => output.context("Failed to collect shell command output")?,

            Err(_) => {
                return Ok(ToolResult::failure(format!(
                    "Command timed out after {} seconds. Run long-lived programs with mode \"detached\" instead.",
                    FOREGROUND_TIMEOUT.as_secs()
                )));
            }
        };

        let (stdout, stderr, status) = output;
        let stdout = captured_output(stdout.0, stdout.1);
        let stderr = captured_output(stderr.0, stderr.1);

        if status.success() {
            Ok(ToolResult::success(format!(
                "Exit status: {}\nSTDOUT:\n{}",
                status, stdout,
            )))
        } else {
            Ok(ToolResult::failure(format!(
                "Exit status: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                status, stdout, stderr,
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
            Ok(child) => {
                let pid = child.id();
                reap_child(child);
                Ok(ToolResult::success(format!(
                    "Process started. PID: {pid:?}"
                )))
            }

            Err(error) => Ok(ToolResult::failure(format!(
                "Failed to start process: {}",
                error
            ))),
        }
    }
}

fn reap_child(mut child: Child) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = child.wait().await;
    })
}

#[async_trait]
impl Tool for ShellTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "shell",
            version: CORE_VERSION,
            description: "Run a shell command. Pass the command as the `command` argument. Use mode `foreground` (default) to wait for completion, or `detached` to launch a GUI app, browser, editor, or long-running server and return its PID without waiting.",
            schema: Schema::Fn(shell_schema),
            risk: &[RiskClass::Read, RiskClass::Mutate, RiskClass::Execute, RiskClass::System],
            capabilities: &[],
            source: Source::Core,
        }
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

fn shell_schema() -> serde_json::Value {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn bounded_reader_drains_excess_output_without_retaining_it() {
        let input = vec![b'x'; 100_000];
        let (captured, truncated) = read_bounded(Cursor::new(input)).await.unwrap();

        assert_eq!(captured.len(), 80_000);
        assert!(truncated);
        assert!(captured_output(captured, truncated).ends_with("... (output truncated)"));
    }

    #[tokio::test]
    async fn detached_launcher_is_reaped_after_it_exits() {
        let child = Command::new("sh")
            .args(["-c", "sleep 0.35"])
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), reap_child(child))
            .await
            .expect("reaper should finish after the launcher exits")
            .unwrap();
    }
}
