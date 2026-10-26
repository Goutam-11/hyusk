//! Provider-independent invariants for OpenAI-compatible messages and tool batches.

use crate::types::Message;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;

pub(super) fn validate_assistant_message(message: &Message) -> Result<()> {
    if message.role != "assistant" {
        return Err(anyhow!(
            "Model response has unexpected role '{}', expected assistant",
            message.role
        ));
    }

    let Some(calls) = message.tool_calls.as_ref() else {
        return Ok(());
    };
    let mut ids = std::collections::HashSet::with_capacity(calls.len());
    for call in calls {
        if call.kind != "function" {
            return Err(anyhow!("Unsupported tool-call type '{}'", call.kind));
        }
        if call.id.trim().is_empty() {
            return Err(anyhow!("Model returned a tool call without an ID"));
        }
        if !ids.insert(call.id.as_str()) {
            return Err(anyhow!(
                "Model returned duplicate tool-call ID '{}'",
                call.id
            ));
        }
        if call.function.name.trim().is_empty() {
            return Err(anyhow!("Tool call '{}' has no function name", call.id));
        }
        let arguments: Value = serde_json::from_str(&call.function.arguments)
            .with_context(|| format!("Tool call '{}' has invalid JSON arguments", call.id))?;
        if !arguments.is_object() {
            return Err(anyhow!(
                "Tool call '{}' arguments must be a JSON object",
                call.id
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_conversation(messages: &[Message]) -> Result<()> {
    let mut pending_tool_ids = std::collections::HashSet::new();

    for message in messages {
        match message.role.as_str() {
            "system" | "user" | "assistant" => {
                if !pending_tool_ids.is_empty() {
                    return Err(anyhow!(
                        "Conversation ended an assistant tool-call batch before all tool results arrived"
                    ));
                }
                if message.tool_call_id.is_some() {
                    return Err(anyhow!(
                        "A '{}' message cannot have tool_call_id",
                        message.role
                    ));
                }
                if message.role == "assistant" {
                    validate_assistant_message(message)?;
                    if let Some(calls) = &message.tool_calls {
                        if calls.is_empty() {
                            return Err(anyhow!("Assistant tool_calls cannot be empty"));
                        }
                        pending_tool_ids.extend(calls.iter().map(|call| call.id.as_str()));
                    }
                } else if message.tool_calls.is_some() {
                    return Err(anyhow!(
                        "A '{}' message cannot contain tool_calls",
                        message.role
                    ));
                }
            }
            "tool" => {
                let id = message
                    .tool_call_id
                    .as_deref()
                    .filter(|id| !id.trim().is_empty())
                    .context("Tool result is missing tool_call_id")?;
                if message.tool_calls.is_some() {
                    return Err(anyhow!("A tool result cannot contain tool_calls"));
                }
                if !pending_tool_ids.remove(id) {
                    return Err(anyhow!(
                        "Tool result references unknown or duplicate tool-call ID '{id}'"
                    ));
                }
            }
            role => return Err(anyhow!("Unsupported conversation role '{role}'")),
        }
    }

    if !pending_tool_ids.is_empty() {
        return Err(anyhow!(
            "Conversation ended before all assistant tool calls received results"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_assistant_message, validate_conversation};
    use crate::types::{message::ToolCall, Message};

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: crate::types::message::FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    #[test]
    fn accepts_well_formed_assistant_tool_call() {
        let mut message = Message::assistant("");
        message.tool_calls = Some(vec![tool_call("call-1", "shell", r#"{"cmd":"pwd"}"#)]);

        assert!(validate_assistant_message(&message).is_ok());
    }

    #[test]
    fn rejects_tool_calls_with_missing_or_duplicate_ids() {
        let mut missing = Message::assistant("");
        missing.tool_calls = Some(vec![tool_call(" ", "shell", "{}")]);
        assert!(validate_assistant_message(&missing).is_err());

        let mut duplicate = Message::assistant("");
        duplicate.tool_calls = Some(vec![
            tool_call("same", "shell", "{}"),
            tool_call("same", "computer", "{}"),
        ]);
        assert!(validate_assistant_message(&duplicate).is_err());
    }

    #[test]
    fn rejects_malformed_or_non_object_tool_arguments() {
        for arguments in ["{", "null", "[]"] {
            let mut message = Message::assistant("");
            message.tool_calls = Some(vec![tool_call("call-1", "shell", arguments)]);
            assert!(validate_assistant_message(&message).is_err(), "{arguments}");
        }
    }

    #[test]
    fn accepts_complete_assistant_tool_result_sequence() {
        let mut assistant = Message::assistant("");
        assistant.tool_calls = Some(vec![
            tool_call("call-1", "shell", "{}"),
            tool_call("call-2", "computer", "{}"),
        ]);
        let messages = vec![
            Message::system("tools"),
            Message::user("do it"),
            assistant,
            Message::tool_result("call-2", "second"),
            Message::tool_result("call-1", "first"),
            Message::assistant("done"),
        ];

        assert!(validate_conversation(&messages).is_ok());
    }

    #[test]
    fn rejects_missing_or_orphaned_tool_results() {
        let mut assistant = Message::assistant("");
        assistant.tool_calls = Some(vec![tool_call("call-1", "shell", "{}")]);

        assert!(validate_conversation(&[assistant.clone()]).is_err());
        assert!(validate_conversation(&[Message::tool_result("missing", "done")]).is_err());
        assert!(validate_conversation(&[
            assistant,
            Message::tool_result("call-1", "done"),
            Message::tool_result("call-1", "duplicate"),
        ])
        .is_err());
    }
}
