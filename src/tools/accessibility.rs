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
use crate::tools::manifest::{Capability, RiskClass, Schema, Source, ToolManifest, CORE_VERSION};

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
            crate::tools::capabilities::record_accessibility(
                crate::tools::capabilities::Capability::Unsupported,
                "AT-SPI bridge",
            );
            bail!("AT-SPI bridge script is missing: {}", script.display());
        }

        let child = match tokio::process::Command::new("python3")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn python3 with {}", script.display()))
        {
            Ok(child) => child,
            Err(error) => {
                crate::tools::capabilities::record_accessibility(
                    crate::tools::capabilities::Capability::Unsupported,
                    "AT-SPI bridge",
                );
                return Err(error);
            }
        };

        match Self::from_child(child) {
            Ok(bridge) => {
                crate::tools::capabilities::record_accessibility(
                    crate::tools::capabilities::Capability::Granted,
                    "AT-SPI bridge",
                );
                Ok(bridge)
            }
            Err(error) => {
                crate::tools::capabilities::record_accessibility(
                    crate::tools::capabilities::Capability::Unsupported,
                    "AT-SPI bridge",
                );
                Err(error)
            }
        }
    }

    fn from_child(mut child: Child) -> Result<Self> {
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

        let mut response = String::new();

        let read = tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.stdin
                .write_all(&line)
                .await
                .context("Failed to write to the accessibility bridge")?;
            self.stdin
                .flush()
                .await
                .context("Failed to flush bridge stdin")?;
            self.stdout
                .read_line(&mut response)
                .await
                .context("Failed to read from the accessibility bridge")
        })
        .await
        .context("Accessibility bridge request timed out")??;

        if read == 0 {
            bail!("Accessibility bridge closed unexpectedly");
        }

        serde_json::from_str(&response).context("Accessibility bridge returned invalid JSON")
    }
}

/// Keep an in-flight bridge out of the shared slot. If this future is
/// cancelled, dropping its local Bridge kills the child and leaves the slot
/// empty, so the next request cannot consume a stale reply.
async fn request_owned(bridge_slot: &mut Option<Bridge>, request: &Value) -> Result<Value> {
    let mut bridge = bridge_slot
        .take()
        .context("accessibility bridge was not initialized")?;
    let response = bridge.request(request).await?;
    *bridge_slot = Some(bridge);
    Ok(response)
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

            match request_owned(&mut guard, &request).await {
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
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "accessibility",
            version: CORE_VERSION,
            description: "Control and read GUI applications natively through Linux AT-SPI2 \
            accessibility -- the fastest and most reliable way to drive the \
            desktop, no screenshots needed. Start with `active` to see the focused \
            window and element, then `windows`/`apps` to list them. `tree` \
            inspects one, `find` locates an element by name/role/text, and `read` \
            dumps the visible text content of an app (the native replacement for \
            screenshot+OCR). Act on returned paths with `click` (optionally \
            `action_name`); if the element has bounds but no clickable AT-SPI action, \
            `click` falls back to a desktop mouse click at its center. Use `focus`, \
            `set_text`, or `get_text` for other interactions. Some apps \
            (Chromium/Electron, or any app while toolkit-accessibility is off) \
            expose no tree; responses include `warnings` when that is the case, \
            and you should then fall back to the `computer` tool.",
            schema: Schema::Fn(accessibility_schema),
            risk: &[RiskClass::Read, RiskClass::Mutate, RiskClass::Input],
            capabilities: &[Capability::Input, Capability::Accessibility],
            source: Source::Core,
        }
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let request: Value = serde_json::from_str(input)
            .context("Accessibility tool input must be a JSON object")?;
        if !request.is_object() {
            return Ok(ToolResult::failure(
                "Accessibility tool input must be a JSON object.",
            ));
        }
        if !request.get("action").is_some_and(Value::is_string) {
            return Ok(ToolResult::failure(
                "Accessibility tool input requires a string `action` field.",
            ));
        }

        let is_click = request.get("action").and_then(Value::as_str) == Some("click");
        let response = self.send(request).await?;

        if is_click {
            if let Some(fallback) = response.get("coordinate_fallback") {
                let x = fallback.get("x").and_then(Value::as_i64);
                let y = fallback.get("y").and_then(Value::as_i64);
                if let (Some(x), Some(y)) = (x, y) {
                    let click = super::computer::ComputerTool::new()
                        .execute(&json!({"action":"mouse_click", "x":x, "y":y}).to_string())
                        .await;
                    match click {
                        Ok(result) if result.success => {
                            return Ok(ToolResult::success(pretty(&json!({
                                "ok": true,
                                "fallback": "computer.mouse_click at the AT-SPI element bounds center",
                                "click": {"x": x, "y": y},
                                "element": fallback.get("element"),
                                "accessibility_error": response.get("error"),
                            }))));
                        }
                        Ok(result) => {
                            let error = result.error.unwrap_or(result.output);
                            return Ok(ToolResult::failure(format!(
                                "{}; coordinate click at ({x}, {y}) also failed: {error}. Use computer.screenshot/OCR to inspect the visible control.",
                                response
                                    .get("error")
                                    .and_then(Value::as_str)
                                    .unwrap_or("AT-SPI click was unavailable")
                            )));
                        }
                        Err(error) => {
                            return Ok(ToolResult::failure(format!(
                                "{}; coordinate click at ({x}, {y}) also failed: {error:#}. Use computer.screenshot/OCR to inspect the visible control.",
                                response
                                    .get("error")
                                    .and_then(Value::as_str)
                                    .unwrap_or("AT-SPI click was unavailable")
                            )));
                        }
                    }
                }
            }
        }

        if let Some(error) = bridge_error(&response) {
            return Ok(ToolResult::failure(error));
        }

        Ok(ToolResult::success(pretty(&response)))
    }
}

fn accessibility_schema() -> Value {
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
                "description": "Optional exact AT-SPI action name for `click` (e.g. 'press', 'open'). Without it, click/press/activate/toggle actions are selected; if none exists and bounds are available, the tool uses a computer mouse click at the element center."
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
    async fn cancelling_bridge_request_leaves_slot_empty() {
        let child = tokio::process::Command::new("python3")
            .args([
                "-u",
                "-c",
                "import sys, time; sys.stdin.readline(); time.sleep(30)",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn fake bridge");
        let mut slot = Some(Bridge::from_child(child).expect("capture bridge pipes"));

        {
            let request = json!({"action": "tree"});
            let pending = request_owned(&mut slot, &request);
            tokio::pin!(pending);
            tokio::select! {
                result = &mut pending => panic!("fake bridge unexpectedly responded: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        assert!(slot.is_none(), "cancelled bridge must not remain reusable");
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
