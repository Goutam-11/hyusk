//! API credentials are kept in the logged-in user's GNOME Keyring.

use std::process::Command;

pub fn lookup(provider: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "hyusk", "provider", provider])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}
