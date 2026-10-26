//! Local transport regressions: no microphone, provider credentials, or tools.
use super::*;
use serde_json::json;
use std::sync::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn request_case(responses: Vec<(&str, &str, String)>) -> (Result<Message>, Vec<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let responses: Vec<_> = responses
        .into_iter()
        .map(|(s, t, b)| (s.to_owned(), t.to_owned(), b))
        .collect();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let request = loop {
                let mut chunk = [0; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                assert!(bytes.len() < 100_000);
                if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if bytes.len() >= index + 4 + length {
                        break serde_json::from_slice::<Value>(
                            &bytes[index + 4..index + 4 + length],
                        )
                        .unwrap();
                    }
                }
            };
            let index = {
                let mut requests = captured.lock().unwrap();
                requests.push(request);
                requests.len() - 1
            };
            let (status, content_type, body) = &responses[index.min(responses.len() - 1)];
            let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let mut agent = Agent::new(
        OpenRouterClient::new("local-test".into(), base),
        "test".into(),
        ToolRegistry::new(),
    );
    agent.messages = vec![Message::system("Test"), Message::user("Hello")];
    let (sentences, _) = tokio::sync::mpsc::unbounded_channel();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.request_model(&CancellationToken::new(), Some(&sentences), None),
    )
    .await;
    server.abort();
    let _ = server.await;
    let requests = requests.lock().unwrap().clone();
    (
        result
            .expect("local model request must settle")
            .expect("not cancelled")
            .map(|reply| reply.message),
        requests,
    )
}

fn completed_stream() -> String {
    "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".into()
}

#[tokio::test]
async fn authentication_failure_is_not_replayed_as_plain_request() {
    let (result, requests) = request_case(vec![(
        "401 Unauthorized",
        "application/json",
        "missing authentication".into(),
    )])
    .await;
    assert!(result.is_err());
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn incomplete_stream_is_not_replayed_as_plain_request() {
    let (result, requests) = request_case(vec![(
        "200 OK",
        "text/event-stream",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Partial\"}}]}\n\n".into(),
    )])
    .await;
    assert!(result.unwrap_err().to_string().contains("before [DONE]"));
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn explicit_stream_rejection_falls_back_once() {
    let (result, requests) = request_case(vec![
        ("400 Bad Request", "application/json", "streaming is not supported".into()),
        ("200 OK", "application/json", json!({"choices":[{"message":{"role":"assistant","content":"Hello"},"finish_reason":"stop"}]}).to_string()),
    ]).await;
    assert_eq!(result.unwrap().text(), "Hello");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["stream"], true);
    assert!(requests[1].get("stream").is_none());
}

#[tokio::test]
async fn streamed_sentences_reach_the_sink_before_the_reply_completes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut chunk = [0; 4096];
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&chunk[..read]);
        }

        let first = json!({
            "choices": [{
                "delta": { "content": "{\"text\":\"First sentence. Second" },
                "finish_reason": null
            }]
        });
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {first}\n\n"
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();

        // The reply must not finish until the test has seen its first sentence.
        released.await.unwrap();

        let second = json!({
            "choices": [{
                "delta": { "content": " sentence.\",\"needs_reply\":false}" },
                "finish_reason": "stop"
            }]
        });
        let tail = format!("data: {second}\n\ndata: [DONE]\n\n");
        socket.write_all(tail.as_bytes()).await.unwrap();
    });

    let mut agent = Agent::new(
        OpenRouterClient::new("local-test".into(), base),
        "test".into(),
        ToolRegistry::new(),
    );
    agent.messages = vec![Message::system("Test"), Message::user("Hello")];

    let (sentences, mut sentence_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let request = agent.request_model(&cancel, Some(&sentences), Some(&ui_tx));

    let observe = async {
        let first = tokio::time::timeout(Duration::from_secs(5), sentence_rx.recv())
            .await
            .expect("first sentence while the stream is still open")
            .expect("sentence channel open");
        assert_eq!(first, "First sentence.");
        release.send(()).unwrap();
        let second = tokio::time::timeout(Duration::from_secs(5), sentence_rx.recv())
            .await
            .expect("second sentence")
            .expect("sentence channel open");
        assert_eq!(second, "Second sentence.");
    };

    let (reply, ()) = tokio::join!(request, observe);
    let reply = reply.expect("not cancelled").expect("streamed reply");
    assert_eq!(
        reply.message.text(),
        "{\"text\":\"First sentence. Second sentence.\",\"needs_reply\":false}"
    );
    assert!(reply.streamed, "text was already delivered");

    assert!(sentence_rx.try_recv().is_err(), "no duplicated reply");
    let deltas: Vec<_> = std::iter::from_fn(|| ui_rx.try_recv().ok()).collect();
    assert_eq!(deltas.len(), 2);
    assert!(matches!(&deltas[0], HyuskEvent::ResponseDelta(text) if text == "First sentence."));
    assert!(matches!(&deltas[1], HyuskEvent::ResponseDelta(text) if text == "Second sentence."));

    server.await.unwrap();
}

#[tokio::test]
async fn transient_http_failure_retries_stream_without_plain_replay() {
    let (result, requests) = request_case(vec![
        (
            "503 Service Unavailable",
            "application/json",
            "unavailable".into(),
        ),
        ("200 OK", "text/event-stream", completed_stream()),
    ])
    .await;
    assert_eq!(result.unwrap().text(), "Hello");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request["stream"] == true));
}
