//! Adapter for an already-authenticated local Codex CLI.
//!
//! Hyusk never reads Codex credential/configuration files. The CLI handles its
//! own sign-in and is constrained to the configured workspace.
use std::{
    path::PathBuf,
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use crate::types::Message;

#[derive(Clone)]
pub struct CodexClient {
    workspace: PathBuf,
}

impl CodexClient {
    pub fn discover() -> Result<Self> {
        let workspace = std::env::var("HYUSK_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);
        let workspace = workspace
            .canonicalize()
            .context("HYUSK_WORKSPACE does not exist")?;
        if !workspace.is_dir() {
            bail!("HYUSK_WORKSPACE must be a directory");
        }
        Ok(Self { workspace })
    }

    pub fn for_workspace(workspace: PathBuf) -> Result<Self> {
        let workspace = workspace
            .canonicalize()
            .context("workspace does not exist")?;
        if !workspace.is_dir() {
            bail!("workspace must be a directory");
        }
        Ok(Self { workspace })
    }

    pub async fn chat(&self, model: &str, messages: &[Message]) -> Result<Message> {
        let prompt = messages
            .iter()
            .map(|message| format!("{}: {}", message.role, message.text()))
            .collect::<Vec<_>>()
            .join("\n\n");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let output = std::env::temp_dir().join(format!("hyusk-codex-{stamp}.txt"));
        let mut command = Command::new("codex");
        command
            .args([
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--approve-for-me",
                "--ephemeral",
                "-C",
            ])
            .arg(&self.workspace)
            .arg("--output-last-message")
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if model != "default" {
            command.arg("--model").arg(model);
        }
        command.arg(format!("You are Hyusk's workspace coding mode. Work only in the current workspace. Do not access personal files, desktop applications, or credentials. Give a concise final report.\n\n{prompt}"));
        let result = command
            .output()
            .await
            .context("Failed to start Codex CLI")?;
        let text = tokio::fs::read_to_string(&output).await.unwrap_or_default();
        let _ = tokio::fs::remove_file(&output).await;
        if !result.status.success() {
            bail!(
                "Codex CLI failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }
        Ok(Message::assistant(if text.trim().is_empty() {
            "Codex completed without a final report.".to_string()
        } else {
            text
        }))
    }
}
