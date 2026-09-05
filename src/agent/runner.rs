use anyhow::Result;

use crate::{model::openrouter::OpenRouterClient, tools::{ToolRegistry, ToolResult}, types::Message};

pub struct Agent {
    client: OpenRouterClient,
    model: String,
    tools: ToolRegistry,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(client: OpenRouterClient, model: String, tools: ToolRegistry) -> Self {
        let system_prompt = build_system_prompt(&tools);

        Self {
            client,
            model,
            tools,
            messages: vec![Message {
                role: "system".to_string(),
                content: system_prompt,
            }],
        }
    }

    pub async fn handle(&mut self, user_input: String) -> Result<String> {
        self.messages.push(Message {
            role: "user".to_string(),
            content: user_input,
        });

        loop {
            let response = self.client.chat(&self.model, &self.messages).await?;

            if let Some((tool_name, tool_input)) = parse_tool_call(&response.content) {
                let tool = self.tools.get(&tool_name);

                match tool {
                    Some(tool) => {
                        println!("\n[Hyusk] Running {}: {}\n", tool_name, tool_input);

                        let result = match tool.execute(&tool_input).await {
                            Ok(result) => result,
                        
                            Err(error) => ToolResult::failure(
                                format!("Tool execution error: {}", error)
                            ),
                        };

                        self.messages.push(response);

                        self.messages.push(Message {
                            role: "user".to_string(),

                            content: format!("Tool result:\n{}", result.as_agent_message()),
                        });

                        continue;
                    }

                    None => {
                        self.messages.push(Message {
                            role: "user".to_string(),

                            content: format!("Tool '{}' does not exist.", tool_name),
                        });

                        continue;
                    }
                }
            }

            self.messages.push(response.clone());

            return Ok(response.content);
        }
    }
}

fn build_system_prompt(tools: &ToolRegistry) -> String {
    format!(
        r#"
You are Hyusk, a local AI agent.

You have access to these tools:

{}

When using a tool, respond ONLY with valid JSON:

{{
  "tool": "TOOL_NAME",
  "input": "TOOL_INPUT"
}}

Do not wrap tool calls in markdown.

When you do not need a tool,
respond normally.
"#,
        tools.descriptions()
    )
}

fn parse_tool_call(content: &str) -> Option<(String, String)> {
    #[derive(serde::Deserialize)]
    struct ToolCall {
        tool: String,
        input: String,
    }

    serde_json::from_str::<ToolCall>(content)
        .ok()
        .map(|call| (call.tool, call.input))
}
