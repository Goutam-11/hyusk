# Architecture

`hyusk_agent` is a small composition root around four concerns:

1. A remote chat model accessed through an OpenAI-compatible endpoint.
2. A local tool registry that executes computer, shell, process, and media
   actions.
3. A Tokio event task that coordinates voice input, tool activity, responses,
   and TTS.
4. Optional wake-word, speech-to-text, and overlay components that are wired
   together when their local models and backends are available.

## Runtime topology

```text
                         +-----------------------+
                         | main.rs               |
                         | configuration + setup |
                         +-----------+-----------+
                                     |
                 +-------------------+-------------------+
                 |                   |                   |
          +------v------+     +------v------+     +------v------+
          | Agent task  |     | Wake task   |     | UI task     |
          | MPSC loop   |     | microphone  |     | eframe      |
          +------+------+     +------+------+     +------+------+
                 |                   |                   |
          +------v------+     +------v------+            |
          | OpenRouter  |     | ONNX model  |            |
          | client      |     | classifier  |            |
          +------+------+     +------+------+            |
                 |                   |                   |
          +------v------+     +------v------+            |
          | ToolRegistry|     | SpeechToText|            |
          +--+---+---+--+     +------+------+            |
             |   |   |               |                   |
        computer shell process media +---- UserInput --->+
```

`main.rs` creates one MPSC channel for requests to the agent and another for
events from the agent to the UI. The wake detector pauses through a small
shared `WakeResume` signal while STT records, so two components do not compete
for the microphone at the same time.

## Startup sequence

`main` performs the following steps in order:

1. Call `dotenvy::dotenv()` and read OpenRouter configuration.
2. Construct `OpenRouterClient`.
3. Register `ComputerTool`, `ShellTool`, `ProcessTool`, and `MediaTool`.
4. Construct `Agent` with an in-memory system prompt and tool specs.
5. Probe the default audio input device.
6. Create the agent and UI channels.
7. Try to load `STT_MODEL`; on failure, log a warning and continue in
   text-only mode.
8. Spawn `run_agent_task` with the STT handle and the shared wake-resume signal.
9. If `WAKE_WORD_ENABLED` is truthy, try to construct `WakeWordDetector` from
   `WAKE_WORD_MODELS`, `WAKE_WORD_MODEL`, or the first available candidate
   (`hey_hyusk.onnx`, `hey_livekit.onnx`, `nihao_livekit.onnx`). The threshold,
   noise floor, and cooldown come from `WAKE_WORD_THRESHOLD`,
   `WAKE_WORD_MIN_RMS`, and `WAKE_WORD_COOLDOWN_MS`. A missing or incompatible
   model logs a warning and skips wake detection instead of stopping startup.
10. Start a terminal stdin reader that sends typed lines as `UserInput` events.
11. Start the native `eframe` event loop.

## Modules

| Module | Responsibility |
| --- | --- |
| `agent/runner.rs` | Owns message history, builds tool specs, and runs the tool-call loop. |
| `agent/runtime.rs` | Consumes `HyuskEvent`s, runs post-wake STT, invokes the agent, emits responses, and calls TTS. |
| `model/openrouter.rs` | Serializes chat requests, handles retries, and deserializes structured tool calls. |
| `tools/tool.rs` | Defines the async `Tool` trait and `ToolResult`. |
| `tools/registry.rs` | Stores tools as `Arc<dyn Tool>` and exposes them by name. |
| `tools/computer.rs` | Controls the GUI with the portal/enigo backends, screenshots, OCR, and key/mouse helpers. |
| `tools/portal.rs` | Linux-only XDG RemoteDesktop and Screenshot portal integration. |
| `tools/shell.rs` | Executes commands through `sh -c`, with foreground and detached modes. |
| `tools/process.rs` | Spawns an executable directly and reports immediate failures. |
| `tools/media.rs` | Delegates MPRIS control to `playerctl`. |
| `types/message.rs` | Mirrors the chat-completions message, tool-call, and tool-result wire shape. |
| `types/event.rs` | Defines events exchanged by the agent, wake detector, STT path, and UI. |
| `types/state.rs` | Defines visual states consumed by the overlay. |
| `speech/speech.rs` | Contains Whisper microphone transcription and platform-specific TTS adapters. |
| `wake/detector.rs` | Captures microphone audio, resamples it, runs a LiveKit ONNX classifier, and exposes `WakeResume`. |
| `ui/orb.rs` | Consumes events and draws the animated butterfly overlay. |

## Voice and wake path

```text
WakeWordDetector
      |
      |  emits WakeWordDetected
      v
run_agent_task
      |
      |  StateChanged(Waking)
      |  StateChanged(Listening)
      |  SpeechToText::transcribe_from_microphone
      |  WakeResume::resume
      v
UserInput event
      |
      v
Agent::handle -> OpenRouter -> tool loop -> final text
      |
      +-> Response event
      +-> TextToSpeech::speak (awaited)
      +-> StateChanged(Hidden)
```

`SpeechToText` opens the default CPAL input device, captures a fixed window,
downmixes to mono, resamples to 16 kHz, rejects near-silence, and transcribes
with Whisper. If STT is not configured, times out, or returns empty text, the
wake event is ignored and the detector resumes.

The detector scores every configured classifier and takes the highest
confidence, runs RNNoise denoising, tracks a slowly-moving noise floor, and
peak-normalizes inference windows so quiet or distant speech still scores.

Typed terminal lines take the same `UserInput` path, so the agent remains
usable for text and computer tool calls even when wake detection or STT is
unavailable.

A new `UserInput` (typed or transcribed) cancels the active turn, including the
model request, the running tool call, and Linux TTS playback. The agent rolls
the conversation back to the state before the cancelled turn before starting
the replacement.

## Agent loop

`Agent::handle` appends a user message and repeatedly calls the model:

```text
request chat completion
        |
        +-- assistant has tool_calls
        |       |
        |       +-- append assistant message
        |       +-- for each call:
        |             - resolve tool by name
        |             - execute with JSON arguments
        |             - append role: tool result with tool_call_id
        |       +-- request another completion
        |
        +-- no tool_calls
                - append assistant message
                - return text
```

The ordering of the assistant message before its tool-result messages is part
of the OpenAI-compatible protocol and must be preserved when changing the loop.
Tool arguments arrive as a JSON-encoded string. The runner parses them only for
console display; the tool implementation parses the original string according
to its own schema.

The agent keeps all messages in `Vec<Message>` for the lifetime of the process.
There is no compaction, database storage, or cross-process session sharing. Each
turn records its starting message index; cancellation truncates back to that
index so a replacement turn never sees an assistant `tool_calls` message without
its tool results.

## Tool specifications

At construction time, `Agent` enumerates the registry and creates one OpenAI
function spec per tool:

```json
{
  "type": "function",
  "function": {
    "name": "computer",
    "description": "...",
    "parameters": {
      "type": "object"
    }
  }
}
```

The model layer does not know about individual tool structs. This keeps the
provider boundary independent from local execution details.

## Events and UI states

`HyuskEvent` is a shared enum rather than a typed API:

- `WakeWordDetected`
- `UserInput(String)`
- `StateChanged(HyuskState)`
- `ToolStarted { name }`
- `ToolFinished { name, success }`
- `Response(String)`
- `Shutdown`

`run_agent_task` handles wake events and `UserInput`, cancels the previous turn,
and spawns each turn as a separate task. `OrbApp` drains events on each frame
and changes its visual state, including `Waking` and `Listening`. The overlay is
a 160 x 160 transparent window with a black circle and always-on-top window
level. It does not request keyboard focus by default (`ORB_STEAL_FOCUS=0`), so
portal keyboard input reaches the controlled application. The `Response` text
is stored but not rendered, so the UI remains an activity indicator rather than
a text console or control surface.

## Model protocol and failure handling

The client posts to:

```text
{OPENROUTER_BASE_URL}/chat/completions
```

The request contains `model`, `messages`, and optional `tools`. Transient
failures are retried up to three times after the initial attempt. The client
classifies network errors, HTTP 408, HTTP 429, 5xx responses, malformed JSON,
and empty choices as retryable. A numeric `Retry-After` header overrides the
normal exponential delay, capped at eight seconds.

Messages serialize `content` as either a plain string or a multimodal parts
array. When `MODEL_VISION` is enabled, a `computer` screenshot is converted into
a user message with a `data:image/png;base64,...` part so vision-capable models
can inspect the screen directly.

HTTP 400, 401, 403, and 404 responses fail immediately because retrying them is
unlikely to correct an invalid key, model, or payload.

## Boundaries and intentional omissions

This crate is an integration scaffold, not the full Hyusk runtime. It does not
provide:

- streaming model output;
- persistent sessions or message history;
- permission prompts or a sandbox for tool calls;
- process supervision or monitoring (turn cancellation does not kill arbitrary
  shell children);
- concurrent tool execution;
- a first-use portal approval dialog that can be skipped: RemoteDesktop and
  Screenshot access always require the desktop's own prompt;
- echo cancellation or true acoustic barge-in (while the agent speaks the clap
  scanner is disabled and the wake threshold is raised to avoid hearing its own
  voice, but a deliberate wake word still interrupts the turn; wake detection
  and STT use the microphone sequentially);
- guaranteed top-center window placement or focus on Wayland, where the
  compositor may ignore those requests;
- a rendered response UI;
- a stable client API or IPC protocol.

Those boundaries are useful to preserve while extending the crate: keep model
translation, tool execution, event coordination, and desktop/audio adapters
separate rather than putting all behavior in `main.rs`.
