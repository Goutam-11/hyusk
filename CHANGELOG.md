# Changelog

## Unreleased

### Added

- `HYUSK_WAKE_TEST=1` diagnostics mode: runs only the wake detector with live
  level/gate/score output and prints every detection event, so microphone and
  classifier problems are visible without starting the agent.
- `HYUSK_WAKE_CAPTURE=<path>` records the exact 16 kHz mono stream the
  classifier consumes, and the `wake_score_probe` test scores it offline
  (sliding windows) for tuning thresholds against a real voice.
- Accessibility `read` action: dumps the visible text content of an app
  natively (the fast replacement for screenshot + OCR), plus a `windows`
  action listing open windows with active state, richer element descriptions
  with embedded text, an optional `action_name` for `click`, and a
  native-first desktop-control recipe in the system prompt.

### Fixed

- Wake-word classifier never fired from an exactly 2-second buffer: the
  classifier consumes the last 16 embedding windows, which a 32000-sample
  buffer cannot produce. The inference buffer is now 2.2 seconds.
- Hand-clap detector rejected almost every real clap: the onset check
  required the preceding 10 ms window to be under 20% of the clap peak,
  which real claps (with spread-out attacks) fail. The ratio is now 45%
  with a matching decay check, and the background level also adapts to
  sustained loud noise (music, TV) so clap sensitivity survives playback.
- Accessibility bridge reported success even when `doAction`, `grabFocus`,
  or `setTextContents` threw; failures are now returned as errors, and a
  click on an element without an action interface returns its screen
  bounds so the computer tool can click there instead.
- Portal clicks frequently landed on the old pointer position or were
  dropped; move-to-press and press-to-release now settle briefly, and
  drags move through intermediate points so compositors register motion.
- Shell foreground commands had no timeout and could wedge a turn for
  minutes; they are now bounded at 120 s with truncated output.
- Media volume levels 1-9 set 10x the requested volume (`0.1` instead of
  `0.01`).
- The wake detector's microphone thread could hang forever if the runtime
  died while it was paused; every resume wait is now bounded (120 s).
- Chunk-wise microphone resampling restarted its phase every callback,
  adding timing jitter on non-48 kHz microphones; the resampler is now
  phase-continuous across chunks (unit-tested against whole-buffer
  resampling).
- RNNoise state was recreated (plus an 8-frame silence warmup) on every
  wake inference; the denoiser state now persists for the session.
- Classifier input was peak-normalized with up to 8x gain, amplifying
  denoiser residue; the cap is now 2x, and the energy gate is softer so
  quiet or far speech still reaches the model.
- System prompt (wall-clock time, memory summary) was frozen at process
  start; it is rebuilt every turn.
- Wake-word inference almost never ran in time: dependency code (the ONNX
  feature extraction in `livekit-wakeword`) was compiled unoptimized in debug
  builds, taking seconds per prediction, so the 2.2 s audio window slid past
  the wake word before it was ever scored. Dependencies are now optimized even
  in dev builds, dropping inference to tens of milliseconds.
- Hand-clap detection silently stopped after startup: the scanner used a
  buffer-relative cursor that stopped advancing once the 2.5 s sliding audio
  buffer reached its cap. It now tracks absolute stream positions, so claps
  are detected for the whole session.
- The detector stopped scoring as soon as the audio went quiet, skipping the
  single best-aligned window (the wake word ending at the buffer end). A grace
  period now keeps scoring through the trailing silence.
- Microphone captures dominated by sub-100 Hz energy (DC offset and rumble
  from high hardware gain) masked speech; a 90 Hz high-pass now runs before
  denoising and clap detection.
- A loud pop when the capture stream opened could poison the clap scanner's
  background estimate and spuriously fire; the first second of audio is now
  ignored.
- PipeWire/ALSA UCM often restores the microphone capture path to maximum
  (+30 dB capture, +30 dB mic boost), clipping the ADC beyond recovery; the
  detector now warns when the input is clipping.
- Speech-to-text discarded valid commands as "background noise": its silence
  gate used fixed thresholds tuned for the old maximum-gain default, so a
  correctly calibrated (quieter) microphone failed it while loud fan noise
  could pass. The gate is now adaptive -- it compares the loud part of the
  clip against its own background -- and the captured level is printed.
- Speech-to-text fed low-frequency rumble straight to Whisper, which
  hallucinated sentences from noise; the recording is now high-passed (the
  same 90 Hz filter the wake detector uses).
- The wake detector kept its microphone stream open while speech-to-text
  recorded the command, opening a second capture stream. It now pauses the
  stream for the recording and resumes afterwards.
- The wake detector heard the agent's own text-to-speech and re-triggered in a
  loop (a clap on the speech onset). Hand-clap wake is now off by default, and
  while the agent is working the clap scanner is disabled and the wake-word
  threshold is raised slightly. The wake word stays live, so saying "alexa"
  during a turn interrupts it and starts listening again (barge-in), but the
  agent's own voice does not trigger it.

### Changed

- Voice turns are much faster. Common commands ("open Firefox", "go to
  YouTube", "switch to Code", "next song", "volume up") are recognized by a
  local router and executed without a model call. Longer replies stream from
  the model and are spoken sentence-by-sentence while generation continues,
  instead of waiting for the whole answer. Recording now stops 700 ms after
  speech ends (was about a second), recordings are capped at 4 s, and Whisper
  uses all CPU cores instead of a hard-coded four.
- The default model is now a fast paid one (`openai/gpt-4o-mini`); free tiers
  are frequently queued and add seconds to every turn.
- Hand-clap wake is disabled by default: background noise (doors, coughs,
  desk bumps) triggered it often enough to be a nuisance. Set
  `WAKE_CLAP_ENABLED=1` to opt back in; `alexa` remains the wake word.
- The system prompt now lists the registered tools and teaches a concrete
  strategy: work one tool at a time, prefer native accessibility, fall back to
  the `computer` tool, recover from errors with a different approach, and
  confirm destructive actions before running them.
- Memory retrieval is query-aware. Each turn ranks stored notes against the
  user's message with a small tf-idf score (tag matches weighted higher) and
  injects those, instead of only the most recently created notes.
- The accessibility bridge can now tell where the user is: a new `active`
  action reports the focused application, window, and element (and the visible
  text with `read`). `apps` includes window titles and an active flag,
  `windows` includes title, bounds, and active state, and `find`/`click`/
  `focus` can match an element by its visible `text` as well as name/role.
  Responses carry a `warnings` field when the environment limits visibility.
- The system prompt now tells the agent to start with `accessibility active`,
  and that apps publishing no tree (Chromium/Electron without
  `ACCESSIBILITY_ENABLED=1`, or any app while toolkit-accessibility is off) are
  a blind spot to fall back from to screenshots.
- Voice wake flow no longer blocks the agent event loop: recording and
  transcription run as their own cancellable task, a new wake or typed
  message interrupts an in-flight listening session, and the wake detector
  is always resumed (drop guard) even when that task is cancelled.
- Whisper inference runs on the blocking pool instead of a runtime worker.
- After a wake, recording stops shortly after speech ends instead of
  always waiting the full fixed duration, cutting wake-to-command latency.
- `WAKE_DEBUG=1` prints live level/gate/score diagnostics for tuning.
- Lowered default clap sensitivity/min-peak and wake threshold in `.env`.

### Added

- Event-driven agent runtime using Tokio MPSC channels.
- Local instant command router (`src/agent/commands.rs`) with built-in app and
  site aliases, search-on-engine support, media/volume control, window
  switching, lock/screenshot, and "take a note". Extend it with
  `~/.config/hyusk/commands.json`.
- `window` tool backed by a new `org.hyusk.Shell` D-Bus interface in the GNOME
  extension: `list`, `active`, and `activate <query>` for native window
  switching on Wayland (install with `scripts/install-gnome-indicator.sh`).
- `HYUSK_TIMING=1` prints per-stage latency (record/transcribe, model, TTS).
- `scripts/setup-accessibility.sh` enables GNOME toolkit accessibility and sets
  `ACCESSIBILITY_ENABLED=1` for Chromium/Electron apps (including Flatpak
  browsers) so more applications publish an accessibility tree; `--check` shows
  what AT-SPI currently sees.
- Memory deduplication: identical notes and graph triples are not stored twice.
- The prompt now tells the assistant to save durable facts about the user
  proactively (preferences, routines, projects, decisions, corrections).
- `HYUSK_WAKE_TEST=<seconds>` standalone microphone/classifier diagnostic
  (`HYUSK_WAKE_TEST=30 cargo run`) that prints live level/gate/score
  diagnostics and every detection.
- `HYUSK_WAKE_CAPTURE=<path>` records the exact 16 kHz stream the classifier
  consumes for offline analysis.
- `HYUSK_STT_TEST=1` records from the default microphone without a wake word
  and prints the captured level and transcription.
- `scripts/set-mic-gain.sh` sets a sane capture level and mic-boost value for
  the default microphone so the ADC does not clip.
- Replies are passed through a speech sanitizer before text-to-speech: Markdown
  markers, code fences, emoji, tables, and links are removed and symbols like
  `%` and `+` are spoken as words, so the voice no longer reads formatting
  aloud.
- The system prompt now defines Hyusk as a friendly, concise, spoken persona
  that answers in plain prose without Markdown.
- Animated transparent `eframe` status overlay.
- `computer` tool for desktop GUI control: status, screenshots, OCR, mouse
  movement/click/drag, scrolling, typing, key presses, key combos, and waits.
- Linux XDG RemoteDesktop portal backend for GNOME/Wayland input and Screenshot
  portal fallback for capture, both with one-time user approval.
- Interruptible async agent turns: a new typed message or wake word cancels the
  in-flight model request, tool call, and TTS, and rolls the conversation back
  to the previous complete state.
- Cancellable Linux TTS using `tokio::process` children with `kill_on_drop`.
- Circular black always-on-top orb with opt-in keyboard focus and an optional
  GNOME top-bar indicator mode.
- Absolute pointer movement through the portal's combined ScreenCast stream.
- Keyboard input through portal evdev keycodes with keysym fallback; the orb no
  longer steals keyboard focus by default (`ORB_STEAL_FOCUS`).
- GNOME Shell top-bar indicator (`gnome-extension/hyusk@hyusk.local`) with a
  color-coded butterfly, plus `scripts/install-gnome-indicator.sh` and the
  `$XDG_RUNTIME_DIR/hyusk-state` runtime state file.
- `batch` action for multi-step desktop workflows in one tool call.
- OCR bounding-box targeting with `find_text`/`click_text` and a `cursor`
  position action for more precise GUI control.
- Automatic temporary-screenshot cleanup via
  `HYUSK_SCREENSHOT_RETENTION_SECS`/`HYUSK_SCREENSHOT_MAX_FILES`.
- Persistent local memory tool with Markdown notes, JSON storage,
  knowledge-graph triples, and sparse bag-of-words search; recent memories are
  injected into the system prompt at startup.
- System context (OS, distribution, kernel, host, desktop/session, date/time)
  in the model system prompt.
- Linux AT-SPI2 accessibility tool (`accessibility`) backed by a Python
  `pyatspi` bridge for fast app control without screenshots.
- Multi-phrase wake support: the temporary `hey_livekit.onnx` and `alexa.onnx`
  classifiers are auto-discovered and scored together.
- Fixed `install-gnome-indicator.sh` so the temporary zip bundle is created
  correctly.
- Clipboard set/get/clear, `open_url`, and best-effort `focus_window` actions.
- Multi-model wake detection (`WAKE_WORD_MODELS`), adaptive noise-floor
  tracking, peak normalization, `WAKE_WORD_MIN_RMS`, and
  `WAKE_WORD_COOLDOWN_MS`.
- RNNoise denoising (`WAKE_WORD_DENOISE`) for fan/hiss noise before wake
  scoring.
- Multimodal screenshot support (`MODEL_VISION`): screenshots are attached to
  the next model turn as image content for vision-capable models instead of
  requiring OCR.
- `scripts/download-wake-word.sh` and automatic `models/hey_livekit.onnx`
  fallback with the `WAKE_WORD_THRESHOLD` setting.
- Wake-word detector based on CPAL and `livekit-wakeword`, with an optional
  `WakeResume` pause/resume handshake for post-wake STT.
- Whisper-based speech-to-text wired into the wake flow, plus platform-specific
  text-to-speech adapters whose completion is now awaited.
- `media` tool for MPRIS playback control through `playerctl`.
- Structured OpenAI-compatible function calling with per-tool JSON schemas.
- Retry handling for transient model requests, including `Retry-After`.
- Environment switches for `WAKE_WORD_ENABLED`, `WAKE_WORD_MODEL`, and
  `STT_MODEL`.
- Terminal stdin input path so typed messages always reach the agent, even when
  wake detection or STT is unavailable.

### Changed

- Replaced the original REPL composition with a desktop/voice-oriented startup
  sequence.
- Tool results now use `role: "tool"` messages with matching `tool_call_id`
  values.
- Tool discovery now uses `ToolRegistry::iter()` and each tool's schema instead
  of a prose description list.
- The agent keeps an in-memory conversation history across tool-call rounds.
- Wake detection is optional: a missing `WAKE_WORD_MODEL` disables detection
  instead of stopping startup.
- `WakeWordDetected` is now followed by STT transcription and a `UserInput`
  event, and the runtime resumes the detector after transcription.
- The runtime awaits TTS completion before returning the UI to `Hidden`.

### Removed

- Unused `hound`, `rodio`, and `async-std` dependencies.
- Unused `WakeWordDetector::new` single-model helper and
  `TextToSpeech::speak_async`.
- Duplicate non-cancellable Linux Piper/espeak TTS path; Linux now uses only
  the cancellable implementation.

### Current limitations

- `models/hey_hyusk.onnx` is not bundled. The temporary
  `models/hey_livekit.onnx` classifier is downloaded by
  `scripts/download-wake-word.sh` and selected automatically when the custom
  model is absent.
- STT uses a fixed six-second recording window rather than voice-activity
  detection.
- Hand-clap detection is stricter (sharp onset plus fast decay) and
  restartable, and the noise-floor threshold bug that made claps impossible
  was fixed.
- Wake detection and TTS are sequential. There is no echo cancellation, so
  while the agent is speaking the detector relies on a raised threshold and a
  disabled clap scanner to avoid hearing its own voice; a close, deliberate
  wake word still barges in and stops the agent.
- The UI does not render response text; it only displays activity states.
- GNOME Wayland may ignore the orb's top-position and focus requests.
- The GNOME indicator extension is only scanned at session start, so the first
  install requires a logout/login.
- `focus_window` has no generic GNOME Wayland backend and can fail there.
- There is no persistence, streaming, permission prompt, process monitoring, or
  tool-level cancellation inside arbitrary tools.

## v0.1 - initial agent

- REPL that loaded `.env`, registered `ShellTool` and `ProcessTool`, and parsed
  JSON tool requests from model text.
- OpenRouter-compatible chat client and in-memory agent loop.
