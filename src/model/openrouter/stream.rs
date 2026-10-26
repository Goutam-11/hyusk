//! Streaming chat-completion decoding; transport stays in the parent module.

use super::{is_truncated_finish_reason, protocol::validate_assistant_message};
use crate::types::{
    message::{FunctionCall, ToolCall},
    Message,
};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;

// A broken endpoint must not grow Hyusk's memory indefinitely while streaming
// unterminated events or enormous tool arguments. This includes wire framing.
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn parse_sse_stream<F>(
    response: reqwest::Response,
    on_delta: &mut F,
) -> Result<Message>
where
    F: FnMut(&str),
{
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut content = String::new();
    let mut tool_calls: std::collections::BTreeMap<usize, ToolCall> =
        std::collections::BTreeMap::new();
    let mut done = false;
    let mut finish_reason = None;
    let mut received_bytes = 0usize;
    let mut refusal = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Model stream error")?;
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
    use super::{parse_sse_stream, pop_sse_line, MAX_STREAM_BYTES};
    use crate::types::Message;
    use anyhow::Result;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn parse_events(events: String) -> Result<Message> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{events}", events.len());
            // Oversize tests may close the receiver before the write finishes.
            let _ = socket.write_all(response.as_bytes()).await;
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()?
            .get(format!("http://{address}"))
            .send()
            .await?;
        let result = parse_sse_stream(response, &mut |_| {}).await;
        server.await?;
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
