# hyusk_agent

`hyusk_agent` is an experimental single-binary agent runtime for the Hyusk
project. It combines an OpenRouter-compatible chat model, structured function
calling, local process tools, an event-driven agent task, a small `eframe`
status overlay, and optional voice components.

This directory is a member of the parent Hyusk workspace. It is separate from
the main `hyusk` CLI in `../crates/hyusk-cli`; see the workspace
[`README.md`](../README.md) for the broader project.

## Current status

| Area | State |
| --- | --- |
| OpenRouter chat and structured tool calling | Implemented |
| Shell, process, media, and computer tools | Implemented |
| Event-driven agent runtime and in-memory history | Implemented |
| Animated desktop status overlay | Implemented, visual only |
| Desktop/GUI control | Implemented; GNOME/Wayland uses the desktop portal with a one-time approval |
| Accessibility control | Implemented; AT-SPI2 via a `pyatspi` bridge for fast app control |
| Interruptible async turns | Implemented; a new message or wake word cancels the current model/tool/TTS work |
| Circular desktop orb | Implemented; optional when the GNOME top-bar indicator is installed |
| GNOME top-bar indicator | Implemented as a Shell extension in `gnome-extension/` |
| Persistent memory | Implemented; Markdown + JSON notes, knowledge-graph triples, query-aware tf-idf retrieval |
| System context in prompt | Implemented; OS, distro, session type, host, date/time |
| Wake-word detection | Optional; uses `hey_hyusk.onnx` or the temporary `hey_livekit.onnx` |
| Speech-to-text | Connected to the wake flow when a Whisper model loads |
| Text-to-speech | Invoked and awaited after responses; depends on local tools |
| Persistent sessions, streaming, permissions, cancellation | Not implemented here |

The `models/` directory currently contains Whisper and Piper assets plus the
downloaded temporary `hey_livekit.onnx` classifier. The detector is optional:
startup logs a warning and continues in text/GUI mode when no classifier is
available. Downloads are handled by `scripts/download-wake-word.sh`.

`WAKE_WORD_MODEL` selects the classifier. When it is unset and
`models/hey_hyusk.onnx` is absent, `models/hey_livekit.onnx` is used
automatically.
Set `WAKE_DEBUG=1` to print live audio level, gate, and per-classifier
confidence scores while tuning the detector. STT is initialized from `STT_MODEL` and falls back to text-only
mode when it cannot load.

## Quick start

From this directory:

```bash
cd hyusk_agent
cp .env.example .env
# Edit .env, or export the variables in your shell.
cargo check
cargo run
```

The required chat configuration is:

```bash
export OPENROUTER_API_KEY='your-api-key'
export OPENROUTER_MODEL='openai/gpt-4o-mini'
export OPENROUTER_BASE_URL='https://openrouter.ai/api/v1'
```

`dotenvy` loads `.env` when present. Environment variables already set in the
shell take precedence. Keep `.env` local; it is ignored by Git.

Running the binary starts these components:

1. Load configuration and create the OpenRouter client.
2. Register the `computer`, `shell`, `process`, and `media` tools.
3. Check for a default microphone.
4. Initialize speech-to-text from `STT_MODEL` when available.
5. Start the agent event task.
6. Start the wake-word task when enabled and the model is present.
7. Start a terminal stdin reader for typed messages.
8. Open the 140 x 140 transparent, always-on-top `eframe` window.

The agent receives `HyuskEvent::UserInput` events through an MPSC channel. Typed
terminal lines are the always-available text path. When wake detection fires,
the runtime pauses the detector, records and transcribes a short command with
`SpeechToText`, resumes the detector, and sends `UserInput`. If STT cannot load
or times out, the wake event is ignored and the terminal input path still works.

## Configuration

| Variable | Required | Default | Purpose |
| --- | --- | --- | --- |
| `OPENROUTER_API_KEY` | Yes | - | Bearer token for the chat provider. |
| `OPENROUTER_MODEL` | No | `openai/gpt-4o-mini` | Model identifier sent to the provider. |
| `OPENROUTER_BASE_URL` | No | `https://openrouter.ai/api/v1` | Base URL; `/chat/completions` is appended. |
| `MODEL_VISION` | No | `1` | Attach screenshots as image content for vision-capable models. Set `0` for text-only models, then use `ocr` instead. |
| `ORB_STEAL_FOCUS` | No | `0` | Let the orb take keyboard focus while active. Default is off so portal keyboard input goes to the controlled app. |
| `HYUSK_ORB` | No | auto | Set `0` to disable the orb, `1` to force it. By default the orb is skipped when the GNOME indicator extension is enabled. |
| `HYUSK_SCREENSHOT_RETENTION_SECS` | No | `900` | Delete temporary screenshots older than this. |
| `HYUSK_SCREENSHOT_MAX_FILES` | No | `30` | Maximum temporary screenshots retained. |
| `WAKE_CLAP_ENABLED` | No | `0` | Enable hand-clap wake (off by default; noise triggers it). |
| `WAKE_CLAP_SENSITIVITY` | No | `6.0` | How many times louder than background a clap must be. |
| `WAKE_CLAP_MIN_PEAK` | No | `0.03` | Absolute peak floor for a clap. |
| `WAKE_WORD_ENABLED` | No | `1` | Set to `0`, `false`, `off`, or `no` to skip wake detection. |
| `WAKE_WORD_MODEL` | No | `models/hey_hyusk.onnx`, then fallbacks | LiveKit-compatible classifier. |
| `WAKE_WORD_MODELS` | No | - | Comma-separated classifiers; highest score wins. Overrides `WAKE_WORD_MODEL`. |
| `WAKE_WORD_THRESHOLD` | No | `0.4` | Detection confidence threshold. Lower is more sensitive. |
| `WAKE_WORD_MIN_RMS` | No | `0.003` | Adaptive noise-gate floor. Lower accepts quieter speech. |
| `WAKE_WORD_COOLDOWN_MS` | No | `1500` | Minimum milliseconds between detections. |
| `WAKE_WORD_DENOISE` | No | `1` | Run RNNoise before wake scoring to suppress fan/hiss noise. |
| `STT_MODEL` | No | `models/ggml-base.en.bin` | Whisper GGML model for post-wake transcription. |
| `PIPER_MODEL` | No | `models/en_US-lessac-medium.onnx` | Piper voice model used by Linux TTS. |

The base URL must not include the trailing `/chat/completions` path.

## Tools

The agent exposes six tools through OpenAI-compatible function specs:

### `shell`

Runs a command through `sh -c`.

```json
{ "command": "pwd", "mode": "foreground" }
```

`foreground` is the default and waits for completion. `detached` starts the
command and returns its PID without waiting.

### `process`

Spawns a program directly, without a shell.

```json
{ "action": "launch", "program": "firefox", "args": [] }
```

The tool waits 300 ms to report an immediate launch failure, then returns the
PID if the process is still running.

### `media`

Controls an MPRIS media player through the external `playerctl` executable.

```json
{ "action": "status" }
```

Supported actions are `play`, `pause`, `play_pause`, `next`, `previous`,
`stop`, `status`, `volume`, `volume_up`, and `volume_down`. The `volume`
action accepts an integer `level` from 0 through 100.

### `computer`

Controls the local GUI without going through the shell. It supports status and
screenshot/OCR inspection, mouse movement/click/drag, scrolling, typing, key
presses, key combos, and short waits. Coordinates are global pixels with the
origin at the top-left.

```json
{ "action": "screenshot" }
{ "action": "screens" }
{ "action": "find_text", "text": "Search" }
{ "action": "click_text", "text": "Search" }
{ "action": "cursor" }
{ "action": "mouse_move", "x": 640, "y": 360 }
{ "action": "mouse_click", "button": "left" }
{ "action": "type_text", "text": "hello" }
{ "action": "key_combo", "keys": ["ctrl", "l"] }
{ "action": "clipboard_set", "text": "message to paste" }
{ "action": "open_url", "url": "https://web.whatsapp.com" }
{ "action": "focus_window", "title": "Visual Studio Code" }
```

Use `batch` to run several actions in one model round-trip, which makes
multi-step GUI workflows much faster:

```json
{
  "action": "batch",
  "steps": [
    { "action": "open_url", "url": "https://web.whatsapp.com" },
    { "action": "wait", "milliseconds": 4000 },
    { "action": "screenshot" }
  ]
}
```

For VS Code, the `process` tool can open a folder directly:

```json
{ "action": "launch", "program": "code", "args": ["/path/to/folder"] }
```

On Linux Wayland the tool uses the XDG RemoteDesktop portal. The first input
action opens a permission dialog; approve it once and the session is reused for
the rest of the process. X11 and Wayland compositors with virtual input
protocols use the native `enigo` backend. Screenshot capture tries `grim`,
GNOME Shell D-Bus, and the Screenshot portal for Wayland, then ImageMagick
`import`/`scrot` for X11. Temporary screenshots are cleaned up automatically
(`HYUSK_SCREENSHOT_RETENTION_SECS`, `HYUSK_SCREENSHOT_MAX_FILES`). When
`MODEL_VISION=1` (default), screenshots are attached to the next model turn as
real image content, so a vision model can inspect them directly; `ocr` remains
available as a fallback. Set `MODEL_VISION=0` for text-only models. See
[docs/tools.md](docs/tools.md) for the full action list.

### `accessibility`

Controls GUI applications through Linux **AT-SPI2** using a Python `pyatspi`
bridge spawned by Rust. For supported apps this is much faster and more precise
than screenshots.

```json
{ "action": "active", "read": true }
{ "action": "apps" }
{ "action": "windows" }
{ "action": "find", "app": 3, "name": "Search", "role": "push button" }
{ "action": "find", "text": "Sign in" }
{ "action": "click", "path": [3, 0, 1, 2] }
{ "action": "set_text", "path": [3, 0, 1, 4], "text": "hello" }
{ "action": "tree", "app": 3, "max_depth": 4 }
```

Actions: `active`, `apps`, `windows`, `tree`, `find`, `click`, `focus`,
`set_text`, `get_text`, `read`. `active` reports the focused app, window, and
element (and with `read`, the visible text) — the fastest way to orient. `find`
can match by `name`, `role`, and/or visible `text`.

Requires `python3` with `pyatspi` and a running AT-SPI bus. Apps only appear if
they publish an accessibility tree, so run the setup once:

```bash
./scripts/setup-accessibility.sh          # toolkit-accessibility + Chromium/Electron env
./scripts/setup-accessibility.sh --check  # show what AT-SPI currently sees
```

Then restart the apps (and re-login once). Responses include a `warnings` field
when the environment limits visibility; the agent falls back to the `computer`
tool for apps that publish no tree (e.g. Chromium/Electron without
`ACCESSIBILITY_ENABLED=1`).

### `memory`

Persistent local memory that survives restarts. Data is stored under
`$XDG_DATA_HOME/hyusk/` (or `~/.local/share/hyusk/`) as `memory.json` and a
human-readable `memory.md`.

```json
{ "action": "remember", "text": "User prefers dark mode", "tags": ["preference"] }
{ "action": "search", "query": "dark mode" }
{ "action": "graph_add", "subject": "user", "relation": "uses", "object": "GNOME Wayland" }
{ "action": "graph_query", "subject": "user" }
{ "action": "list" }
{ "action": "forget", "id": 3 }
```

`search` ranks notes with a small tf-idf score (tag matches weigh more). Each
turn retrieves the notes that best match the user's message and injects them
into the system prompt, so the agent sees what is relevant to the current
request. Duplicate notes and graph triples are rejected, and the prompt asks the
agent to save durable facts about the user proactively.

See [docs/tools.md](docs/tools.md) for schemas, examples, and tool-extension
instructions.

## Temporary wake word

`scripts/download-wake-word.sh` downloads the LiveKit `hey_livekit.onnx`
classifier and the OpenWakeWord `alexa.onnx` classifier (converted to the
LiveKit tensor names). Both are auto-discovered from `models/`, so you can say
**"Hey LiveKit"** or **"Alexa"**. This is a temporary stand-in until a custom
"Hey Hyusk" classifier is trained; the models are not part of the repository
and are downloaded on demand.

Multiple classifiers can be loaded with `WAKE_WORD_MODELS`; the highest score
wins. The detector also tracks an adaptive noise floor, runs RNNoise denoising
before scoring (set `WAKE_WORD_DENOISE=0` to disable), and peak-normalizes each
window, so quiet or distant speech is still scored.

## Interruptions and UI

`run_agent_task` keeps receiving events while a turn is running. A new typed
message, or the wake word, cancels the in-flight model request, tool call, and
Linux TTS playback, then starts the replacement turn. The conversation rolls
back to the state before the cancelled turn, so partial tool-call messages are
not replayed.

There are two optional status UIs:

- a 160 x 160 transparent orb window with a black circle; and
- a GNOME Shell top-bar indicator with a color-coded butterfly.

Install the GNOME indicator with:

```bash
./scripts/install-gnome-indicator.sh
```

Then log out and back in, and enable **Hyusk Agent Indicator** in the
Extensions app. The agent writes its state to `$XDG_RUNTIME_DIR/hyusk-state`;
the extension polls that file and colors the butterfly per state. Once the
extension is enabled, the orb is skipped automatically. Use `HYUSK_ORB=1` to
force the orb, or `HYUSK_ORB=0` to force indicator-only mode.

The orb requests the top-center position and always-on-top while active, but it
does not take keyboard focus by default (`ORB_STEAL_FOCUS=0`). That matters
because keyboard events follow the focused window; if the orb stole focus,
portal `type_text`/`key_press` would go to the orb instead of the browser or
editor. GNOME Wayland does not let ordinary clients choose absolute window
positions, so the compositor may place the orb elsewhere.

## Agent and model flow

```text
UserInput event
      |
      v
Agent::handle
      |
      v
OpenRouter /chat/completions
      |
      +-- final text ----------------> Response event -> TTS
      |
      +-- tool_calls -> ToolRegistry -> tool result
                                  |
                                  +-> append role: tool message
                                  +-> request another model turn
```

`OpenRouterClient` sends the complete in-memory message history and the tool
specs on every turn. It retries network failures, HTTP 408, HTTP 429, and 5xx
responses up to three retries with exponential backoff. It honors a numeric
`Retry-After` header and fails fast for ordinary 4xx responses.

Message history is process-local. It is not persisted across restarts.

## Project layout

```text
hyusk_agent/
├── .env.example
├── Cargo.toml
├── README.md
├── ARCHITECTURE.md
├── CHANGELOG.md
├── docs/
│   ├── README.md
│   ├── architecture.md
│   ├── development.md
│   ├── tools.md
│   └── voice-and-ui.md
├── gnome-extension/
│   └── hyusk@hyusk.local/
│       ├── metadata.json
│       └── extension.js
├── scripts/
│   ├── download-wake-word.sh
│   └── install-gnome-indicator.sh
├── models/
│   ├── en_US-lessac-medium.onnx
│   ├── en_US-lessac-medium.onnx.json
│   ├── ggml-base.en.bin
│   ├── ggml-small.en.bin
│   └── hey_livekit.onnx
└── src/
    ├── agent/       # Agent loop and MPSC runtime task
    ├── model/       # OpenRouter-compatible HTTP client
    ├── speech/      # Whisper STT and local TTS adapters
    ├── tools/       # Tool trait, registry, computer, shell, process, media
    ├── types/       # Chat messages and Hyusk events/state
    ├── ui/          # Transparent animated eframe overlay
    ├── wake/        # LiveKit wake-word microphone loop
    └── main.rs      # Composition root and startup sequence
```

## Development commands

From the workspace root:

```bash
cargo check -p hyusk_agent
cargo test -p hyusk_agent
cargo run -p hyusk_agent
```

From this directory, omit `-p hyusk_agent`. `cargo check --all-targets`
currently passes. The workspace formatting check reports existing formatting
diffs in the newly added UI and audio modules; formatting is not yet a clean
gate.

Dependencies are compiled with optimizations even in debug builds (workspace
`[profile.dev.package."*"]`), so `cargo run` is fast enough for realtime
wake-word inference. Building the dependencies the first time after a profile
change takes a few minutes.

Wake-word diagnostics:

```bash
HYUSK_WAKE_TEST=30 cargo run          # live level/gate/score output
HYUSK_WAKE_TEST=30 HYUSK_WAKE_CAPTURE=/tmp/wake.raw cargo run
HYUSK_STT_TEST=1 cargo run            # record + transcribe without a wake word
```

If wake detection misses, clips, or falsely fires, calibrate the microphone
gain first:

```bash
./scripts/set-mic-gain.sh             # 48% capture, 0% mic boost
```

## Safety and troubleshooting

The `shell` tool executes arbitrary commands as the current user, and the
`computer` tool can move the real cursor, click, and type. There is no
permission prompt, sandbox, timeout, or cancellation token in this crate. Use a
trusted model/provider and run Hyusk in an account or environment with only the
permissions it should have.

Common failures:

- `OPENROUTER_API_KEY is missing`: set the variable or create `.env` from
  `.env.example`.
- `Wake-word detector disabled`: add a compatible LiveKit classifier at
  `WAKE_WORD_MODEL`, or set `WAKE_WORD_ENABLED=0` to skip it intentionally.
- Wake word not detected, or the log warns `Microphone is clipping`:
  the capture path is too hot or too quiet. Run `./scripts/set-mic-gain.sh`
  and, if needed, adjust the threshold with `WAKE_WORD_THRESHOLD`. Use
  `HYUSK_WAKE_TEST=30 cargo run` to see live levels and scores.
- Clap wake does nothing: hand-clap wake is off by default. Set
  `WAKE_CLAP_ENABLED=1` if you want it.
- `Speech-to-text disabled`: set `STT_MODEL` to a compatible Whisper GGML file or
  install the model at the default path.
- `Screenshot capture failed`: no backend succeeded. On GNOME Wayland approve
  the Screenshot portal dialog or install a compatible helper. The tool reports
  every backend error it saw.
- `RemoteDesktop portal` input failures: approve the portal dialog. If it was
  denied, restart the binary to clear the cached denial. X11 and Wayland
  compositors with virtual input protocols use `enigo` instead.
- `No default microphone found`: connect and select an input device supported
  by CPAL.
- `Failed to invoke playerctl`: install `playerctl` or remove `MediaTool` from
  the registration in `main.rs`.
- `Piper model not found`: install Piper and set `PIPER_MODEL`, or provide the
  bundled model at the default path.
- HTTP 429 or 5xx: the client retries automatically; sustained rate limits
  require waiting or changing models.

## License

MIT
