use super::manifest::ToolManifest;
use anyhow::Result;
use async_trait::async_trait;

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
    /// The tool's declaration (T1): name, version, description, schema, risk
    /// classes, capability needs, and source.
    ///
    /// There is no default, so a tool cannot compile without one. The registry
    /// checks it when the tool registers, and the name, description, and schema
    /// the model sees are read back from here rather than declared twice.
    fn manifest(&self) -> ToolManifest;

    async fn execute(&self, input: &str) -> Result<ToolResult>;

    fn name(&self) -> &str {
        self.manifest().name
    }

    fn description(&self) -> &str {
        self.manifest().description
    }

    /// Whether the registry should currently offer this tool to the model and
    /// allow a call.
    ///
    /// A compiled tool is always available. An external tool returns false
    /// when its server leaves the configuration (T3), so the operator can
    /// remove a server and the next turn stops offering its tools without a
    /// restart. The manifest stays registered, so conformance still covers it.
    fn is_available(&self) -> bool {
        true
    }
}
