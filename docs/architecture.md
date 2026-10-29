# Architecture

`hyusk_agent` is a small composition root around four concerns:

1. A remote chat model accessed through an OpenAI-compatible endpoint.
2. A local tool registry that executes desktop, shell, media, memory,
   scheduling, and background-task actions.
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
          | provider    |     | ONNX model  |            |
          | client      |     | classifier  |            |
          +------+------+     +------+------+            |
                 |                   |                   |
          +------v------+     +------v------+            |
          | ToolRegistry|     | SpeechToText|            |
          +--+---+---+--+     +------+------+            |
             |   |   |               |                   |
    desktop shell media memory +------+---- UserInput --->+
```

`main.rs` creates one MPSC channel for requests to the agent and another for
events from the agent to the UI. The wake detector pauses through a small
shared `WakeResume` signal while STT records, so two components do not compete
for the microphone at the same time.

## Startup sequence

`main` performs the following steps in order:

1. Call `dotenvy::dotenv()` and resolve each API provider's client from the
   keyring, then the environment (`model::configured_client`).
2. Read the saved provider/model selection from `~/.config/hyusk/config.json`,
   falling back to `OPENROUTER_MODEL` and the first configured provider.
3. Register the core tools, the `browser` tool when its CDP configuration
   loads, and on Linux the `accessibility`, `window`, and `gnome_doctor` tools
   unconditionally (they report unavailability at call time).
4. Start the optional TLS link server and register `mobile` when
   `HYUSK_LINK_ENABLED` is set.
5. Construct `Agent` and `SharedActiveModel` with a per-turn system prompt and
   tool specs.
6. Probe the default audio input device.
7. Create the agent, UI, and status channels.
8. Try to load `STT_MODEL`; on failure, log a warning and continue in
   text-only mode.
9. Spawn `run_agent_task` with the STT handle and the shared wake-resume signal.
10. Spawn the model-catalog refresh and the `hyusk-control.json` poller used by
    the indicators.
11. If `WAKE_WORD_ENABLED` is truthy, try to construct `WakeWordDetector` from
    `WAKE_WORD_MODELS`, `WAKE_WORD_MODEL`, or the first available candidate
    (`hey_hyusk.onnx`, `hey_livekit.onnx`, `alexa.onnx`, `nihao_livekit.onnx`,
    `hey_jarvis.onnx`, `hey_mycroft.onnx`, `hey_rhasspy.onnx`). The threshold,
    noise floor, and cooldown come from `WAKE_WORD_THRESHOLD`,
    `WAKE_WORD_MIN_RMS`, and `WAKE_WORD_COOLDOWN_MS`. A missing or incompatible
    model logs a warning and skips wake detection instead of stopping startup.
12. Start a terminal stdin reader that sends typed lines as `UserInput` events.
13. Start the native `eframe` event loop unless `HYUSK_ORB`/the GNOME
    indicator say to skip it.

## Modules

| Module | Responsibility |
| --- | --- |
| `agent/runner.rs` | Owns agent state, provider selection, and the text-model tool-call loop. |
| `agent/runner/prompts.rs` | Builds system instructions, tool specs, user profile, and per-turn desktop context. |
| `agent/runner/reply.rs` | Decodes spoken response envelopes, filters tool preambles, and recognizes conversation controls. |
| `agent/runner/session.rs` | Persists sessions, repairs interrupted tool batches, and compacts history without splitting tool/result pairs. |
| `agent/runner/stream.rs` | Decodes streamed prose and response envelopes into speakable sentences while the reply is still arriving. |
| `agent/runner/sonic.rs` | Runs Nova Sonic conversations, tool dispatch, audio playback, and stream cleanup. |
| `agent/runtime.rs` | Consumes `HyuskEvent`s, runs post-wake STT, invokes the agent, emits responses, and calls TTS. |
| `agent/commands.rs` | Deterministic app/site/workflow command router and workflow validation. |
| `apps.rs` | Installed-application discovery for the workflow editor and aliases. |
| `credentials.rs` | Login-keyring lookup and storage for provider keys. |
| `link/` | Opt-in TLS WebSocket server, pairing auth, and phone command channel. |
| `status.rs` | Publishes `hyusk-status.json` for the GNOME and Noctalia indicators. |
| `model/openrouter.rs` | Sends OpenAI-compatible HTTP requests, handles model catalogs and retry policy. |
| `model/openrouter/protocol.rs` | Validates message roles, tool-call arguments, IDs, and complete result batches. |
| `model/openrouter/stream.rs` | Decodes SSE text/tool deltas, preserves stream errors and refusals, and bounds response memory. |
| `tools/tool.rs` | Defines the async `Tool` trait and `ToolResult`. |
| `tools/registry.rs` | Stores tools as `Arc<dyn Tool>` and exposes them by name. |
| `tools/computer.rs` | Controls the GUI with the portal/enigo backends, screenshots, OCR, and key/mouse helpers. |
| `tools/portal.rs` | Linux-only XDG RemoteDesktop and Screenshot portal integration. |
| `tools/browser.rs` | Controls a dedicated browser profile over CDP instead of screenshots. |
| `tools/accessibility.rs` | Drives AT-SPI2 through the `pyatspi` bridge. |
| `tools/window.rs` | Lists and activates windows over niri IPC or the extension's D-Bus. |
| `tools/gnome_doctor.rs` | Read-only checks for GNOME window, accessibility, and capture support. |
| `tools/memory.rs` | SQLite memory, FTS search, and knowledge-graph triples. |
| `tools/safety.rs` | Holds the explicit per-tool approval table (unknown names ask), classifies risky calls, and arms one-shot confirmations that expire. |
| `tools/scheduler.rs` | Persists one-shot reminders, workflows, and agent tasks. |
| `tools/task.rs` | Bounded research and Codex workspace workers. |
| `tools/timer.rs` | In-process countdown timers with desktop notifications. |
| `tools/web_search.rs` | Brave or keyless DuckDuckGo search without a browser. |
| `tools/mobile.rs` | Forwards typed actions to the paired Android phone. |
| `tools/shell.rs` | Executes commands through `sh -c`, with foreground and detached modes. |
| `tools/process.rs` | Spawns an executable directly and reports immediate failures. |
| `tools/media.rs` | Delegates MPRIS control to `playerctl`. |
| `types/message.rs` | Mirrors the chat-completions message, tool-call, and tool-result wire shape. |
| `types/event.rs` | Defines events exchanged by the agent, wake detector, STT path, and UI. |
| `types/state.rs` | Defines visual states consumed by the overlay. |
| `speech/speech.rs` | Contains Whisper microphone transcription and platform-specific TTS adapters. |
| `wake/detector.rs` | Captures microphone audio, resamples it, runs a LiveKit ONNX classifier, and exposes `WakeResume`. |
| `ui/orb.rs` | Consumes events and draws the animated butterfly overlay. |

### Where to change agent behavior

- **Wrong tool choice or unnecessary questions:** inspect `runner/prompts.rs`
  and the actual tool result first. `tools/safety.rs` decides real approval
  requirements; a failed executable or CDP timeout is not an approval request.
- **A task ends too early or repeats actions:** inspect the text loop in
  `runner.rs`, or the voice loop in `runner/sonic.rs`, together with the tool's
  output. Dispatch success alone is not proof that the user goal is complete.
- **Lost conversation or malformed tool history:** inspect
  `runner/session.rs` and `model/openrouter/protocol.rs`. Preserve call IDs,
  roles, and complete tool batches when editing compaction or cancellation.
- **Listening stays active or the microphone remains occupied:** inspect
  `runtime.rs` lifecycle tokens and `runner/sonic.rs` cleanup guards. Recorder
  timeout cancellation must not cancel the outer lifecycle or suppress its
  completion event. Dropping an async task handle does not stop its worker.
- **Browser editing or navigation:** inspect `tools/browser.rs`. Keep action
  errors separate from failed follow-up observations, since repeating an
  uncertain click or submission may duplicate the action.

The extracted modules are internal implementation details; callers still use
`agent::Agent` and `OpenRouterClient`. Existing session JSON versions remain
readable. No mobile architecture or session migration is implied by these
laptop changes.

For changes across these boundaries, run `cargo test --all-targets`,
`cargo fmt --all -- --check`, and the package's Clippy check.
Live microphone, desktop, and provider checks remain necessary in addition to
unit tests; a passing suite is not a guarantee of production readiness.

## Voice and wake path

Before a model-driven text turn, the runner adds a timestamped GNOME window
snapshot to the system prompt. It contains the focused window and up to twenty
open windows (application, bounded title, stable ID, and available workspace/
monitor state). The read times out after 750 ms and reports unknown state when
the extension is unavailable. It does not capture document contents or a
screenshot, and it does not infer that the user is actively working merely
because a window is open. The snapshot replaces the previous turn's observation;
it is not stored as a user memory.

Nova Sonic receives the same snapshot when its voice session starts, after
microphone capture has begun. During that continuing session the model uses
`window active/list` to refresh changing state. Context-relative commands such
as "search here" bypass the instant command router and use the model and UI
inspection tools. Focus metadata identifies a likely target; accessibility or
screen inspection is still required to find controls and verify actions.

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

The agent keeps working messages in `Vec<Message>`. Old complete turns and
verbose tool output are compacted to a budget derived from the selected model's
context window (or `HYUSK_AGENT_MAX_HISTORY_BYTES`), and the recent plain
user/assistant conversation is mirrored to a private `session.json` snapshot so
it survives a restart. Compaction never splits an assistant `tool_calls` message
from its results. Each turn records its starting index; cancellation truncates
back to it so a replacement turn never sees a dangling tool call. Durable facts
live separately in the SQLite memory store, not in the session snapshot.

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

`HyuskEvent` is a shared enum rather than a typed API. The main groups are:

- Input and voice: `WakeWordDetected`, `UserInput`, `VoiceTranscript`,
  `ListenCompleted`, `ContinueListening`, `AwaitReplyListening`, `StopListening`,
  and the scoped `SonicListenCompleted`/`WakeProbeFinished` results.
- Work: `RunWorkflow`, `ScheduledWorkflow`, `ScheduledAgentInput`,
  `ScheduledAgentTask`, `WorkflowFinished`, `SubtaskFinished`, `TurnFinished`.
- UI and control: `StateChanged`, `ToolStarted`, `ToolFinished`, `Response`,
  `ResponseDelta`, `StopRequested`, `ApprovalExpired`, `ModelSelected`,
  `ApiKeySaved`, `Shutdown`.

Ids on the async variants let the runtime drop a result from a superseded
listen session or turn.

`run_agent_task` handles wake events and `UserInput`, cancels the previous turn,
and spawns each turn as a separate task. `OrbApp` drains events on each frame
and changes its visual state, including `Waking` and `Listening`. The overlay is
a 160 x 160 transparent window with a black circle and always-on-top window
level. It does not request keyboard focus by default (`ORB_STEAL_FOCUS=0`), so
portal keyboard input reaches the controlled application. The orb itself does
not render `Response` text; the GNOME response card and the Noctalia tooltip do,
including Approve/Deny for armed actions.

## Model protocol and failure handling

The client posts to the selected provider's OpenAI-compatible base URL:

```text
{OPENROUTER_BASE_URL}/chat/completions	https://api.openai.com/v1/chat/completions
https://bedrock-mantle.{region}.api.aws/v1/chat/completions
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

- a sandbox for approved actions (approvals are a guardrail, not confinement);
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
- response text inside the `eframe` orb (the GNOME and Noctalia indicators
  render it; the orb stays a status visual);
- a stable client API or IPC protocol. The runtime status and control files are
  intentionally small, versioned by a monotonic `revision`, and may change.

Those boundaries are useful to preserve while extending the crate: keep model
translation, tool execution, event coordination, and desktop/audio adapters
separate rather than putting all behavior in `main.rs`.
