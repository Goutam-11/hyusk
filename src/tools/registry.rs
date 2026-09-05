use std::collections::HashMap;
use std::sync::Arc;

use super::Tool;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T>(
        &mut self,
        tool: T,
    )
    where
        T: Tool + 'static,
    {
        let name = tool.name().to_string();

        self.tools.insert(
            name,
            Arc::new(tool),
        );
    }

    pub fn get(
        &self,
        name: &str,
    ) -> Option<Arc<dyn Tool>> {
        self.tools
            .get(name)
            .cloned()
    }

    pub fn descriptions(&self) -> String {
        self.tools
            .values()
            .map(|tool| {
                format!(
                    "- {}: {}",
                    tool.name(),
                    tool.description()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}