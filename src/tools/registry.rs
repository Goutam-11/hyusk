use std::collections::HashMap;
use std::sync::Arc;

use super::Tool;

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        let name = tool.name().to_string();

        self.tools.insert(name, Arc::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        let mut tools: Vec<_> = self.tools.values().cloned().collect();
        tools.sort_by(|left, right| left.name().cmp(right.name()));
        tools.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;

    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "test tool"
        }

        async fn execute(&self, _input: &str) -> Result<super::super::ToolResult> {
            Ok(super::super::ToolResult::success("ok"))
        }
    }

    #[test]
    fn iteration_is_sorted_by_tool_name() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool("zeta"));
        registry.register(NamedTool("alpha"));
        registry.register(NamedTool("middle"));

        let names: Vec<_> = registry
            .iter()
            .map(|tool| tool.name().to_string())
            .collect();
        assert_eq!(names, ["alpha", "middle", "zeta"]);
    }
}
