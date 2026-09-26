use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Tool, ToolResult};

const SERVICE: &str = "org.hyusk.Shell";
const PATH: &str = "/org/hyusk/Shell";
const INTERFACE: &str = "org.hyusk.Shell";

/// Native window management through the Hyusk GNOME Shell extension.
///
/// AT-SPI cannot reliably list or raise windows on Wayland, so the extension
/// exposes a small D-Bus interface the agent can call. This is what makes
/// "switch to <app>" instant and reliable.
pub struct WindowTool;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum WindowAction {
    List,
    Active,
    Activate {
        query: String,
    },
    Close {
        #[serde(default)]
        query: String,
    },
}

async fn connection() -> Result<zbus::Connection> {
    zbus::Connection::session()
        .await
        .context("failed to connect to the session D-Bus")
}

async fn call(
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<String> {
    let connection = connection().await?;

    let reply = connection
        .call_method(Some(SERVICE), PATH, Some(INTERFACE), method, body)
        .await
        .with_context(|| {
            format!(
                "{SERVICE}.{method} failed. Is the Hyusk GNOME extension \
                 (hyusk@hyusk.local) installed and enabled? On Wayland it is \
                 the only reliable way to switch windows."
            )
        })?;

    reply
        .body()
        .deserialize::<String>()
        .context("unexpected reply from the Hyusk shell extension")
}

impl WindowTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WindowTool {
    fn name(&self) -> &str {
        "window"
    }

    fn description(&self) -> &str {
        "List, inspect, switch, and close desktop windows natively on GNOME \
         through the Hyusk shell extension. Use `list` for open windows (app, \
         title, stable ID, bounds, active flag), `active` for the focused window, \
         `activate` with a `query` (prefer `id:<number>` from `list`; app name \
         or title substring also works) to raise a window, and `close` \
         (empty `query` closes the focused window). This is the reliable way to \
         manage windows on Wayland; prefer it over blind alt+tab or \
         accessibility, which cannot raise windows."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "active", "activate", "close"],
                    "description": "Window operation."
                },
                "query": {
                    "type": "string",
                    "description": "Prefer an exact `id:<number>` from `list` for `activate`/`close`; app name or window-title substring also works. Empty closes the focused window."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let action: WindowAction = serde_json::from_str(input)
            .context("Window tool input must be JSON with an `action` field")?;

        match action {
            WindowAction::List => {
                let raw = match call("ListWindows", &()).await {
                    Ok(raw) => raw,
                    Err(error) => return Ok(ToolResult::failure(error.to_string())),
                };

                let windows: Vec<Value> = serde_json::from_str(&raw).unwrap_or_default();

                if windows.is_empty() {
                    return Ok(ToolResult::success("No windows reported."));
                }

                let mut lines = vec![format!("Windows ({}):", windows.len())];

                for window in &windows {
                    let active = window
                        .get("active")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let bounds = window.get("bounds").and_then(|value| {
                        Some(format!(
                            " at {},{} {}x{}",
                            value.get("x")?.as_i64()?,
                            value.get("y")?.as_i64()?,
                            value.get("width")?.as_u64()?,
                            value.get("height")?.as_u64()?,
                        ))
                    });

                    lines.push(format!(
                        "{}[id:{}] {} — {}{}{}",
                        if active { "* " } else { "  " },
                        window
                            .get("id")
                            .and_then(Value::as_u64)
                            .map_or_else(|| "?".to_string(), |id| id.to_string()),
                        window.get("app").and_then(Value::as_str).unwrap_or(""),
                        window.get("title").and_then(Value::as_str).unwrap_or(""),
                        bounds.unwrap_or_default(),
                        if window.get("minimized").and_then(Value::as_bool) == Some(true) {
                            " (minimized)"
                        } else {
                            ""
                        }
                    ));
                }

                Ok(ToolResult::success(lines.join("\n")))
            }

            WindowAction::Active => {
                let raw = match call("ActiveWindow", &()).await {
                    Ok(raw) => raw,
                    Err(error) => return Ok(ToolResult::failure(error.to_string())),
                };

                Ok(ToolResult::success(raw))
            }

            WindowAction::Activate { query } => {
                let query = query.trim().to_string();

                if query.is_empty() {
                    return Ok(ToolResult::failure(
                        "`activate` requires a non-empty `query`.",
                    ));
                }

                let raw = match call("ActivateWindow", &query).await {
                    Ok(raw) => raw,
                    Err(error) => return Ok(ToolResult::failure(error.to_string())),
                };

                let value: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw.clone()));

                if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    let title = value.get("title").and_then(Value::as_str).unwrap_or(&query);

                    Ok(ToolResult::success(format!("Activated window: {title}")))
                } else {
                    let error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("no window matched the query");

                    Ok(ToolResult::failure(error.to_string()))
                }
            }

            WindowAction::Close { query } => {
                let query = query.trim().to_string();

                let raw = match call("CloseWindow", &query).await {
                    Ok(raw) => raw,
                    Err(error) => return Ok(ToolResult::failure(error.to_string())),
                };

                let value: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw.clone()));

                if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    let title = value
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("window");

                    Ok(ToolResult::success(format!("Closed window: {title}")))
                } else {
                    let error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("no window matched the query");

                    Ok(ToolResult::failure(error.to_string()))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_never_errors_without_the_extension() {
        let tool = WindowTool::new();

        // Without the extension this returns a failure ToolResult, not an Err;
        // with it installed on the dev machine it succeeds. Either is fine.
        let result = tool
            .execute("{\"action\":\"list\"}")
            .await
            .expect("tool executes");

        let _ = result;
    }
}
