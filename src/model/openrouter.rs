use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use crate::types::{
    message::{FunctionCall, ToolCall},
    Message,
};

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

/// Carries an HTTP error response through the retry loop so the
/// classifier can see the status and `Retry-After` header.
#[derive(Debug)]
struct HttpError {
    status: Option<StatusCode>,
    retry_after: Option<Duration>,
    body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(
                f,
                "HTTP {} {}: {}",
                s.as_u16(),
                status_label(Some(s)),
                self.body
            ),
            None => write!(f, "network error: {}", self.body),
        }
    }
}

impl std::error::Error for HttpError {}

pub struct OpenRouterClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl OpenRouterClient {
    pub fn new(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url,
        }
    }

    /// Issue a chat completion with automatic retries on
    /// transient failures (HTTP 429, 5xx, network errors, and
    /// malformed responses). Honors `Retry-After` when present.
    ///
    /// Up to `MAX_RETRIES + 1` attempts are made, with
    /// `BASE_BACKOFF` → 2x → 4x backoff (capped at `MAX_BACKOFF`).
    /// Non-transient errors (4xx other than 408/429) fail fast.
    ///
    /// `tools` is a list of OpenAI-compatible tool specs in the form:
    ///
    /// ```json
    /// {
    ///   "type": "function",
    ///   "function": {
    ///     "name": "...",
    ///     "description": "...",
    ///     "parameters": { ... JSON schema ... }
    ///   }
    /// }
    /// ```
    ///
    /// Pass an empty slice to disable function calling.
    pub async fn chat(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[Value],
    ) -> Result<Message> {
        let request = ChatRequest {
            model: model.to_string(),
            messages: messages.to_vec(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            stream: None,
        };

        let max_attempts = MAX_RETRIES + 1;
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;

            match self.send_once(&request).await {
                Ok(message) => return Ok(message),

                Err(error) => {
                    let transient = classify_error(&error);

                    if !transient {
                        return Err(error);
                    }

                    if attempt >= max_attempts {
                        return Err(anyhow!(
                            "Model request failed after {} attempt(s): {}",
                            attempt,
                            error
                        ));
                    }

                    let retry_after = error
                        .downcast_ref::<HttpError>()
                        .and_then(|h| h.retry_after);
                    let delay = backoff_for(attempt, retry_after);

                    eprintln!(
                        "[Hyusk] Model request failed ({}); retrying in {:?} (attempt {}/{})",
                        first_line(&format!("{:#}", error)),
                        delay,
                        attempt,
                        max_attempts
                    );

                    sleep(delay).await;
                }
            }
        }
    }

    async fn send_once(&self, request: &ChatRequest) -> Result<Message> {
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
            .map_err(|e| {
                anyhow!(HttpError {
                    status: None,
                    retry_after: None,
                    body: e.to_string(),
                })
            })
            .context("Failed to reach model provider")?;

        let status = response.status();

        if !status.is_success() {
            let retry_after = parse_retry_after(response.headers());
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(HttpError {
                status: Some(status),
                retry_after,
                body: truncate(&body, 240),
            }));
        }

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

    /// Stream a chat completion, invoking `on_delta` for each text delta.
    ///
    /// Returns the assembled assistant message (text plus any streamed tool
    /// calls). This is a single attempt with no retries: it is meant to make
    /// replies feel fast, and the caller falls back to [`Self::chat`] when the
    /// provider rejects streaming.
    pub async fn chat_stream<F>(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[Value],
        on_delta: &mut F,
    ) -> Result<Message>
    where
        F: FnMut(&str),
    {
        let request = ChatRequest {
            model: model.to_string(),
            messages: messages.to_vec(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            stream: Some(true),
        };

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                anyhow!(HttpError {
                    status: None,
                    retry_after: None,
                    body: error.to_string(),
                })
            })
            .context("Failed to reach model provider")?;

        let status = response.status();

        if !status.is_success() {
            let retry_after = parse_retry_after(response.headers());
            let body = response.text().await.unwrap_or_default();

            return Err(anyhow!(HttpError {
                status: Some(status),
                retry_after,
                body: truncate(&body, 240),
            }));
        }

        parse_sse_stream(response, on_delta).await
    }
}

async fn parse_sse_stream<F>(response: reqwest::Response, on_delta: &mut F) -> Result<Message>
where
    F: FnMut(&str),
{
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut content = String::new();
    let mut tool_calls: std::collections::BTreeMap<usize, ToolCall> =
        std::collections::BTreeMap::new();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Model stream error")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(position) = buffer.find('\n') {
            let line = buffer[..position].trim_end_matches('\r').to_string();

            buffer.drain(..position + 1);

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };

            let data = data.trim();

            if data == "[DONE]" {
                done = true;
                break;
            }

            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };

            let Some(delta) = value.pointer("/choices/0/delta") else {
                continue;
            };

            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push_str(text);
                    on_delta(text);
                }
            }

            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;

                    let entry = tool_calls.entry(index).or_insert_with(|| ToolCall {
                        id: String::new(),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: String::new(),
                            arguments: String::new(),
                        },
                    });

                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        if !id.is_empty() {
                            entry.id = id.to_string();
                        }
                    }

                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            entry.function.name.push_str(name);
                        }

                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                            entry.function.arguments.push_str(arguments);
                        }
                    }
                }
            }
        }

        if done {
            break;
        }
    }

    let mut message = Message::assistant(content.clone());

    if !tool_calls.is_empty() {
        if content.is_empty() {
            message.content = None;
        }

        message.tool_calls = Some(tool_calls.into_values().collect());
    }

    Ok(message)
}

fn backoff_for(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d.min(MAX_BACKOFF);
    }

    let exp = 1u32 << (attempt - 1).min(10);
    let base_ms = BASE_BACKOFF.as_millis() as u64;
    let candidate = Duration::from_millis(base_ms.saturating_mul(exp as u64));
    candidate.min(MAX_BACKOFF)
}

fn classify_error(error: &anyhow::Error) -> bool {
    if let Some(http) = error.downcast_ref::<HttpError>() {
        return match http.status {
            Some(s) => is_transient_status(s),
            None => true, // network error
        };
    }

    if error.downcast_ref::<reqwest::Error>().is_some() {
        return true;
    }

    let msg = format!("{:#}", error).to_ascii_lowercase();
    msg.contains("failed to reach model provider")
        || msg.contains("failed to parse model response")
        || msg.contains("model returned no choices")
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

fn status_label(status: Option<StatusCode>) -> &'static str {
    match status {
        Some(s) => s.canonical_reason().unwrap_or("error"),
        None => "network error",
    }
}

/// Parse a `Retry-After` header. We handle the seconds form
/// (the one OpenRouter and Cloudflare send); HTTP-dates are ignored.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds: u64 = value.trim().parse().ok()?;
    Some(Duration::from_secs(seconds))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}
