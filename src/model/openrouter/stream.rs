//! Streaming chat-completion decoding; transport stays in the parent module.

use super::{is_truncated_finish_reason, protocol::validate_assistant_message};
use crate::types::{
    message::{FunctionCall, ToolCall},
    Message,
};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

// A broken endpoint must not grow Hyusk's memory indefinitely while streaming
// unterminated events or enormous tool arguments. This includes wire framing.
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// A streamed reply may take as long as it keeps producing tokens, so the
/// limit is on silence between chunks rather than on the whole response.
const DEFAULT_IDLE_SECS: u64 = 45;
/// Whole-reply ceiling, well above a long spoken answer. `0` disables it and
/// leaves the idle window as the only bound.
const DEFAULT_TOTAL_SECS: u64 = 600;
const MAX_IDLE_SECS: u64 = 3_600;
const MAX_TOTAL_SECS: u64 = 86_400;

/// Timing policy for one streamed reply.
#[derive(Clone, Copy, Debug)]
pub(super) struct StreamLimits {
    /// Fail when no data arrives for this long.
    pub idle: Duration,
    /// Fail once the reply has been streaming for this long; `None` disables
    /// the ceiling.
    pub total: Option<Duration>,
}

impl StreamLimits {
    pub(super) fn from_env() -> Self {
        Self::from_values(
            env_seconds("HYUSK_STREAM_IDLE_TIMEOUT_SECS"),
            env_seconds("HYUSK_STREAM_TOTAL_TIMEOUT_SECS"),
        )
    }

    /// Testable half of [`Self::from_env`]: unset values fall back to the
    /// defaults, `0` disables the total ceiling, and both are clamped.
    fn from_values(idle_secs: Option<u64>, total_secs: Option<u64>) -> Self {
        let idle = idle_secs
            .filter(|seconds| *seconds > 0)
            .map(|seconds| seconds.min(MAX_IDLE_SECS))
            .unwrap_or(DEFAULT_IDLE_SECS);
        let total = total_secs
            .map(|seconds| seconds.min(MAX_TOTAL_SECS))
            .unwrap_or(DEFAULT_TOTAL_SECS);

        Self {
            idle: Duration::from_secs(idle),
            total: (total > 0).then(|| Duration::from_secs(total)),
        }
    }
}

fn env_seconds(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse::<u64>().ok()
}

fn describe(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds > 0 {
        format!("{seconds}s")
    } else {
        format!("{}ms", duration.as_millis())
    }
}

pub(super) async fn parse_sse_stream<F>(
    response: reqwest::Response,
    on_delta: &mut F,
    limits: StreamLimits,
) -> Result<Message>
where
    F: FnMut(&str),
{
    use futures_util::StreamExt;

    let deadline = limits.total.map(|total| Instant::now() + total);
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut content = String::new();
    let mut tool_calls: std::collections::BTreeMap<usize, ToolCall> =
        std::collections::BTreeMap::new();
    let mut done = false;
    let mut finish_reason = None;
    let mut received_bytes = 0usize;
    let mut refusal = String::new();

    loop {
        // Wait for the next chunk, bounded by the idle window and by whatever
        // remains of the whole-reply ceiling, whichever ends first.
        let (wait, capped_by_total) = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(anyhow!(
                        "Model stream exceeded the {} total limit",
                        describe(limits.total.unwrap_or_default())
                    ));
                }
                (remaining.min(limits.idle), remaining <= limits.idle)
            }
            None => (limits.idle, false),
        };

        let chunk = match tokio::time::timeout(wait, stream.next()).await {
            Ok(Some(chunk)) => chunk.context("Model stream error")?,
            Ok(None) => break,
            Err(_) if capped_by_total => {
                return Err(anyhow!(
                    "Model stream exceeded the {} total limit",
                    describe(limits.total.unwrap_or_default())
                ))
            }
            Err(_) => {
                return Err(anyhow!(
                    "Model stream stalled: no data arrived for {}",
                    describe(limits.idle)
                ))
            }
        };

        received_bytes = received_bytes.saturating_add(chunk.len());
        if received_bytes > MAX_STREAM_BYTES {
            return Err(anyhow!("Model stream exceeded the 4 MiB response limit"));
        }
        buffer.extend_from_slice(&chunk);

        while let Some(line) = pop_sse_line(&mut buffer, false)? {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };

            let data = data.trim();

            if data == "[DONE]" {
                done = true;
                break;
            }

            let value = serde_json::from_str::<Value>(data)
                .context("Invalid JSON in model stream event")?;

            if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
                return Err(anyhow!(
                    "Model stream provider error: {}",
                    super::truncate(&error.to_string(), 240)
                ));
            }

            if let Some(reason) = value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                finish_reason = Some(reason.to_string());
            }

            let Some(delta) = value.pointer("/choices/0/delta") else {
                continue;
            };

            if let Some(role) = delta.get("role").and_then(Value::as_str) {
                if role != "assistant" {
                    return Err(anyhow!("Model stream has unexpected role '{role}'"));
                }
            }
            if let Some(text) = delta.get("refusal").and_then(Value::as_str) {
                refusal.push_str(text);
            }

            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push_str(text);
                    on_delta(text);
                }
            }

            if let Some(raw_calls) = delta.get("tool_calls") {
                let calls = raw_calls
                    .as_array()
                    .context("Streamed tool_calls field is not an array")?;
                for call in calls {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .context("Streamed tool call is missing a valid index")?
                        as usize;

                    let entry = tool_calls.entry(index).or_insert_with(|| ToolCall {
                        id: String::new(),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: String::new(),
                            arguments: String::new(),
                        },
                    });

                    if let Some(raw_id) = call.get("id") {
                        let id = raw_id
                            .as_str()
                            .context("Streamed tool-call ID is not a string")?;
                        if !id.is_empty() {
                            if !entry.id.is_empty() && entry.id != id {
                                return Err(anyhow!(
                                    "Streamed tool call {index} changed its ID mid-response"
                                ));
                            }
                            entry.id = id.to_string();
                        }
                    }

                    if let Some(raw_kind) = call.get("type") {
                        let kind = raw_kind
                            .as_str()
                            .context("Streamed tool-call type is not a string")?;
                        if kind != "function" {
                            return Err(anyhow!("Unsupported streamed tool-call type '{kind}'"));
                        }
                        entry.kind = kind.to_string();
                    }

                    if let Some(function) = call.get("function") {
                        if !function.is_object() {
                            return Err(anyhow!("Streamed tool-call function is not an object"));
                        }
                        if let Some(raw_name) = function.get("name") {
                            let name = raw_name
                                .as_str()
                                .context("Streamed function name is not a string")?;
                            entry.function.name.push_str(name);
                        }

                        if let Some(raw_arguments) = function.get("arguments") {
                            let arguments = raw_arguments
                                .as_str()
                                .context("Streamed function arguments are not a string")?;
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

    if let Some(line) = pop_sse_line(&mut buffer, true)? {
        if line
            .strip_prefix("data:")
            .is_some_and(|data| data.trim() == "[DONE]")
        {
            done = true;
        }
    }

    // Bedrock Mantle can close a valid OpenAI-compatible SSE response after
    // its terminal finish_reason event without sending the optional [DONE]
    // sentinel. A finish_reason is the protocol's completion boundary; EOF
    // without one is still incomplete and must not be treated as success.
    if !done && finish_reason.is_none() {
        return Err(anyhow!("Model stream ended before [DONE]"));
    }

    if finish_reason
        .as_deref()
        .is_some_and(is_truncated_finish_reason)
    {
        return Err(anyhow!("Model response truncated (finish_reason=length)"));
    }

    if content.trim().is_empty() && tool_calls.is_empty() && !refusal.trim().is_empty() {
        content = refusal;
    }
    if content.trim().is_empty() && tool_calls.is_empty() {
        return Err(anyhow!("Model returned an empty streamed response"));
    }
    if let Some((&last_index, _)) = tool_calls.last_key_value() {
        if last_index != tool_calls.len() - 1 {
            return Err(anyhow!("Streamed tool-call indexes are not contiguous"));
        }
    }

    let mut message = Message::assistant(content.clone());

    if !tool_calls.is_empty() {
        if content.is_empty() {
            message.content = None;
        }

        message.tool_calls = Some(tool_calls.into_values().collect());
    }

    validate_assistant_message(&message)?;

    Ok(message)
}

fn pop_sse_line(buffer: &mut Vec<u8>, eof: bool) -> Result<Option<String>> {
    let end = buffer
        .iter()
        .position(|byte| *byte == b'\n')
        .or_else(|| eof.then_some(buffer.len()));
    let Some(end) = end else {
        return Ok(None);
    };
    let mut line = buffer
        .drain(..end + usize::from(end < buffer.len()))
        .collect::<Vec<_>>();
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(Some(
        String::from_utf8(line).context("Invalid UTF-8 in model stream event")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{parse_sse_stream, pop_sse_line, StreamLimits, MAX_STREAM_BYTES};
    use crate::types::Message;
    use anyhow::Result;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn generous_limits() -> StreamLimits {
        StreamLimits {
            idle: Duration::from_secs(5),
            total: Some(Duration::from_secs(30)),
        }
    }

    async fn parse_events(events: String) -> Result<Message> {
        parse_pieces(
            vec![(Duration::ZERO, events)],
            generous_limits(),
            &mut |_| {},
        )
        .await
    }

    /// Serve one request as a chunked SSE response, writing each piece after
    /// its delay. A piece that never arrives inside the idle window is the
    /// stalled-stream fixture. The server is aborted once the client finishes,
    /// so a test that ends the stream early does not wait for the rest.
    async fn parse_pieces<F>(
        pieces: Vec<(Duration, String)>,
        limits: StreamLimits,
        on_delta: &mut F,
    ) -> Result<Message>
    where
        F: FnMut(&str),
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;

            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }

            for (delay, piece) in pieces {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let frame = format!("{:x}\r\n{piece}\r\n", piece.len());
                if socket.write_all(frame.as_bytes()).await.is_err() {
                    return;
                }
                let _ = socket.flush().await;
            }

            let _ = socket.write_all(b"0\r\n\r\n").await;
        });

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()?
            .get(format!("http://{address}"))
            .send()
            .await?;
        let result = parse_sse_stream(response, &mut *on_delta, limits).await;
        server.abort();
        let _ = server.await;
        result
    }

    #[tokio::test]
    async fn surfaces_provider_stream_errors_instead_of_empty_response() {
        let error = parse_events(
            "data: {\"error\":{\"message\":\"quota exhausted\"}}\n\ndata: [DONE]\n\n".into(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("quota exhausted"));
    }

    #[tokio::test]
    async fn accepts_terminal_finish_reason_when_endpoint_omits_done_sentinel() {
        let message = parse_events(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Complete\"},\"finish_reason\":\"stop\"}]}\n\n"
                .into(),
        )
        .await
        .unwrap();

        assert_eq!(message.text(), "Complete");
    }

    #[tokio::test]
    async fn preserves_streamed_refusal_instead_of_reporting_no_answer() {
        let result = parse_events("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"refusal\":\"Cannot do that.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into()).await.unwrap();
        assert_eq!(result.text(), "Cannot do that.");
    }

    #[tokio::test]
    async fn rejects_unexpected_streamed_role() {
        let error = parse_events("data: {\"choices\":[{\"delta\":{\"role\":\"user\",\"content\":\"bad\"}}]}\n\ndata: [DONE]\n\n".into()).await.unwrap_err();
        assert!(error.to_string().contains("unexpected role"));
    }

    #[tokio::test]
    async fn bounds_unterminated_stream_memory() {
        let error = parse_events("x".repeat(MAX_STREAM_BYTES + 1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("response limit"));
    }

    #[tokio::test]
    async fn slow_chunks_keep_a_reply_alive_past_the_idle_window() {
        // Scaled-down version of the real case: a reply that keeps producing
        // tokens is never cut, however long the gaps are compared with the old
        // whole-request timeout.
        let mut pieces = Vec::new();
        let mut expected = String::new();
        for index in 0..8 {
            let token = format!("t{index} ");
            expected.push_str(&token);
            pieces.push((
                if index == 0 {
                    Duration::ZERO
                } else {
                    Duration::from_millis(40)
                },
                format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"content\":\"{token}\"}}}}]}}\n\n"
                ),
            ));
        }
        pieces.push((Duration::ZERO, "data: [DONE]\n\n".to_string()));

        let limits = StreamLimits {
            idle: Duration::from_millis(500),
            total: Some(Duration::from_secs(30)),
        };
        let mut deltas = Vec::new();
        let message = parse_pieces(pieces, limits, &mut |delta| deltas.push(delta.to_string()))
            .await
            .expect("a live stream succeeds");

        assert_eq!(message.text(), expected);
        assert_eq!(deltas.concat(), expected);
    }

    #[tokio::test]
    async fn stalled_stream_fails_after_the_idle_window() {
        let pieces = vec![
            (
                Duration::ZERO,
                "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Half\"}}]}\n\n"
                    .to_string(),
            ),
            // Never delivered: the client gives up first.
            (Duration::from_millis(600), "data: [DONE]\n\n".to_string()),
        ];
        let limits = StreamLimits {
            idle: Duration::from_millis(120),
            total: Some(Duration::from_secs(30)),
        };

        let started = std::time::Instant::now();
        let error = parse_pieces(pieces, limits, &mut |_| {})
            .await
            .expect_err("a stalled stream fails");

        assert!(error.to_string().contains("stalled"), "{error}");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the idle window ends the stream, not the server delay"
        );
    }

    #[tokio::test]
    async fn total_limit_ends_a_stream_that_keeps_sending() {
        let mut pieces = Vec::new();
        for _ in 0..20 {
            pieces.push((
                Duration::from_millis(40),
                "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"x\"}}]}\n\n"
                    .to_string(),
            ));
        }
        let limits = StreamLimits {
            idle: Duration::from_secs(5),
            total: Some(Duration::from_millis(150)),
        };

        let error = parse_pieces(pieces, limits, &mut |_| {})
            .await
            .expect_err("a stream past the total limit fails");

        assert!(error.to_string().contains("total limit"), "{error}");
    }

    #[test]
    fn stream_limits_use_defaults_and_treat_zero_total_as_unlimited() {
        let defaults = StreamLimits::from_values(None, None);
        assert_eq!(defaults.idle, Duration::from_secs(45));
        assert_eq!(defaults.total, Some(Duration::from_secs(600)));

        let unlimited = StreamLimits::from_values(Some(0), Some(0));
        assert_eq!(
            unlimited.idle,
            Duration::from_secs(45),
            "an idle window of zero is not usable"
        );
        assert_eq!(unlimited.total, None, "zero disables the total ceiling");

        let clamped = StreamLimits::from_values(Some(99_999), Some(999_999));
        assert_eq!(clamped.idle, Duration::from_secs(3_600));
        assert_eq!(clamped.total, Some(Duration::from_secs(86_400)));
    }

    #[test]
    fn sse_line_buffer_preserves_utf8_split_across_chunks() {
        let encoded = "data: {\"text\":\"café\"}\n".as_bytes();
        let split = encoded.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
        let mut buffer = encoded[..split].to_vec();
        assert!(pop_sse_line(&mut buffer, false).unwrap().is_none());

        buffer.extend_from_slice(&encoded[split..]);
        assert_eq!(
            pop_sse_line(&mut buffer, false).unwrap().as_deref(),
            Some("data: {\"text\":\"café\"}")
        );
    }

    #[test]
    fn sse_line_buffer_flushes_final_line_at_eof() {
        let mut buffer = b"data: [DONE]".to_vec();
        assert_eq!(
            pop_sse_line(&mut buffer, true).unwrap().as_deref(),
            Some("data: [DONE]")
        );
        assert!(buffer.is_empty());
    }
}
