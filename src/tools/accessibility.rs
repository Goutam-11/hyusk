use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

use super::{Tool, ToolResult};

/*
 * Walking a large AT-SPI tree is slow (every node is a D-Bus round trip);
 * 30 s used to time out mid-walk, which killed and respawned the bridge in
 * a loop without ever returning data. Give big walks room to finish.
 */
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

fn action_may_change_ui(request: &Value) -> bool {
    matches!(
        request.get("action").and_then(Value::as_str),
        Some("click" | "set_text" | "focus")
    )
}

struct Bridge {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Bridge {
    async fn spawn() -> Result<Self> {
        let script = bridge_path();

        if !script.exists() {
            bail!("AT-SPI bridge script is missing: {}", script.display());
        }

        let mut child = tokio::process::Command::new("python3")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn python3 with {}", script.display()))?;

        let stdin = child.stdin.take().context("bridge stdin missing")?;
        let stdout = child.stdout.take().context("bridge stdout missing")?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    async fn request(&mut self, request: &Value) -> Result<Value> {
        let mut line = serde_json::to_vec(request).context("Failed to encode bridge request")?;
        line.push(b'\n');

        self.stdin
            .write_all(&line)
            .await
            .context("Failed to write to the accessibility bridge")?;
        self.stdin
            .flush()
            .await
            .context("Failed to flush bridge stdin")?;

        let mut response = String::new();

        let read = tokio::time::timeout(REQUEST_TIMEOUT, self.stdout.read_line(&mut response))
            .await
            .context("Accessibility bridge timed out")?
            .context("Failed to read from the accessibility bridge")?;

        if read == 0 {
            bail!("Accessibility bridge closed unexpectedly");
        }

        serde_json::from_str(&response).context("Accessibility bridge returned invalid JSON")
    }
}

pub struct AccessibilityTool {
    bridge: tokio::sync::Mutex<Option<Bridge>>,
}

impl AccessibilityTool {
    pub fn new() -> Self {
        Self {
            bridge: tokio::sync::Mutex::new(None),
        }
    }

    async fn send(&self, request: Value) -> Result<Value> {
        let mut guard = self.bridge.lock().await;
        let action = request.get("action").and_then(Value::as_str).unwrap_or("");
        // A timeout says nothing about whether an input reached the target app.
        // Replaying it after restarting the bridge can click twice or overwrite
        // a field twice, so only retry operations that are observational.
        let may_change_ui = action_may_change_ui(&request);

        for attempt in 0..2 {
            if guard.is_none() {
                match Bridge::spawn().await {
                    Ok(bridge) => *guard = Some(bridge),

                    Err(error) => {
                        if attempt == 1 {
                            return Err(error);
                        }

                        continue;
                    }
                }
            }

            let bridge = guard.as_mut().expect("bridge initialized above");

            match bridge.request(&request).await {
                Ok(response) => return Ok(response),

                Err(error) => {
                    eprintln!("[Accessibility] Bridge error: {error}; restarting");
                    *guard = None;
                    if may_change_ui {
                        return Err(error.context(format!(
                            "Accessibility `{action}` outcome is unknown; it was not retried to avoid duplicating a possible UI action"
                        )));
                    }
                }
            }
        }

        bail!("Accessibility bridge failed after restart")
    }
}

impl Default for AccessibilityTool {
    fn default() -> Self {
        Self::new()
    }
}

fn bridge_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/atspi_bridge.py")
}

fn bridge_error(response: &Value) -> Option<String> {
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        None
    } else {
        Some(
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown accessibility error")
                .to_string(),
        )
    }
}

fn pretty(response: &Value) -> String {
    serde_json::to_string_pretty(response).unwrap_or_else(|_| response.to_string())
}

#[async_trait]
impl Tool for AccessibilityTool {
    fn name(&self) -> &str {
        "accessibility"
    }

    fn description(&self) -> &str {
        "Control and read GUI applications natively through Linux AT-SPI2 \
         accessibility -- the fastest and most reliable way to drive the \
         desktop, no screenshots needed. Start with `active` to see the focused \
         window and element, then `windows`/`apps` to list them. `tree` \
         inspects one, `find` locates an element by name/role/text, and `read` \
         dumps the visible text content of an app (the native replacement for \
         screenshot+OCR). Act on returned paths with `click` (optionally \
         `action_name`), `focus`, `set_text`, or `get_text`. Some apps \
         (Chromium/Electron, or any app while toolkit-accessibility is off) \
         expose no tree; responses include `warnings` when that is the case, \
         and you should then fall back to the `computer` tool."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "active",
                        "apps",
                        "windows",
                        "tree",
                        "find",
                        "read",
                        "click",
                        "focus",
                        "set_text",
                        "get_text"
                    ],
                    "description": "Accessibility operation. `active` reports the focused window and element."
                },
                "action_name": {
                    "type": "string",
                    "description": "Optional exact AT-SPI action name for `click` (e.g. 'press', 'open'). Without it, only click/press/toggle actions are selected."
                },
                "read": {
                    "type": "boolean",
                    "description": "For `active`, also return the active window's visible text lines (like `read`)."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 200,
                    "maximum": 20000,
                    "description": "Maximum text characters returned by `read`. Defaults to 4000."
                },
                "app": {
                    "type": "integer",
                    "description": "Application index from `apps`."
                },
                "name": {
                    "type": "string",
                    "description": "Element name substring for `find`/`click`/`focus`/`set_text`."
                },
                "role": {
                    "type": "string",
                    "description": "Element role substring, e.g. 'push button', 'entry', 'link'."
                },
                "path": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Exact accessibility path returned by `find`/`tree`."
                },
                "text": {
                    "type": "string",
                    "description": "For `find`/`click`/`focus`: match the element's visible text (useful when the name is empty). For `set_text`: the text to write into the field."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Maximum results. Defaults to 20 for `find`."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 12,
                    "description": "Maximum tree depth. Defaults to 5."
                },
                "max_nodes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20000,
                    "description": "Maximum nodes to visit. Defaults to 2000 for `tree`."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let request: Value = serde_json::from_str(input)
            .context("Accessibility tool input must be a JSON object")?;

        let response = self.send(request).await?;

        if let Some(error) = bridge_error(&response) {
            return Ok(ToolResult::failure(error));
        }

        Ok(ToolResult::success(pretty(&response)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_accessibility_requests_that_must_not_be_replayed() {
        assert!(action_may_change_ui(&json!({"action":"click"})));
        assert!(action_may_change_ui(&json!({"action":"set_text"})));
        assert!(action_may_change_ui(&json!({"action":"focus"})));
        assert!(!action_may_change_ui(&json!({"action":"tree"})));
        assert!(!action_may_change_ui(&json!({"action":"read"})));
    }

    #[tokio::test]
    async fn bridge_lists_apps_when_available() {
        let available = tokio::process::Command::new("python3")
            .args(["-c", "import pyatspi"])
            .status()
            .await;

        if !matches!(available, Ok(status) if status.success()) {
            eprintln!("skipping accessibility test: pyatspi unavailable");
            return;
        }

        let tool = AccessibilityTool::new();

        let result = tool
            .execute("{\"action\":\"apps\"}")
            .await
            .expect("tool executes");

        if !result.success {
            eprintln!(
                "skipping accessibility test: {}",
                result.error.unwrap_or_default()
            );
            return;
        }

        assert!(result.output.contains("\"apps\""));
    }
}
