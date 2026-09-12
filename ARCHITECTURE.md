# Architecture

`hyusk_agent` is an experimental composition of an OpenRouter-compatible agent,
local tools, event-driven Tokio tasks, optional audio components, and a small
`eframe` overlay.

The detailed module and event walkthrough is in
[`docs/architecture.md`](docs/architecture.md).

## At a glance

```text
main.rs
  |
  +-- OpenRouterClient -> Agent -> ToolRegistry -> computer/shell/process/media
  |
  +-- MPSC channels -> run_agent_task -> cancellable turn -> Response -> TextToSpeech
  |
  +-- WakeWordDetector -> WakeWordDetected -> SpeechToText -> UserInput
  |
  +-- eframe UI <- state/tool/response events
```

## Current boundaries

- `agent/runner.rs` owns in-memory chat history and the structured tool-call
  loop.
- `agent/runtime.rs` consumes `HyuskEvent`s, cancels the previous turn, and
  spawns each turn as a separate task.
- `model/openrouter.rs` handles the HTTP protocol and transient retries.
- `tools/` contains the tool trait, registry, and four local tools.
- `speech/`, `wake/`, and `ui/` are audio and desktop refinement components.

The voice path is connected when the wake model and Whisper model are available:
`WakeWordDetector` pauses after `WakeWordDetected`, `run_agent_task` records and
transcribes the command with `SpeechToText`, resumes the detector, and emits
`UserInput`. A new typed message or wake word cancels the active turn and its
Linux TTS playback. The temporary `models/hey_livekit.onnx` classifier is
selected automatically when `models/hey_hyusk.onnx` is absent. The overlay draws
a black circular orb but does not render response text. See
[`docs/voice-and-ui.md`](docs/voice-and-ui.md) for the exact behavior.

## Runtime characteristics

The agent uses structured OpenAI-compatible function calling, preserves the
assistant `tool_calls` message before each `role: "tool"` result, and keeps the
conversation in memory for the life of the process. Turns are cancellable, but
there is no persistence, streaming response, permission system, or process
monitor in this crate.

The `shell` tool runs arbitrary commands as the current user. The `computer`
tool controls the real pointer and keyboard and attempts screenshots through
local backends. See [`docs/tools.md`](docs/tools.md) before exposing this binary
to untrusted prompts or accounts.
