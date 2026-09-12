# Tool platform

Tools are asynchronous capabilities registered by name. The model sees a JSON
Schema for each tool, while the implementation receives the raw JSON argument
string and decides how to validate it.

## Tool contract

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult>;
}
```

`ToolResult` contains `success`, `output`, and an optional `error`.
`as_agent_message()` converts it into the text placed in a `role: "tool"`
message. A Rust error from `execute` is converted into a failed `ToolResult` by
the agent runner, so one bad tool does not terminate the agent task.

`ToolRegistry` stores `Arc<dyn Tool>` values in a `HashMap`. It supports
registration, lookup by name, and iteration for building model tool specs.

## Computer

`ComputerTool` controls the local desktop without going through a shell. It is
implemented in `src/tools/computer.rs` with a Linux portal backend in
`src/tools/portal.rs`.

On Linux Wayland it uses the XDG RemoteDesktop portal. The first input action
opens a permission dialog; approval is persisted for the rest of the process.
Relative pointer motion and keyboard keysyms are used because a
RemoteDesktop-only session has no PipeWire stream for absolute motion. The tool
seeds a pointer position from the XWayland pointer query when available and
tracks it, so `mouse_move` can still accept absolute coordinates.

On X11 and Wayland compositors with virtual keyboard/pointer protocols, the
native `enigo` backend is used.

```json
{ "action": "status" }
{ "action": "screenshot", "include_cursor": false }
{ "action": "screenshot", "region": { "x": 0, "y": 0, "width": 640, "height": 360 } }
{ "action": "ocr", "path": "/tmp/shot.png" }
{ "action": "mouse_move", "x": 640, "y": 360 }
{ "action": "mouse_move", "x": 40, "y": -20, "relative": true }
{ "action": "mouse_click", "button": "right", "x": 640, "y": 360, "count": 2 }
{ "action": "mouse_drag", "from": { "x": 10, "y": 10 }, "to": { "x": 300, "y": 300 } }
{ "action": "scroll", "axis": "vertical", "amount": -3 }
{ "action": "type_text", "text": "hello world" }
{ "action": "key_press", "key": "enter" }
{ "action": "key_combo", "keys": ["ctrl", "shift", "t"] }
{ "action": "wait", "milliseconds": 500 }
```

| Action | Required input | Notes |
| --- | --- | --- |
| `status` | none | Reports session type, input connection, display size, pointer location when the backend exposes it, and configured screenshot backends. |
| `screens` | none | Reports primary display size and, once a portal session exists, the portal stream position/size. |
| `cursor` | none | Returns the current pointer position when the backend exposes it. |
| `find_text` | `text` | OCRs the screen (or a supplied `path`) and returns center coordinates for matching words. |
| `click_text` | `text` | Finds matching OCR text and clicks its center. Optional `button`, `path`, `region`. |
| `batch` | `steps` | Runs up to 50 computer actions in order and stops at the first failure. Use this to reduce model round-trips. |
| `clipboard_set` | `text` | Replaces the clipboard using `wl-copy`, `xclip`/`xsel`, `pbcopy`, or `clip`. |
| `clipboard_get` | none | Returns the current clipboard text. |
| `clipboard_clear` | none | Clears the clipboard. |
| `open_url` | `url` | Opens an `http`, `https`, or `file` URL with `xdg-open`, `open`, or `start`. |
| `focus_window` | `title` | Best-effort window focus through `hyprctl`, `wmctrl`, or `xdotool`. GNOME Wayland has no generic API and returns a clear failure. |
| `screenshot` | none | Optional `region` and `include_cursor`. With `MODEL_VISION=1`, the image is attached to the next model turn automatically; `inline_image` only matters for text-only models. |
| `ocr` | none | Optional `path`; when omitted, captures a screenshot first. Requires `tesseract`. |
| `mouse_move` | `x`, `y` | Absolute by default; set `relative: true` for offsets. Negative absolute coordinates are rejected. |
| `mouse_click` | none | Optional `button`, `x`, `y`, and `count`. |
| `mouse_drag` | `from`, `to` | Optional `button`. Both points are absolute. |
| `scroll` | `amount` | Optional `axis`, `x`, and `y`. Positive scrolls down/right. |
| `type_text` | `text` | Unicode text through the input backend. |
| `key_press` | `key` | A single character or a named key such as `enter`, `escape`, `ctrl`, `f1`, or `numpad7`. |
| `key_combo` | `keys` | Ordered key names, for example `["ctrl", "l"]`. Modifiers are pressed first and released in reverse order. |
| `wait` | none | Optional `milliseconds`, default 1000, maximum 30000. |

Coordinates are global screen pixels with the origin at the top-left.
Temporary screenshot files are cleaned automatically: files older than
`HYUSK_SCREENSHOT_RETENTION_SECS` (default 900) are deleted, and at most
`HYUSK_SCREENSHOT_MAX_FILES` (default 30) are retained.

When `MODEL_VISION=1`, the `screenshot` action emits an internal marker that
the agent runner converts into a multimodal user message with a
`data:image/png;base64,...` URL, regardless of `inline_image`. Vision-capable
models should inspect that image directly and only use `ocr` when the image is
unreadable. Set `MODEL_VISION=0` for text-only models; the tool then returns
only the path and `ocr` is the extraction path.

Screenshot capture tries available backends in order:

- Wayland: `grim`, GNOME Shell D-Bus, then the Screenshot portal.
- X11: ImageMagick `import`, then `scrot`.
- macOS: `screencapture`.

On GNOME Wayland, `grim` and the Shell screenshot interface normally fail; the
tool then calls the Screenshot portal, which may show an approval dialog. If the
dialog is denied, the tool reports every backend error instead of reporting
false success.

Pointer and keyboard injection is compositor-dependent. On Wayland the XDG
RemoteDesktop portal is tried first. If the portal is unavailable or the user
denies access, the tool falls back to `enigo`, but detects and rejects the
XWayland fallback because XTEST events are not delivered to native Wayland
applications. Native X11 and Wayland sessions with virtual-input support are
accepted.

Keyboard input uses Linux evdev keycodes through `NotifyKeyboardKeycode`, with
keysyms as a fallback for characters that have no evdev mapping. Keyboard
events follow the focused window, so the orb does not request focus by default
(`ORB_STEAL_FOCUS=0`); otherwise portal typing would be delivered to the orb
instead of the controlled application.

Absolute pointer movement uses the PipeWire stream from a combined ScreenCast
session, so `mouse_move` is not limited to relative deltas. The first action
opens the approval dialog; later actions reuse the session.

### Precision targeting

Vision models are unreliable at guessing exact pixel coordinates. Prefer the
OCR-backed targeting actions when the element has a visible label:

```json
{ "action": "find_text", "text": "Search" }
{ "action": "click_text", "text": "Search" }
{ "action": "cursor" }
```

`find_text` returns word centers derived from Tesseract bounding boxes, and
`click_text` clicks the first matching center directly. Use `screenshot` when
you need to see the layout or verify the result.

### Faster workflows

`batch` is the main speed feature. A single model tool call can open an app,
wait, and take a screenshot:

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

A WhatsApp Web message flow is typically:

1. `open_url` to `https://web.whatsapp.com`.
2. `wait` for the page and QR/login state.
3. `screenshot` + `ocr` to locate the search box, or use fixed coordinates once
   the layout is known.
4. `mouse_click` the search field, `type_text` the contact name, `key_press`
   `enter`.
5. `mouse_click` the message box, `type_text` the message, `key_press`
   `enter`.

Wrap steps 3-5 in `batch` when the coordinates are known. For VS Code, open the
folder with the `process` tool (`code /path/to/folder`), then use `focus_window`
and `key_combo` for editor shortcuts.

## Accessibility

`AccessibilityTool` drives GUI applications through Linux AT-SPI2. Rust spawns
`scripts/atspi_bridge.py` once and exchanges line-delimited JSON with it. This
is much faster than screenshots for apps that expose an accessible tree.

```json
{ "action": "active", "read": true }
{ "action": "apps" }
{ "action": "windows" }
{ "action": "find", "app": 3, "name": "Search", "role": "push button", "limit": 10 }
{ "action": "find", "text": "Sign in", "limit": 5 }
{ "action": "click", "path": [3, 0, 1, 2] }
{ "action": "focus", "name": "Search", "app": 3 }
{ "action": "set_text", "name": "Message", "role": "entry", "text": "hello" }
{ "action": "get_text", "path": [3, 0, 1, 4] }
{ "action": "read", "app": 3 }
{ "action": "tree", "app": 3, "max_depth": 4, "max_nodes": 2000 }
```

| Action | Input | Notes |
| --- | --- | --- |
| `active` | optional `read` | The focused app, window, and element; with `read` also the active window's visible text. Prefer this first to orient. |
| `apps` | none | Applications with indices, window titles, and an `active` flag. |
| `windows` | none | Every top-level window with title, bounds, `active` flag, and `path`. |
| `tree` | `app`, optional `max_depth`/`max_nodes` | Flat list of accessible nodes with `path`, role, name, bounds, states, actions. |
| `find` | `name`/`role`/`text`, optional `app`, `limit` | Matching nodes with exact `path` values. |
| `click` | `path` or `name`/`role`/`text` | Performs the preferred action (`click`, `press`, `activate`, ...), or `action_name`. |
| `focus` | `path` or `name`/`role`/`text` | Grabs keyboard focus. |
| `set_text` | `path` or `name`/`role`/`text`, `text` | Sets editable text. |
| `get_text` | `path` or `name`/`role`/`text` | Reads text from a text interface. |
| `read` | `app` or `path` | Dumps visible text as labelled lines (native screen reader). |

Prerequisites: `python3` with `pyatspi` and a running AT-SPI bus. Apps only
appear if they publish a tree, so run `scripts/setup-accessibility.sh` once:

```bash
./scripts/setup-accessibility.sh          # enable toolkit-accessibility + env
./scripts/setup-accessibility.sh --check  # show what AT-SPI currently sees
```

That enables `toolkit-accessibility`, writes `ACCESSIBILITY_ENABLED=1` for
Chromium/Electron apps, and prints the Flatpak override for sandboxed browsers.
Restart apps (and re-login once) afterwards.

Responses include a `warnings` field when the environment is limiting what can
be seen (for example toolkit-accessibility is off), and `active` returns an
explicit error when the focused app publishes no tree -- in that case the agent
falls back to the `computer` tool (screenshot + vision or OCR).

## Memory

`MemoryTool` provides persistent local memory across runs. Data is stored in:

```text
$XDG_DATA_HOME/hyusk/memory.json   (or ~/.local/share/hyusk/memory.json)
$XDG_DATA_HOME/hyusk/memory.md     (human-readable)
```

```json
{ "action": "remember", "text": "User prefers dark mode", "tags": ["preference"] }
{ "action": "search", "query": "dark mode" }
{ "action": "graph_add", "subject": "user", "relation": "uses", "object": "GNOME Wayland" }
{ "action": "graph_query", "subject": "user" }
{ "action": "list", "limit": 10 }
{ "action": "forget", "id": 3 }
```

| Action | Input | Notes |
| --- | --- | --- |
| `remember` | `text`, optional `tags` | Adds a note and rewrites `memory.json` and `memory.md`. Duplicate notes are rejected. |
| `search` | `query`, optional `limit` | Ranks notes with a small tf-idf score; tag matches weigh more. |
| `graph_add` | `subject`, `relation`, `object` | Adds a knowledge-graph triple. Duplicate triples are rejected. |
| `graph_query` | optional `subject`, `relation`, `object` | Filters triples by substring. |
| `list` | optional `limit` | Most recent notes. |
| `forget` | `id` | Removes a note. |

Each turn retrieves the notes that best match the user's message and injects
them into the system prompt, so the model sees what is relevant to the current
request rather than only the newest notes. The prompt also tells the model to
save durable facts proactively (preferences, routines, projects, decisions,
corrections) and to search before assuming something about the user or system.

## Shell

`ShellTool` runs `sh -c <command>`.

```json
{ "command": "ls -la", "mode": "foreground" }
```

Modes:

- `foreground` (default): wait for the command and return exit status, stdout,
  or stderr.
- `detached`: spawn and return the child PID without waiting.

This tool is appropriate for shell features such as pipelines and redirection.
It executes arbitrary commands as the current user.

## Process

`ProcessTool` launches an executable directly, without shell parsing.

```json
{ "action": "launch", "program": "/usr/bin/firefox", "args": ["-new-window"] }
```

The only current action is `launch`. After spawning, the tool sleeps for 300 ms
and calls `try_wait()`. This gives a useful error when a binary cannot start or
exits immediately, while allowing long-running applications to continue.

Use `process` for a known executable and `shell` when shell syntax is required.

## Media

`MediaTool` invokes `playerctl`, which talks to MPRIS players such as Spotify,
VLC, mpv, and browser-based players.

```json
{ "action": "play" }
{ "action": "volume", "level": 50 }
{ "action": "status" }
```

Actions are:

| Action | Input |
| --- | --- |
| `play` | none |
| `pause` | none |
| `play_pause` | none |
| `next` | none |
| `previous` | none |
| `stop` | none |
| `status` | none |
| `volume_up` | none |
| `volume_down` | none |
| `volume` | integer `level` from 0 through 100 |

`status` formats artist, title, album, and playback status. A missing player is
returned as a normal tool failure so the model can respond instead of crashing
the process.

## Adding a tool

1. Add a module under `src/tools/`.
2. Implement `Tool`, including a precise `input_schema()`.
3. Export the module from `src/tools/mod.rs`.
4. Register an instance in `main.rs`.

A minimal implementation looks like this:

```rust
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Tool, ToolResult};

pub struct Ping;

#[derive(Debug, Deserialize)]
struct PingInput {
    message: String,
}

#[async_trait]
impl Tool for Ping {
    fn name(&self) -> &str { "ping" }

    fn description(&self) -> &str {
        "Echo a short message without touching the host."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: PingInput = serde_json::from_str(input)?;
        Ok(ToolResult::success(format!("pong: {}", input.message)))
    }
}
```

Then add `pub mod ping;` to `tools/mod.rs` and call
`tools.register(Ping);` during startup. Restart the binary so the new spec is
sent to the model.

## Security model

The current tool platform has no approval or confinement layer. In particular:

- `shell` can read, modify, or delete files available to the user.
- `process` can launch any executable visible on `PATH` or by absolute path.
- `media` can control desktop players exposed through MPRIS.
- `computer` can move the real pointer, click, scroll, type, and read clipboard
  or screen contents through OCR. It can trigger irreversible GUI actions.
- Tool output is sent back to the remote model provider.

Treat the configured model and provider as trusted code. Run this crate in a
restricted OS account or container when experimenting with untrusted prompts,
and do not expose the API key or tool output to logs or reports.
