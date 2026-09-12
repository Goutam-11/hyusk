use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use crate::types::Message;

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
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
