use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::types::Message;

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

pub struct OpenRouterClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl OpenRouterClient {
    pub fn new(
        api_key: String,
        base_url: String,
    ) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url,
        }
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: &[Message],
    ) -> Result<Message> {
        let request = ChatRequest {
            model: model.to_string(),
            messages: messages.to_vec(),
        };

        let response = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url
            ))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key),
            )
            .json(&request)
            .send()
            .await?
            .error_for_status()?;

        let response: ChatResponse = response
            .json()
            .await
            .context("Failed to parse model response")?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .context("Model returned no choices")?;

        Ok(choice.message)
    }
}