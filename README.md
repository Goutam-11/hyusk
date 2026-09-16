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
| Persistent memory | Implemented; SQLite + FTS5 hybrid retrieval, knowledge-graph triples, readable Markdown export |
| System context in prompt | Implemented; OS, distro, session type, host, date/time |
| Wake-word detection | Optional; uses `hey_hyusk.onnx` or the temporary `hey_livekit.onnx` |
| Speech-to-text | Connected to the wake flow when a Whisper model loads |
| Text-to-speech | Invoked and awaited after responses; depends on local tools |
| Guarded long-running model/tool loops | Implemented; configurable round/time limits, cancellation, and no-progress detection |
| Background model/Codex tasks | Implemented; bounded concurrency with list, cancel, completion cards, and announcements |
| Persistent reminders and workflow schedules | Implemented; one-shot jobs survive service restarts |

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
| `OPENAI_API_KEY` | No | - | Separate OpenAI API credential; API usage is billed separately from a ChatGPT subscription. |
| `BRAVE_SEARCH_API_KEY` | No | DuckDuckGo fallback | Optional token for Brave's structured Search API. |
| `HYUSK_WORKSPACE` | No | Current directory | Workspace root for Codex CLI coding mode. |
| `MODEL_VISION` | No | `1` | Attach screenshots as image content for vision-capable models. Set `0` for text-only models, then use `ocr` instead. |
| `ORB_STEAL_FOCUS` | No | `0` | Let the orb take keyboard focus while active. Default is off so portal keyboard input goes to the controlled app. |
| `HYUSK_ORB` | No | auto | Set `0` to disable the orb, `1` to force it. By default the orb is skipped when the GNOME indicator extension is enabled. |
| `HYUSK_SCREENSHOT_RETENTION_SECS` | No | `900` | Delete temporary screenshots older than this. |
| `HYUSK_SCREENSHOT_MAX_FILES` | No | `30` | Maximum temporary screenshots retained. |
| `HYUSK_AGENT_MAX_TOOL_ROUNDS` | No | `24` | Maximum model/tool rounds in one foreground turn. |
| `HYUSK_AGENT_MAX_TURN_SECS` | No | `600` | Wall-clock limit for one foreground turn, including tools. |
| `HYUSK_AGENT_MAX_REPEATED_TOOL_ROUNDS` | No | `3` | Stop after this many identical tool-call/result rounds. |
| Background subagents | No configuration | At most two workers. Research is read-only; workspace tasks use local Codex CLI with a 15-minute limit. |
| `WAKE_CLAP_ENABLED` | No | `0` | Enable hand-clap wake (off by default; noise triggers it). |
| `WAKE_CLAP_SENSITIVITY` | No | `6.0` | How many times louder than background a clap must be. |
| `WAKE_CLAP_MIN_PEAK` | No | `0.03` | Absolute peak floor for a clap. |
| `WAKE_WORD_ENABLED` | No | `1` | Set to `0`, `false`, `off`, or `no` to skip wake detection. |
| `WAKE_WORD_MODEL` | No | `models/hey_hyusk.onnx`, then fallbacks | LiveKit-compatible classifier. |
| `WAKE_WORD_MODELS` | No | - | Comma-separated classifiers; highest score wins. Overrides `WAKE_WORD_MODEL`. |
| `WAKE_WORD_THRESHOLD` | No | `0.93` | Detection confidence threshold. Lower is more sensitive. |
| `WAKE_WORD_STRONG_THRESHOLD` | No | `0.94` | A single score at or above this level wakes immediately. |
| `WAKE_WORD_MIN_HITS` | No | `1` | Matching scores required before waking. One minimizes latency and avoids cutting off speech. |
| `WAKE_WORD_HIT_WINDOW_MS` | No | `1000` | Time window in which confirmation hits must arrive. |
| `WAKE_WORD_MIN_RMS` | No | `0.003` | Adaptive noise-gate floor. Lower accepts quieter speech. |
| `WAKE_WORD_COOLDOWN_MS` | No | `1500` | Minimum milliseconds between detections. |
| `WAKE_WORD_DENOISE` | No | `0` | Optional RNNoise preprocessing. Leave off for models made by the bundled raw-audio trainer. |
| `STT_MODEL` | No | `models/ggml-base.en.bin` | Whisper GGML model for post-wake transcription. |
| `STT_LANGUAGE` | No | `en` | Whisper language code, or `auto` with a multilingual model. |
| `STT_DENOISE` | No | `1` | Apply RNNoise to command audio before Whisper transcription. |
| `STT_SILENCE_MS` | No | `900` | Silence after speech before recording stops. |
| `STT_NO_SPEECH_MS` | No | `4000` | Give up and stop early if no speech starts. |
| `PIPER_MODEL` | No | `models/en_US-lessac-medium.onnx` | Piper voice model used by Linux TTS. |
| `PIPER_SPEAKER` | No | - | Speaker name or numeric ID for a multi-speaker Piper model. |

The base URL must not include the trailing `/chat/completions` path.

## Start automatically and view logs

On Fedora GNOME, install Hyusk as a per-user graphical-session service:

```bash
./scripts/install-autostart.sh
```

It builds the release binary, starts Hyusk after graphical login, restarts it
after crashes, and sends SIGINT during logout or shutdown so it can stop cleanly.
The service preserves the existing component-tagged logs in journald:

```bash
systemctl --user status hyusk.service
journalctl --user -u hyusk.service -f       # live logs
journalctl --user -u hyusk.service -b       # this boot
```

Disable it with `./scripts/uninstall-autostart.sh`.

## Tools

The agent exposes its tools through OpenAI-compatible function specs. In
addition to desktop, process, media, memory, and timer actions, it includes
managed background tasks and persistent scheduling.

### `scheduler`

Creates one-time reminders or runs named deterministic workflows later. Jobs
are stored atomically in `$XDG_DATA_HOME/hyusk/schedules.json`, reloaded after a
restart, and can be listed or cancelled by ID:

```json
{ "action": "schedule", "type": "reminder", "text": "stretch", "after_seconds": 600 }
{ "action": "schedule", "type": "workflow", "workflow": "focus mode", "at_unix": 1900000000 }
{ "action": "schedule", "type": "agent_task", "prompt": "summarize today's notes", "after_seconds": 3600 }
{ "action": "list" }
{ "action": "cancel", "id": 2 }
```

Common relative phrases are handled without an LLM, including `remind me in
ten minutes to stretch`, `remind me to drink water in thirty minutes`, and
`schedule workflow focus mode in one hour`. A scheduled workflow that becomes
due during another foreground request is queued instead of interrupting it.

### `task`

Delegates longer model work while the main assistant remains responsive.
Research workers cannot use local tools; workspace workers use the installed
Codex CLI inside one explicitly approved project root. Use `list` and `cancel`
to manage running jobs, and optionally set `timeout_seconds` between thirty
seconds and one hour. Hyusk posts the report and announces completion when a
worker finishes.

### `web_search`

Searches the live web directly without opening a browser or using screenshots.
It returns titles, source URLs, and snippets for the model to summarize. The
keyless fallback uses DuckDuckGo's non-JavaScript HTML search. For more stable
API results, set `BRAVE_SEARCH_API_KEY` or store it in GNOME Keyring:

```bash
secret-tool store --label="Hyusk Brave Search" hyusk provider brave_search
```

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

### `task`

Delegates independent work while Hyusk remains available. The default
`research` profile is read-only and has no local tools, memory, desktop access,
or credentials. The `workspace` profile gives an already-installed and signed-in
Codex CLI a single approved project root in `workspace-write` sandbox mode for
code inspection and edits. Starting either profile requires explicit user
confirmation.

The GNOME indicator displays the latest response, completion, or error in a
small card even when audio is muted. Its Model menu uses a cached provider
catalog (refreshed every 24 hours or manually) to switch OpenRouter, OpenAI API,
or Codex CLI workspace mode. Switching starts a fresh conversation; the choice
is stored in `$XDG_CONFIG_HOME/hyusk/config.json`.

The GNOME top-bar indicator includes **Stop Hyusk**, which cancels the active
foreground turn and listening session.

### Instant voice commands

Common commands are handled locally, with no model round trip, so they run in
well under a second:

- `open Firefox`, `open Brave`, `open Calculator`, ...
- `open YouTube`, `go to github.com`, `search lofi beats on youtube`
- `switch to Code`, `show Firefox`
- `next song`, `pause`, `volume up`, `set volume to 30`
- `play something`, `play lofi beats` (plays via `mpv` + `yt-dlp` when
  installed, otherwise opens a YouTube Music search)
- `lock the screen`, `screenshot`, `take a note buy milk`
- chains: `open Brave and go to YouTube`

Built-in aliases live in `src/agent/commands.rs`; add your own in
`~/.config/hyusk/commands.json`:

```json
{
  "apps":  { "notes": ["flatpak", "run", "com.example.Notes"] },
  "sites": { "hn": "https://news.ycombinator.com" }
}
```

#### Native voice workflows (no LLM)

The same file can define Shortcuts-style workflows. A workflow matches its
name, any phrase in `phrases`, or `run <name>` / `start <name>`, then executes
each `steps` entry locally in order:

```json
{
  "workflows": [
    {
      "name": "start my workday",
      "phrases": ["begin work", "open my work setup"],
      "steps": [
        "open code",
        "open slack",
        "set timer for 25 minutes"
      ]
    }
  ]
}
```

Workflow steps use the built-in deterministic command vocabulary, so they do
not require an API key, model call, or screenshots. They can launch configured
apps, open sites, switch windows, control media, set timers, change brightness,
lock the session, take screenshots, and save notes. Arbitrary shell text is not
accepted as a workflow step. Dangerous operations continue to require the
normal Hyusk confirmation path when invoked through the model.

Each `steps` item is one command; a semicolon can also separate commands in a
single item. The supported forms include `open <app-or-site>`, `launch
<app>`, `go to <site>`, `switch to <window>`, `play`/`pause`/`next song`,
`volume up`/`volume down`/`set volume to <0-100>`, `set timer for <number>
<seconds|minutes|hours>`, `brightness up`/`brightness down`, `lock the screen`,
`screenshot`, and `remember that <text>`. App and site aliases from the same
file are available inside workflows, so add an alias before using a custom
application or site.

Hyusk validates a workflow before executing it and never runs only part of a
broken workflow. Entries without a usable name are ignored when the
configuration is loaded; a named empty or malformed workflow stays addressable
so Hyusk can say what needs fixing. Exact duplicate entries are ignored, while
duplicate triggers use the first complete workflow.

#### Hyusk Workflows desktop app

Install the native GTK4/libadwaita editor for the current user:

```bash
./scripts/install-workflow-app.sh
```

Open **Hyusk Workflows** from the GNOME app grid, run
`hyusk-workflows`, or choose **Workflows** from the Hyusk top-bar menu.
The app provides a searchable workflow library, voice-trigger editing,
installed-app discovery and search, custom application commands, a website
builder with browser choice, ordered steps with move/remove controls,
validation, atomic saves, delete confirmation, one-time workflow/reminder
scheduling, and direct execution through the running Hyusk service. It
preserves the existing `apps`, `sites`, and other keys in
`~/.config/hyusk/commands.json`.

The app and voice path use the same version-one workflow format. Editing and
running workflows does not require an LLM or API key.

Anything the router does not recognize goes to the model as before.

**`window` tool.** Native window control on GNOME via the
`org.hyusk.Shell` D-Bus interface in the extension: `list`, `active`, and
`activate` (by app or title substring). This is the reliable way to switch
apps on Wayland. Install/refresh the extension with
`scripts/install-gnome-indicator.sh`, then log out and back in.

### `memory`

Persistent local memory that survives restarts. SQLite `memory.db` is the
source of truth under `$XDG_DATA_HOME/hyusk/` (or `~/.local/share/hyusk/`), and
`memory.md` is a generated human-readable view. Existing `memory.json` data is
imported transactionally on first use and kept unchanged as a backup.

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
wins. The detector also tracks an adaptive noise floor and peak-normalizes each
window. Runtime preprocessing should match training: personal models made by
the bundled trainer should normally use `WAKE_WORD_DENOISE=0`.

For a personal model, record plenty of *hard negatives*: phrases that resemble
the target (`hey Siri`, `hey Google`, `Alexa`, `hey you`, `hi Hyusk`, `hey
Musk`) plus the television, music, keyboard, and room noise that causes real
false wakes. Recordings are checkpointed, so increasing `--negatives` resumes
the existing set instead of asking for the positive clips again:

```bash
python3 scripts/train-wake-word.py --word "hey hyusk" \
  --output models/hey_hyusk.onnx --positives 60 --negatives 180
```

You can also add a bounded public speech set instead of manually recording all
general negatives. This downloads TensorFlow Mini Speech Commands (about 182
MB compressed, 8,000 short clips from many speakers), while the trainer samples
only 2,000 clips by default:

```bash
./scripts/download-wake-negatives.sh

.venv-wake/bin/python scripts/train-wake-word.py \
  --word "hey hyusk" \
  --output models/hey_hyusk.onnx \
  --positives-dir models/wake_recordings/hey_hyusk/positives \
  --negatives-dir models/wake_recordings/hey_hyusk/negatives \
  --extra-negatives-dir models/public_negatives/mini_speech_commands \
  --extra-negative-limit 2000 \
  --skip-install
```

Keep the personal negatives in the mix: public files provide broad voices and
words, while your own false activations represent the laptop microphone, room,
speakers, and confusing phrases the deployed detector actually hears.

One qualifying score is required by default, so command recording starts as
soon as the detector crosses the threshold. Increase `WAKE_WORD_MIN_HITS` only
if your microphone produces too many isolated false positives.

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
HYUSK_TIMING=1 cargo run              # per-stage latency (record, model, TTS)
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
