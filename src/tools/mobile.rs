use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::link::LinkServer;

use super::{Tool, ToolResult};

pub struct MobileTool {
    server: Arc<LinkServer>,
}

impl MobileTool {
    pub fn new(server: Arc<LinkServer>) -> Self {
        Self { server }
    }
}

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    phone_action: Option<String>,
    #[serde(default)]
    arguments: Value,
    #[serde(default)]
    risk: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[async_trait]
impl Tool for MobileTool {
    fn name(&self) -> &str {
        "mobile"
    }

    fn description(&self) -> &str {
        "List paired Android devices or invoke one structured phone action. The phone enforces local confirmation for consequential actions. Never place raw shell in arguments."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "invoke", "pairing"] },
                "device_id": { "type": "string" },
                "phone_action": { "type": "string", "description": "For example apps.launch, url.open, ui.click, timer.create, or device.status" },
                "arguments": { "type": "object" },
                "risk": { "type": "string", "enum": ["safe", "confirm"] },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 60000 }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: Input = serde_json::from_str(input)?;
        match input.action.as_str() {
            "list" => Ok(ToolResult::success(serde_json::to_string_pretty(
                &self.server.devices().await,
            )?)),
            "pairing" => Ok(ToolResult::success(serde_json::to_string_pretty(
                &self.server.pairing_payload(),
            )?)),
            "invoke" => {
                let Some(device_id) = input.device_id.filter(|value| !value.trim().is_empty())
                else {
                    return Ok(ToolResult::failure("device_id is required"));
                };
                let Some(phone_action) =
                    input.phone_action.filter(|value| !value.trim().is_empty())
                else {
                    return Ok(ToolResult::failure("phone_action is required"));
                };
                if phone_action == "shell.execute" {
                    return Ok(ToolResult::failure(
                        "Raw remote shell is disabled; use a structured phone action",
                    ));
                }
                // The laptop never grants a consequential phone action a
                // lower risk level by default. Android independently makes
                // the final confirmation decision from the action itself.
                let risk = remote_action_requires_confirmation(&phone_action)
                    .then_some("confirm".to_string())
                    .or(input.risk)
                    .unwrap_or_else(|| "confirm".to_string());
                let invocation = self
                    .server
                    .invoke(
                        &device_id,
                        phone_action,
                        input.arguments,
                        risk,
                        input.timeout_ms.unwrap_or(30_000).clamp(1_000, 60_000),
                    )
                    .await;
                match invocation {
                    Ok(result) if result.success => Ok(ToolResult::success(
                        json!({
                            "completed": true,
                            "invocation_id": result.invocation_id,
                            "device_id": device_id,
                            "output": result.output,
                        })
                        .to_string(),
                    )),
                    Ok(result) => Ok(ToolResult::failure(format!(
                        "Phone action {} failed: {}",
                        result.invocation_id,
                        result
                            .error
                            .unwrap_or_else(|| "phone reported no details".to_string())
                    ))),
                    Err(error) => Ok(ToolResult::failure(error.to_string())),
                }
            }
            _ => Ok(ToolResult::failure("Unknown mobile action")),
        }
    }
}

fn remote_action_requires_confirmation(action: &str) -> bool {
    let action = action.to_ascii_lowercase();
    [
        "delete",
        "remove",
        "uninstall",
        "install",
        "purchase",
        "payment",
        "pay",
        "send",
        "message",
        "share",
        "upload",
        "call",
        "sms",
        "clipboard",
        "reset",
    ]
    .iter()
    .any(|needle| action.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::remote_action_requires_confirmation;

    #[test]
    fn consequential_actions_cannot_be_marked_safe_by_the_laptop() {
        assert!(remote_action_requires_confirmation("message.send"));
        assert!(remote_action_requires_confirmation("apps.uninstall"));
        assert!(!remote_action_requires_confirmation("apps.launch"));
    }
}
