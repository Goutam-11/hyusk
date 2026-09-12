use serde::{Deserialize, Serialize};

/// A chat-completions message.
///
/// `content` is ordinary text or a multimodal parts array. For assistant
/// messages that include function calls, `tool_calls` carries the structured
/// calls. For tool-result messages, `tool_call_id` matches the originating
/// assistant `tool_calls[i].id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// OpenAI-compatible message content.
///
/// A plain string is the common case. An array of parts carries text plus
/// image URLs for vision-capable models.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A single function call emitted by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,

    #[serde(rename = "type")]
    pub kind: String, // always "function" in OpenAI-compatible APIs

    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,

    /// Arguments as a JSON-encoded string. Most OpenAI-compatible
    /// providers (including OpenRouter) emit the arguments object
    /// as a JSON string that the client must parse.
    pub arguments: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// A user message with text plus one image data URL.
    pub fn user_with_image(text: impl Into<String>, image_url: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text { text: text.into() },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: image_url.into(),
                        detail: None,
                    },
                },
            ])),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Plain text content, flattened from any multimodal parts.
    pub fn text(&self) -> String {
        match &self.content {
            Some(MessageContent::Text(text)) => text.clone(),

            Some(MessageContent::Parts(parts)) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),

            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_still_serializes_as_string() {
        let message = Message::user("hello");
        let value = serde_json::to_value(&message).expect("serialize");

        assert_eq!(value["content"], "hello");
    }

    #[test]
    fn user_with_image_serializes_multimodal_parts() {
        let message = Message::user_with_image("hello", "data:image/png;base64,AAAA");
        let value = serde_json::to_value(&message).expect("serialize");
        let content = value["content"].as_array().expect("parts array");

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "hello");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }
}
