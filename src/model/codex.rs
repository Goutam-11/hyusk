//! Adapter for an already-authenticated local Codex CLI.
//!
//! Hyusk never reads Codex credential/configuration files. The CLI handles its
//! own sign-in and is constrained to the configured workspace.
use std::{path::PathBuf, process::Stdio};

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use crate::types::Message;

struct TemporaryOutput(PathBuf);

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

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
        // Owner-only (0600) under the private runtime directory: the last
        // message can quote workspace content, so it must not be world-readable.
        let output = TemporaryOutput(
            crate::paths::private_file("hyusk-codex-", "txt")
                .context("Failed to create a private Codex output file")?,
        );
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
            .arg(&output.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // If the parent task is cancelled, do not leave a hidden Codex
            // process continuing to edit the workspace in the background.
            .kill_on_drop(true);
        if model != "default" {
            command.arg("--model").arg(model);
        }
        command.arg(format!("You are Hyusk's workspace coding mode. Work only in the current workspace. Do not access personal files, desktop applications, or credentials. Give a concise final report.\n\n{prompt}"));
        let result = command
            .output()
            .await
            .context("Failed to start Codex CLI")?;
        let text = tokio::fs::read_to_string(&output.0)
            .await
            .unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_output_is_owner_only_and_removed_on_drop() {
        let path = crate::paths::private_file("hyusk-codex-output-test-", "txt")
            .expect("create private report");
        std::fs::write(&path, "temporary report").expect("write temporary report");

        assert_eq!(
            path.parent(),
            Some(crate::paths::private_dir().as_path()),
            "{}",
            path.display()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("report exists")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let guard = TemporaryOutput(path.clone());
        assert!(path.exists());
        drop(guard);
        assert!(!path.exists());
    }
}
