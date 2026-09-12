use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
        }
    }

    pub fn as_agent_message(&self) -> String {
        if self.success {
            format!("Tool execution succeeded.\nOutput:\n{}", self.output)
        } else {
            format!(
                "Tool execution failed.\nError:\n{}",
                self.error.as_deref().unwrap_or("Unknown error")
            )
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    /// JSON Schema describing the arguments this tool accepts.
    /// Defaults to a permissive empty object so existing tools
    /// keep compiling without overrides.
    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult>;
}
