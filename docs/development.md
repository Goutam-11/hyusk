# Development

## Repository position

This repository is a standalone Cargo workspace. It used to be a member of the
parent Hyusk workspace alongside the older `hyusk` CLI workspace; it now
declares its own `[workspace]` and owns its `Cargo.lock` and `target/`, and the
parent workspace excludes this directory.

Package metadata (version, edition, authors, license, repository), dependency
versions, and build profiles are declared in this crate's own
[`Cargo.toml`](../Cargo.toml), so a fresh clone builds and tests on its own,
which is what CI does.

Run commands from this directory:

```bash
cargo check --all-targets
cargo test --all-targets
cargo run
```

## Continuous integration

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on pushes to
`main`, on pull requests, and on demand:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --all-targets
```

The job installs the native packages listed under Prerequisites below, caches
the Cargo registry and build directory, and reports clippy warnings without
failing on them. The portal, live-CDP, Whisper-inference, and GUI workflow
checks stay `#[ignore]`d, so CI does not cover the desktop-only paths. A cold
run builds the whole dependency tree, including GTK4, the AWS SDK, whisper.cpp,
and ONNX Runtime, and takes far longer than a cached run.

## Prerequisites

- A current stable Rust toolchain with Cargo.
- A display server and the native libraries required by `eframe`/`winit` on
  the host platform.
- A microphone for audio startup checks and wake detection.
- A wake-word classifier. Run `scripts/download-wake-word.sh` to download the
  temporary LiveKit `hey_livekit.onnx` model, or supply a custom model with
  `WAKE_WORD_MODEL`.
- `playerctl` for the media tool.
- A working desktop input backend for the `computer` tool. On Wayland the
  XDG RemoteDesktop portal is used and asks for one-time approval. On X11, or
  on Wayland compositors with virtual-input protocols, the native `enigo`
  backend is used.
- Optional screenshot/OCR helpers: `grim`, ImageMagick `import`, `scrot`,
  `gdbus`, and `tesseract`. The Screenshot portal is used automatically on
  Wayland when the other backends fail. The `computer` tool tries the available
  backends and reports the failures when none work.
- For Linux TTS, `piper` plus an audio player such as `paplay` or `aplay`; the
  code falls back to `espeak-ng` or `espeak` when Piper is unavailable.

On Debian or Ubuntu the build needs `build-essential`, `cmake`, `clang`,
`libclang-dev`, and `pkg-config` for whisper.cpp and the AWS-LC bindings,
`libasound2-dev` for audio, `libgtk-4-dev` and `libadwaita-1-dev` for the
settings editor, and the X11/Wayland client packages `libx11-dev`, `libxi-dev`,
`libxcursor-dev`, `libxrandr-dev`, `libxinerama-dev`, `libwayland-dev`,
`libxkbcommon-dev`, and `libgl1-mesa-dev`. `ci.yml` installs exactly this set.
Distribution package names vary.

## Configuration

Create a local environment file:

```bash
cp .env.example .env
chmod 600 .env
```

Set at least:

```text
OPENROUTER_API_KEY=your-api-key
OPENROUTER_MODEL=openai/gpt-4o-mini
OPENROUTER_BASE_URL=https://openrouter.ai/api/v1
MODEL_VISION=1
WAKE_WORD_ENABLED=1
WAKE_WORD_MODEL=models/hey_hyusk.onnx
WAKE_WORD_MODELS=models/hey_hyusk.onnx,models/hey_livekit.onnx
WAKE_WORD_THRESHOLD=0.93
WAKE_WORD_STRONG_THRESHOLD=0.94
WAKE_WORD_MIN_HITS=1
WAKE_WORD_HIT_WINDOW_MS=1000
WAKE_WORD_MIN_RMS=0.003
WAKE_WORD_COOLDOWN_MS=1500
WAKE_WORD_DENOISE=0
STT_MODEL=models/ggml-base.en.bin
```

`.env` is ignored by Git. Do not commit credentials. Shell exports override
values loaded from the file. Keep the mode at `0600`: it usually holds a live
provider key, `scripts/install-autostart.sh` tightens a wider mode, and
`gnome_doctor` reports `needs_attention` while other accounts can read it.

Model files are runtime assets and `models/` is ignored by Git. Keep trained
wake models, Whisper files, Piper voices, and downloaded negative datasets out
of normal commits.

## Validation

Run these before committing:

```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets
cargo test --bin hyusk_agent
```

`cargo test --all-targets` runs the full suite: 342 passing tests and 7
ignored (the main binary alone: 313 passing, 6 ignored; shared model/stream
tests are compiled into each binary). The ignored ones need a real session or
network: the four portal smoke tests below, a live CDP browser test, a Whisper
cancel/cache test, and the GTK workflow editor draft test. Coverage includes
the computer tool and portal mapping, input and URL validation, browser CDP
protocol handling, accessibility targeting, wake and clap detection, RNNoise
denoising, the instant-command router, streaming sentence splitting, session
repair and compaction, persistent memory, scheduling, and the one-shot
`WakeResume` handshake.

`cargo clippy --all-targets` currently reports three
`result_large_err` warnings on `LinkServerError` in `src/link/server.rs`; there
are no errors and no other warnings. Treat a new warning as a regression.

The ignored tests open real RemoteDesktop/Screenshot approval dialogs and move
the pointer by one pixel. Run them manually when validating portal input or
capture:

```bash
cargo test portal_relative_move_smoke -- --ignored --nocapture
cargo test portal_absolute_move_smoke -- --ignored --nocapture
cargo test portal_keyboard_smoke -- --ignored --nocapture
cargo test portal_screenshot_smoke -- --ignored --nocapture
```

`cargo fmt --all -- --check` is clean; keep it that way rather than mixing
formatting churn into unrelated edits.

## Running

The process working directory matters because startup looks for
`models/hey_hyusk.onnx` using a relative path. Run from this directory:

```bash
cd hyusk_agent
cargo run
```

The binary opens a native window and remains running until the window closes.
The terminal prints audio, wake, tool, and agent diagnostics. Type a message in
the terminal and press Enter to send it to the agent; the UI currently shows
only an animated status visual.

## Fedora automatic startup and logs

Run `scripts/install-autostart.sh` from this crate to build and install the
`hyusk.service` user unit. It is bound to `graphical-session.target`, so it
starts after GNOME login and stops when the graphical session ends or the
laptop powers down. The unit captures stdout/stderr in journald with the
identifier `hyusk` and enables Rust backtraces for crash diagnosis.

```bash
systemctl --user status hyusk.service
journalctl --user -u hyusk.service -f
journalctl --user -u hyusk.service -b -p warning
```

Use `scripts/uninstall-autostart.sh` to disable and remove the unit.

## Known runtime gaps

- `models/hey_hyusk.onnx` is not bundled. The temporary
  `models/hey_livekit.onnx` classifier is downloaded by
  `scripts/download-wake-word.sh` and selected automatically when the custom
  model is absent.
- STT uses variable-length recording: it stops after `STT_SILENCE_MS` of
  silence, gives up after `STT_NO_SPEECH_MS` without speech, and needs a
  compatible Whisper model at `STT_MODEL`. First-time model loading cannot be
  interrupted.
- GNOME Wayland uses the RemoteDesktop and Screenshot portals. The first input
  or screenshot action requires the user to approve a portal dialog.
- Absolute pointer moves on the portal path are translated to relative motion
  because a RemoteDesktop-only session has no PipeWire stream; the tool tracks
  the pointer position from an XWayland hint when available.
- `OrbApp` (eframe) is still a status visual: the GNOME indicator card and the
  Noctalia tooltip render reply text and approvals, the orb does not.
- Echo cancellation depends on the PipeWire/PulseAudio module installed by
  `scripts/install-autostart.sh --voice-audio`; without it the agent can hear
  its own playback.

These gaps are intentional documentation targets for the next voice/UI
integration work; they should not be described as completed behavior.

## Troubleshooting

### Startup says `Wake-word detector disabled`

The detector requires a LiveKit/openWakeWord-compatible classifier. Run
`scripts/download-wake-word.sh` to fetch the temporary `hey_livekit.onnx` model,
or set `WAKE_WORD_MODEL` explicitly. Set `WAKE_WORD_ENABLED=0` to skip wake
detection intentionally; a missing model no longer stops the binary.

### Startup reports no microphone

Check the operating system input-device selection and CPAL support. The initial
audio probe is non-fatal, but wake detection will stop if no default input
device exists.

### Media commands fail

Install `playerctl` and make sure a player exposes MPRIS on the active desktop
session. A command can also fail when no player is currently registered.

### Screenshot or OCR actions fail

The `computer` tool tries `grim`, GNOME Shell D-Bus, and the Screenshot portal
on Wayland, and ImageMagick `import`/`scrot` on X11. On GNOME Wayland approve
the portal dialog when it appears. The tool reports the individual backend
errors. Install `tesseract` for the `ocr` action.

### TTS is silent

On Linux, verify that `piper` is on `PATH`, `PIPER_MODEL` points to a model,
and either PulseAudio (`paplay`) or ALSA (`aplay`) can play the generated WAV.
The fallback requires `espeak-ng` or `espeak`.

### Model requests fail

Check the API key, model identifier, and base URL. HTTP 429 and 5xx responses
are retried automatically; HTTP 400 and 401 responses are not.
