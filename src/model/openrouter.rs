use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use crate::types::Message;

#[path = "openrouter/protocol.rs"]
mod protocol;
#[path = "openrouter/stream.rs"]
mod stream;

use protocol::{validate_assistant_message, validate_conversation};
use stream::{parse_sse_stream, StreamLimits};

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// Ceiling on connecting and receiving response headers. A streamed reply is
/// bounded by its own idle and total limits instead, because a long answer that
/// keeps producing tokens must not be cut mid-reply.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(150);

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

    #[serde(default)]
    finish_reason: Option<String>,
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

#[derive(Clone)]
pub struct OpenRouterClient {
    client: Client,
    api_key: String,
    base_url: String,
    stream_limits: StreamLimits,
}

impl OpenRouterClient {
    pub fn new(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                // No client-wide total timeout: it would also cap a streamed
                // reply. Non-streaming requests set their own per-request
                // timeout, and streams use the idle/total limits below.
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_key,
            base_url,
            stream_limits: StreamLimits::from_env(),
        }
    }

    /// Fetch a provider's OpenAI-compatible model catalog. OpenRouter and the
    /// OpenAI API both expose this at `/models`.
    pub async fn list_models(&self) -> Result<Vec<(String, String)>> {
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .timeout(REQUEST_TIMEOUT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch model catalog")?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .context("Invalid model catalog response")?;
        if !status.is_success() {
            return Err(anyhow!("Model catalog request failed with HTTP {status}"));
        }
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?.to_string();
                let label = item
                    .get("name")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string();
                Some((id, label))
            })
            .collect())
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
        validate_conversation(messages)?;
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
            .timeout(REQUEST_TIMEOUT)
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

        let mut message = choice.message;

        validate_assistant_message(&message)?;

        if choice
            .finish_reason
            .as_deref()
            .is_some_and(is_truncated_finish_reason)
        {
            return Err(anyhow!("Model response truncated (finish_reason=length)"));
        }

        let has_tool_calls = message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty());
        if message.text().trim().is_empty() && !has_tool_calls {
            if let Some(refusal) = message.refusal.take() {
                return Ok(Message::assistant(refusal));
            }
            return Err(anyhow!("Model returned an empty response"));
        }

        Ok(message)
    }

    /// Stream a chat completion, invoking `on_delta` for each text delta.
    ///
    /// Returns the assembled assistant message (text plus any streamed tool
    /// calls). Transient connection/HTTP errors are retried only before a
    /// successful response starts. Once SSE decoding begins, never replay an
    /// incomplete stream automatically. Explicit streaming-mode rejection can
    /// be retried by the caller using [`Self::chat`].
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
        validate_conversation(messages)?;
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

        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            // Connecting and receiving response headers stays bounded by the
            // request timeout; the body is then governed by the stream limits.
            let result = tokio::time::timeout(REQUEST_TIMEOUT, self.open_stream(&request)).await;
            let error = match result {
                Ok(Ok(response)) => break response,
                Ok(Err(error)) => error,
                Err(_) => HttpError {
                    status: None,
                    retry_after: None,
                    body: format!("no response headers after {}s", REQUEST_TIMEOUT.as_secs()),
                },
            };
            if attempt > MAX_RETRIES
                || error
                    .status
                    .is_some_and(|status| !is_transient_status(status))
            {
                return Err(anyhow!(error));
            }
            let delay = backoff_for(attempt, error.retry_after);
            eprintln!(
                "[Hyusk] Stream connection failed; retrying in {delay:?} (attempt {attempt}/{})",
                MAX_RETRIES + 1
            );
            sleep(delay).await;
        };

        parse_sse_stream(response, on_delta, self.stream_limits).await
    }

    /// Send the streaming request and return a success response, or the error
    /// the retry loop classifies. Bounded by the caller so a provider that
    /// accepts the connection and never answers cannot hold a turn open.
    async fn open_stream(
        &self,
        request: &ChatRequest,
    ) -> std::result::Result<reqwest::Response, HttpError> {
        match self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Ok(response),
            Ok(response) => {
                let status = response.status();
                let retry_after = parse_retry_after(response.headers());
                Err(HttpError {
                    status: Some(status),
                    retry_after,
                    body: truncate(&response.text().await.unwrap_or_default(), 240),
                })
            }
            Err(error) => Err(HttpError {
                status: None,
                retry_after: None,
                body: error.to_string(),
            }),
        }
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
        || msg.contains("model returned an empty response")
        || msg.contains("model returned an empty streamed response")
        || msg.contains("model response truncated")
        || msg.contains("model stream ended before [done]")
}

fn is_truncated_finish_reason(reason: &str) -> bool {
    matches!(reason, "length" | "max_tokens" | "max_output_tokens")
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

/// A plain request can recover an explicitly unsupported streaming mode.
/// Do not replay authentication/schema failures, or a response whose stream
/// has already started: those are not evidence that streaming is unsupported.
pub(crate) fn rejects_streaming(error: &anyhow::Error) -> bool {
    let Some(http) = error.downcast_ref::<HttpError>() else {
        return false;
    };
    if !matches!(
        http.status.map(|status| status.as_u16()),
        Some(400 | 422 | 501)
    ) {
        return false;
    }
    let body = http.body.to_ascii_lowercase();
    body.contains("stream")
        && [
            "not supported",
            "unsupported",
            "not support",
            "not implemented",
            "disabled",
        ]
        .iter()
        .any(|marker| body.contains(marker))
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

#[cfg(test)]
mod stream_fallback_tests {
    use super::*;

    #[test]
    fn only_explicit_pre_stream_rejection_allows_plain_fallback() {
        let error = |status, body: &str| {
            anyhow!(HttpError {
                status: Some(status),
                retry_after: None,
                body: body.into(),
            })
        };
        assert!(rejects_streaming(&error(
            StatusCode::BAD_REQUEST,
            "streaming is not supported"
        )));
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert!(!rejects_streaming(&error(status, "streaming is disabled")));
        }
        assert!(!rejects_streaming(&error(
            StatusCode::BAD_REQUEST,
            "invalid tool schema"
        )));
        assert!(!rejects_streaming(&anyhow!(
            "Model stream ended before [DONE]"
        )));
        assert!(!rejects_streaming(&anyhow!(
            "Model stream provider error: streaming not supported"
        )));
    }
}

#[cfg(test)]
mod stream_timeout_tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serve one chunked SSE reply with a gap between chunks, then let the
    /// client read it under `limits`. The server is aborted afterwards, so a
    /// capped or stalled stream does not wait for the remaining chunks.
    async fn stream_with_limits(
        limits: StreamLimits,
        chunk_gap: Duration,
        chunks: usize,
    ) -> (Result<Message>, Vec<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept fixture");
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;

            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }

            for _ in 0..chunks {
                tokio::time::sleep(chunk_gap).await;
                let piece = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"tok \"}}]}\n\n";
                let frame = format!("{:x}\r\n{piece}\r\n", piece.len());
                if socket.write_all(frame.as_bytes()).await.is_err() {
                    return;
                }
                let _ = socket.flush().await;
            }

            let done = "data: [DONE]\n\n";
            let frame = format!("{:x}\r\n{done}\r\n0\r\n\r\n", done.len());
            let _ = socket.write_all(frame.as_bytes()).await;
        });

        let mut client = OpenRouterClient::new("local-test".into(), format!("http://{address}"));
        client.stream_limits = limits;

        let messages = vec![Message::user("hello")];
        let mut deltas = Vec::new();
        let result = client
            .chat_stream("test/model", &messages, &[], &mut |delta| {
                deltas.push(delta.to_string())
            })
            .await;

        server.abort();
        let _ = server.await;
        (result, deltas)
    }

    #[tokio::test]
    async fn capped_stream_fails_even_while_chunks_keep_arriving() {
        let limits = StreamLimits {
            idle: Duration::from_secs(5),
            total: Some(Duration::from_millis(150)),
        };
        let (result, deltas) = stream_with_limits(limits, Duration::from_millis(40), 20).await;
        let error = result.expect_err("a stream past the total limit fails");

        assert!(error.to_string().contains("total limit"), "{error}");
        assert!(
            !deltas.is_empty(),
            "tokens had arrived before the ceiling was reached"
        );
    }

    #[tokio::test]
    async fn streamed_reply_succeeds_while_chunks_arrive_within_the_idle_window() {
        let limits = StreamLimits {
            idle: Duration::from_millis(500),
            total: Some(Duration::from_secs(10)),
        };
        let (result, deltas) = stream_with_limits(limits, Duration::from_millis(40), 6).await;
        let message = result.expect("a slow but live stream succeeds");

        assert_eq!(deltas.len(), 6);
        assert_eq!(message.text(), "tok ".repeat(6));
    }
}
