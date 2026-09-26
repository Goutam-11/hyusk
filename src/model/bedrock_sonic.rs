//! Amazon Nova Sonic uses Bedrock's signed bidirectional event-stream API,
//! not the OpenAI-compatible chat-completions protocol.

use anyhow::{Context, Result};
use aws_sdk_bedrockruntime::{
    primitives::{
        event_stream::{EventReceiver, EventStreamSender},
        Blob,
    },
    types::{
        BidirectionalInputPayloadPart, InvokeModelWithBidirectionalStreamInput,
        InvokeModelWithBidirectionalStreamOutput,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

const DEFAULT_MODEL: &str = "amazon.nova-2-sonic-v1:0";

pub struct BedrockSonicClient {
    client: aws_sdk_bedrockruntime::Client,
    model: String,
}

#[derive(Debug)]
pub struct SonicToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

pub enum SonicEvent {
    UserText(String),
    UserTranscriptEnd,
    AssistantText(String),
    AssistantAudio(Vec<u8>),
    ToolUse(SonicToolCall),
    CompletionEnd(String),
    /// The model stopped its current response because the user began speaking.
    Interrupted,
    /// The bidirectional transport closed, not merely an assistant turn.
    StreamEnd,
    Other,
}

type InputChunk = InvokeModelWithBidirectionalStreamInput;
type InputError =
    aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamInputError;
type OutputChunk = InvokeModelWithBidirectionalStreamOutput;
type OutputError =
    aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamOutputError;

pub struct SonicSession {
    input: Option<mpsc::Sender<Result<InputChunk, InputError>>>,
    output: EventReceiver<OutputChunk, OutputError>,
    prompt_name: String,
    next_content_id: AtomicU64,
    output_role: String,
    output_type: String,
    output_stage: String,
}

impl BedrockSonicClient {
    /// Uses the standard AWS SDK credential chain (environment, shared config,
    /// SSO, and other configured providers) and the configured AWS region.
    pub async fn from_aws_config() -> Result<Self> {
        let model = std::env::var("BEDROCK_SONIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        // The SDK's short default connect timeout caused intermittent failures
        // on this desktop even while subsequent requests succeeded.
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .build(),
            )
            .load()
            .await;
        if config.region().is_none() {
            anyhow::bail!("No AWS region configured; set AWS_REGION or AWS_DEFAULT_REGION, or configure a region in your AWS CLI profile");
        }
        let client = aws_sdk_bedrockruntime::Client::new(&config);
        Ok(Self { client, model })
    }

    pub async fn start_session(
        &self,
        system_prompt: &str,
        tools: &[Value],
    ) -> Result<SonicSession> {
        let prompt_name = format!("hyusk-{}", std::process::id());
        let system_content = "hyusk-system";
        let tool_specs: Vec<Value> = tools.iter().filter_map(openai_tool_to_sonic).collect();
        let mut prompt_start = json!({
            "promptName": prompt_name,
            "textOutputConfiguration": {"mediaType": "text/plain"},
            "audioOutputConfiguration": {"mediaType": "audio/lpcm", "sampleRateHertz": 24000, "sampleSizeBits": 16, "channelCount": 1, "voiceId": "matthew", "encoding": "base64", "audioType": "SPEECH"}
        });
        if !tool_specs.is_empty() {
            prompt_start["toolUseOutputConfiguration"] = json!({"mediaType": "application/json"});
            prompt_start["toolConfiguration"] =
                json!({"tools": tool_specs, "toolChoice": {"auto": {}}});
        }

        let (tx, rx) = mpsc::channel::<Result<InputChunk, InputError>>(32);
        let input_stream = stream::unfold((rx, 0usize), |(mut rx, audio_frames)| async move {
            let item = rx.recv().await?;
            let mut audio_frames = audio_frames;
            if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                let event_name = item
                    .as_ref()
                    .ok()
                    .and_then(|chunk| chunk.as_chunk().ok())
                    .and_then(|chunk| chunk.bytes())
                    .and_then(|bytes| serde_json::from_slice::<Value>(bytes.as_ref()).ok())
                    .and_then(|payload| {
                        payload
                            .get("event")
                            .and_then(Value::as_object)
                            .and_then(|object| object.keys().next().cloned())
                    });
                if let Some(name) = event_name {
                    if name == "audioInput" {
                        audio_frames += 1;
                        if audio_frames == 1 || audio_frames.is_multiple_of(32) {
                            eprintln!("[Nova Sonic] SDK read audio frame {audio_frames}");
                        }
                    } else {
                        eprintln!("[Nova Sonic] SDK read {name}");
                    }
                }
            }
            Some((item, (rx, audio_frames)))
        })
        .fuse();
        let event_body = EventStreamSender::from(input_stream);

        send_input_event(&tx, json!({"event":{"sessionStart":{"inferenceConfiguration":{"maxTokens":2048,"topP":0.9,"temperature":0.4},"turnDetectionConfiguration":{"endpointingSensitivity":"MEDIUM"}}}})).await?;
        send_input_event(&tx, json!({"event":{"promptStart":prompt_start}})).await?;
        send_input_event(&tx, json!({"event":{"contentStart":{"promptName":prompt_name,"contentName":system_content,"type":"TEXT","interactive":false,"role":"SYSTEM","textInputConfiguration":{"mediaType":"text/plain"}}}})).await?;
        send_input_event(&tx, json!({"event":{"textInput":{"promptName":prompt_name,"contentName":system_content,"content":system_prompt}}})).await?;
        send_input_event(
            &tx,
            json!({"event":{"contentEnd":{"promptName":prompt_name,"contentName":system_content}}}),
        )
        .await?;
        let output = self.client
            .invoke_model_with_bidirectional_stream()
            .model_id(&self.model)
            .body(event_body)
            .send()
            .await
            .context("Could not start Nova Sonic stream; verify AWS CLI profile, region, model access, and bedrock:InvokeModel permission")?;
        Ok(SonicSession {
            input: Some(tx),
            output: output.body,
            prompt_name,
            next_content_id: AtomicU64::new(0),
            output_role: String::new(),
            output_type: String::new(),
            output_stage: String::new(),
        })
    }
}

impl SonicSession {
    async fn send_json(&self, event: Value) -> Result<()> {
        let input = self
            .input
            .as_ref()
            .context("Nova Sonic session is closed")?;
        send_input_event(input, event).await
    }

    /// Forward microphone frames as they are captured. The bounded receiver
    /// keeps capture close to the model's real-time input rate.
    pub fn send_live_audio_in_background(
        &self,
        mut frames: mpsc::Receiver<Vec<u8>>,
    ) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let input = self
            .input
            .as_ref()
            .context("Nova Sonic session is closed")?
            .clone();
        let prompt_name = self.prompt_name.clone();
        Ok(tokio::spawn(async move {
            let content = format!("{prompt_name}-audio");
            send_input_event(&input, audio_content_start(&prompt_name, &content)).await?;
            let mut cadence = time::interval(Duration::from_millis(32));
            cadence.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let mut count = 0usize;
            let mut sample_count = 0usize;
            let mut sample_power = 0.0_f64;
            let mut sample_peak = 0.0_f64;
            while let Some(frame) = frames.recv().await {
                cadence.tick().await;
                if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                    for bytes in frame.chunks_exact(2) {
                        let sample = i16::from_le_bytes([bytes[0], bytes[1]]) as f64 / 32768.0;
                        sample_power += sample * sample;
                        sample_peak = sample_peak.max(sample.abs());
                        sample_count += 1;
                    }
                }
                send_input_event(&input, json!({"event":{"audioInput":{"promptName":prompt_name,"contentName":content,"content":STANDARD.encode(frame)}}})).await?;
                count += 1;
            }
            send_audio_tail_and_end(&input, &prompt_name, &content, &mut cadence).await?;
            if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                let rms = (sample_power / sample_count.max(1) as f64).sqrt();
                eprintln!("[Nova Sonic] sent {count} live microphone frames; PCM rms {rms:.4}, peak {sample_peak:.4}");
            }
            Ok(())
        }))
    }

    /// Stream microphone frames for the lifetime of the input channel. Unlike
    /// `send_live_audio_in_background`, this keeps one AUDIO content open and
    /// does not append an utterance-sized silence tail whenever a capture
    /// chunk ends. Close `frames` to finish the audio content and the session
    /// may then continue with text, tools, or another audio content.
    pub fn send_persistent_live_audio_in_background(
        &self,
        mut frames: mpsc::Receiver<Vec<u8>>,
    ) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let input = self
            .input
            .as_ref()
            .context("Nova Sonic session is closed")?
            .clone();
        let prompt_name = self.prompt_name.clone();
        Ok(tokio::spawn(async move {
            send_persistent_audio(&input, &prompt_name, &mut frames).await
        }))
    }

    pub async fn send_text(&self, text: &str) -> Result<()> {
        let content = self.next_content_name("user-text");
        self.send_json(json!({"event":{"contentStart":{"promptName":self.prompt_name,"contentName":content,"type":"TEXT","interactive":true,"role":"USER","textInputConfiguration":{"mediaType":"text/plain"}}}})).await?;
        self.send_json(json!({"event":{"textInput":{"promptName":self.prompt_name,"contentName":content,"content":text}}})).await?;
        self.send_json(
            json!({"event":{"contentEnd":{"promptName":self.prompt_name,"contentName":content}}}),
        )
        .await
    }

    pub async fn send_tool_result(
        &self,
        call: &SonicToolCall,
        result: &str,
        success: bool,
    ) -> Result<()> {
        let content = self.next_content_name("tool-result");
        let tool_payload = serde_json::to_string(&json!({
            "success": success,
            "result": result,
        }))?;
        self.send_json(json!({"event":{"contentStart":{"promptName":self.prompt_name,"contentName":content,"interactive":false,"type":"TOOL","role":"TOOL","toolResultInputConfiguration":{"toolUseId":call.id,"type":"TEXT","textInputConfiguration":{"mediaType":"text/plain"}}}}})).await?;
        self.send_json(json!({"event":{"toolResult":{"promptName":self.prompt_name,"contentName":content,"content":tool_payload}}})).await?;
        self.send_json(
            json!({"event":{"contentEnd":{"promptName":self.prompt_name,"contentName":content}}}),
        )
        .await
    }

    fn next_content_name(&self, kind: &str) -> String {
        let id = self.next_content_id.fetch_add(1, Ordering::Relaxed);
        format!("{}-{kind}-{id}", self.prompt_name)
    }

    pub async fn recv(&mut self) -> Result<SonicEvent> {
        let Some(output) = self
            .output
            .recv()
            .await
            .context("Nova Sonic response stream failed")?
        else {
            return Ok(SonicEvent::StreamEnd);
        };
        let Ok(chunk) = output.as_chunk() else {
            return Ok(SonicEvent::Other);
        };
        let Some(bytes) = chunk.bytes() else {
            return Ok(SonicEvent::Other);
        };
        let event: Value = serde_json::from_slice(bytes.as_ref())
            .context("Nova Sonic returned invalid event JSON")?;
        let Some(event) = event.get("event") else {
            return Ok(SonicEvent::Other);
        };
        if is_interruption_event(event) {
            return Ok(SonicEvent::Interrupted);
        }
        if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
            if let Some(name) = event.as_object().and_then(|object| object.keys().next()) {
                eprintln!("[Nova Sonic] received {name}");
            }
        }
        if let Some(role) = event
            .get("contentStart")
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
        {
            self.output_role.clear();
            self.output_role.push_str(role);
            self.output_type.clear();
            self.output_type
                .push_str(event["contentStart"]["type"].as_str().unwrap_or_default());
            let fields = &event["contentStart"]["additionalModelFields"];
            self.output_stage = fields
                .as_str()
                .and_then(|fields| serde_json::from_str::<Value>(fields).ok())
                .or_else(|| fields.as_object().map(|_| fields.clone()))
                .and_then(|fields| fields["generationStage"].as_str().map(str::to_string))
                .unwrap_or_default();
        }
        if let Some(text) = event
            .get("textOutput")
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
        {
            return Ok(if self.output_role == "USER" {
                SonicEvent::UserText(text.into())
            } else if self.output_stage == "SPECULATIVE" {
                SonicEvent::Other
            } else {
                SonicEvent::AssistantText(text.into())
            });
        }
        if let Some(audio) = event
            .get("audioOutput")
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
        {
            return Ok(SonicEvent::AssistantAudio(
                STANDARD
                    .decode(audio)
                    .context("Invalid Nova Sonic audio chunk")?,
            ));
        }
        if let Some(end) = event.get("contentEnd") {
            let reason = end
                .get("stopReason")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                eprintln!(
                    "[Nova Sonic] content ended role={} type={} reason={reason}",
                    self.output_role, self.output_type
                );
            }
            if self.output_role == "USER" && self.output_type == "TEXT" {
                return Ok(SonicEvent::UserTranscriptEnd);
            }
            if self.output_role == "ASSISTANT" && self.output_type == "AUDIO" {
                if reason == "INTERRUPTED" {
                    return Ok(SonicEvent::Interrupted);
                }
                if reason == "END_TURN" {
                    return Ok(SonicEvent::CompletionEnd(reason.to_string()));
                }
            }
        }
        if let Some(call) = event.get("toolUse") {
            return Ok(SonicEvent::ToolUse(SonicToolCall {
                id: call
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                name: call
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                arguments: call
                    .get("content")
                    .and_then(Value::as_str)
                    .and_then(|value| serde_json::from_str(value).ok())
                    .unwrap_or(Value::Null),
            }));
        }
        if let Some(end) = event.get("completionEnd") {
            return Ok(SonicEvent::CompletionEnd(
                end.get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("END_TURN")
                    .to_string(),
            ));
        }
        Ok(SonicEvent::Other)
    }

    pub async fn close(mut self) -> Result<()> {
        self.send_json(json!({"event":{"promptEnd":{"promptName":self.prompt_name}}}))
            .await?;
        self.send_json(json!({"event":{"sessionEnd":{}}})).await?;
        self.input.take();
        Ok(())
    }
}

#[cfg(test)]
async fn send_audio_frames(
    input: &mpsc::Sender<Result<InputChunk, InputError>>,
    prompt_name: &str,
    audio_pcm16: &[u8],
) -> Result<()> {
    let content = format!("{prompt_name}-audio");
    send_input_event(input, audio_content_start(prompt_name, &content)).await?;
    // Nova's documented audio frame is about 32 ms: 16,000 samples/s ×
    // 2 bytes/sample × 0.032 s = 1,024 bytes.
    let mut cadence = time::interval(Duration::from_millis(32));
    cadence.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let started = time::Instant::now();
    let mut frames = 0usize;
    for chunk in audio_pcm16.chunks(1_024) {
        cadence.tick().await;
        send_input_event(input, json!({"event":{"audioInput":{"promptName":prompt_name,"contentName":content,"content":STANDARD.encode(chunk)}}})).await?;
        frames += 1;
    }
    send_audio_tail_and_end(input, prompt_name, &content, &mut cadence).await?;
    if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
        eprintln!(
            "[Nova Sonic] queued {frames} audio frames ({} captured bytes) in {:.2}s",
            audio_pcm16.len(),
            started.elapsed().as_secs_f32()
        );
    }
    Ok(())
}

async fn send_persistent_audio(
    input: &mpsc::Sender<Result<InputChunk, InputError>>,
    prompt_name: &str,
    frames: &mut mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    let content = format!("{prompt_name}-live-audio");
    send_input_event(input, audio_content_start(prompt_name, &content)).await?;
    let mut cadence = time::interval(Duration::from_millis(32));
    cadence.set_missed_tick_behavior(MissedTickBehavior::Delay);
    while let Some(frame) = frames.recv().await {
        cadence.tick().await;
        send_input_event(
            input,
            json!({"event":{"audioInput":{"promptName":prompt_name,"contentName":content,"content":STANDARD.encode(frame)}}}),
        )
        .await?;
    }
    send_input_event(
        input,
        json!({"event":{"contentEnd":{"promptName":prompt_name,"contentName":content}}}),
    )
    .await
}

fn audio_content_start(prompt_name: &str, content_name: &str) -> Value {
    json!({"event":{"contentStart":{"promptName":prompt_name,"contentName":content_name,"type":"AUDIO","interactive":true,"role":"USER","audioInputConfiguration":{"mediaType":"audio/lpcm","sampleRateHertz":16000,"sampleSizeBits":16,"channelCount":1,"audioType":"SPEECH","encoding":"base64"}}}})
}

fn is_interruption_event(event: &Value) -> bool {
    event.get("interrupted").is_some()
}

async fn send_audio_tail_and_end(
    input: &mpsc::Sender<Result<InputChunk, InputError>>,
    prompt_name: &str,
    content: &str,
    cadence: &mut time::Interval,
) -> Result<()> {
    // The local recorder stops soon after speech. Keep the audio clock running
    // until Nova's endpoint detector has observed enough silence.
    let silence = [0_u8; 1_024];
    for _ in 0..64 {
        cadence.tick().await;
        send_input_event(
            input,
            json!({"event":{"audioInput":{"promptName":prompt_name,"contentName":content,"content":STANDARD.encode(silence)}}}),
        )
        .await?;
    }
    send_input_event(
        input,
        json!({"event":{"contentEnd":{"promptName":prompt_name,"contentName":content}}}),
    )
    .await?;
    Ok(())
}

async fn send_input_event(
    sender: &mpsc::Sender<Result<InputChunk, InputError>>,
    event: Value,
) -> Result<()> {
    if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
        if let Some(name) = event
            .get("event")
            .and_then(Value::as_object)
            .and_then(|object| object.keys().next())
            .filter(|name| name.as_str() != "audioInput")
        {
            eprintln!("[Nova Sonic] queued {name}");
        }
    }
    let bytes = serde_json::to_vec(&event)?;
    sender
        .send(Ok(InputChunk::Chunk(
            BidirectionalInputPayloadPart::builder()
                .bytes(Blob::new(bytes))
                .build(),
        )))
        .await
        .context("Nova Sonic input stream closed")
}

fn openai_tool_to_sonic(tool: &Value) -> Option<Value> {
    let function = tool.get("function")?;
    let mut schema = function.get("parameters")?.clone();
    sanitize_tool_schema(&mut schema);
    // The Nova 2 Sonic bidirectional event protocol expects the schema as a
    // serialized JSON string inside inputSchema.json (as in AWS's Sonic SDK
    // sample), rather than the object accepted by the Converse API.
    let schema = serde_json::to_string(&schema).ok()?;
    Some(json!({"toolSpec": {
        "name": function.get("name")?,
        "description": function.get("description").and_then(Value::as_str).unwrap_or(""),
        "inputSchema": {"json": schema}
    }}))
}

/// Nova Sonic accepts a deliberately small JSON Schema subset for tool inputs.
/// Retain only the documented structural fields at the root, plus the common
/// type/description/enum/array fields on individual properties. Tool execution
/// still performs the authoritative validation locally.
fn sanitize_tool_schema(value: &mut Value) {
    let Some(root) = value.as_object_mut() else {
        *value = json!({"type": "object"});
        return;
    };
    root.retain(|key, _| matches!(key.as_str(), "type" | "properties" | "required"));
    if let Some(properties) = root.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            sanitize_property_schema(property);
        }
    }
}

fn sanitize_property_schema(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.retain(|key, _| {
        matches!(
            key.as_str(),
            "type" | "description" | "enum" | "properties" | "required" | "items"
        )
    });
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            sanitize_property_schema(property);
        }
    }
    if let Some(items) = object.get_mut("items") {
        sanitize_property_schema(items);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_interruption_event, openai_tool_to_sonic, send_audio_frames, send_persistent_audio,
    };
    use base64::Engine as _;
    use serde_json::json;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn persistent_audio_stays_open_until_frame_channel_closes() {
        let (input, mut events_rx) = mpsc::channel(8);
        let (frames_tx, mut frames_rx) = mpsc::channel(2);
        frames_tx.send(vec![1, 2]).await.unwrap();
        drop(frames_tx);
        send_persistent_audio(&input, "persistent", &mut frames_rx)
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Ok(chunk) = events_rx.try_recv() {
            let chunk = chunk.unwrap();
            let bytes = chunk.as_chunk().unwrap().bytes().unwrap();
            events.push(serde_json::from_slice::<serde_json::Value>(bytes.as_ref()).unwrap());
        }
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0]["event"]["contentStart"]["contentName"],
            "persistent-live-audio"
        );
        assert_eq!(
            events[1]["event"]["audioInput"]["contentName"],
            "persistent-live-audio"
        );
        assert_eq!(
            events[2]["event"]["contentEnd"]["contentName"],
            "persistent-live-audio"
        );
    }

    #[test]
    fn maps_sonic_interruption_event() {
        assert!(is_interruption_event(&json!({"interrupted": {}})));
        assert!(!is_interruption_event(
            &json!({"completionEnd": {"stopReason": "END_TURN"}})
        ));
    }

    #[tokio::test]
    async fn audio_events_have_matching_boundaries_and_pcm_frames() {
        let (sender, mut receiver) = mpsc::channel(128);
        let pcm = vec![42_u8; 2_048];
        send_audio_frames(&sender, "test-prompt", &pcm)
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            let chunk = chunk.unwrap();
            let bytes = chunk.as_chunk().unwrap().bytes().unwrap();
            events.push(serde_json::from_slice::<serde_json::Value>(bytes.as_ref()).unwrap());
        }
        assert_eq!(events.len(), 68);
        assert_eq!(
            events[0]["event"]["contentStart"]["contentName"],
            "test-prompt-audio"
        );
        assert_eq!(events[0]["event"]["contentStart"]["interactive"], true);
        for event in &events[1..3] {
            assert_eq!(
                event["event"]["audioInput"]["contentName"],
                "test-prompt-audio"
            );
            let encoded = event["event"]["audioInput"]["content"].as_str().unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap(),
                vec![42_u8; 1_024]
            );
        }
        assert_eq!(
            events[67]["event"]["contentEnd"]["contentName"],
            "test-prompt-audio"
        );
    }

    #[test]
    fn converts_local_function_schemas_to_sonic_tool_specs() {
        let spec = json!({"type":"function","function":{"name":"open_app","description":"Launch an app","parameters":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}});
        let converted = openai_tool_to_sonic(&spec).unwrap();
        assert_eq!(converted["toolSpec"]["name"], "open_app");
        assert_eq!(converted["toolSpec"]["description"], "Launch an app");
        let schema = converted["toolSpec"]["inputSchema"]["json"]
            .as_str()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(schema).unwrap(),
            json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})
        );
    }

    #[test]
    fn removes_unsupported_schema_keywords_for_nova() {
        let spec = json!({
            "type": "function",
            "function": {
                "name": "gnome_doctor",
                "description": "Check GNOME integration.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "default": 5}
                    },
                    "additionalProperties": false,
                    "$schema": "https://json-schema.org/draft/2020-12/schema"
                }
            }
        });
        let converted = openai_tool_to_sonic(&spec).unwrap();
        let schema = converted["toolSpec"]["inputSchema"]["json"]
            .as_str()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(schema).unwrap(),
            json!({
                "type": "object",
                "properties": {"limit": {"type": "integer"}}
            })
        );
    }
}
