use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::link::LinkServer;

use super::{Tool, ToolResult};
use crate::tools::manifest::{RiskClass, Schema, Source, ToolManifest, CORE_VERSION};

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
    timeout_ms: Option<u64>,
}

#[async_trait]
impl Tool for MobileTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "mobile",
            version: CORE_VERSION,
            description: "List paired Android devices or invoke one structured phone action. The phone enforces local confirmation for consequential actions. Never place raw shell in arguments.",
            schema: Schema::Fn(mobile_schema),
            risk: &[RiskClass::Read, RiskClass::Mutate, RiskClass::Network],
            capabilities: &[],
            source: Source::Core,
        }
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: Input = serde_json::from_str(input)?;
        match input.action.as_str() {
            "list" => Ok(ToolResult::success(serde_json::to_string_pretty(
                &self.server.devices().await,
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
                // Ordinary phone actions should not cause a redundant laptop
                // approval. The phone still applies its own action-level check.
                let risk = if remote_action_requires_confirmation(&phone_action) {
                    "confirm"
                } else {
                    "safe"
                };
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

fn mobile_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["list", "invoke"] },
            "device_id": { "type": "string" },
            "phone_action": { "type": "string", "description": "For example apps.launch, url.open, ui.click, timer.create, or device.status" },
            "arguments": { "type": "object" },
            "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 60000 }
        },
        "required": ["action"]
    })
}

fn remote_action_requires_confirmation(action: &str) -> bool {
    crate::tools::safety::phone_action_requires_confirmation(action)
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
