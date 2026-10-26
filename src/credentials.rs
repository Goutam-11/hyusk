//! API credentials are kept in the logged-in user's GNOME Keyring.

use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

pub async fn lookup(provider: &str) -> Option<String> {
    let mut command = tokio::process::Command::new("secret-tool");
    command
        .args(["lookup", "application", "hyusk", "provider", provider])
        .kill_on_drop(true);
    let output = match tokio::time::timeout(Duration::from_secs(2), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return None,
        Err(_) => {
            eprintln!("[Credentials] Keyring lookup timed out for provider {provider}");
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Save an API key in the login keyring.
///
/// The key travels on stdin, never on the command line, so it does not show up
/// in the process list while the entry is written.
pub async fn store(provider: &str, key: &str) -> Result<()> {
    let mut command = tokio::process::Command::new("secret-tool");
    command
        .args([
            "store",
            &format!("--label=Hyusk {provider} API key"),
            "application",
            "hyusk",
            "provider",
            provider,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("secret-tool is unavailable; is the login keyring running?")?;
    let mut stdin = child
        .stdin
        .take()
        .context("could not open secret-tool stdin")?;
    stdin.write_all(format!("{key}\n").as_bytes()).await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .context("saving the keyring entry timed out")??;
    anyhow::ensure!(
        output.status.success(),
        "secret-tool failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
