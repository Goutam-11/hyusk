# Voice and UI pipeline

The wake-word, speech-to-text, text-to-speech, and overlay components are
connected when their local models and external programs are available. This
document describes the implemented path and the remaining limitations.

## Current flow

```text
Microphone
   |
   v
WakeWordDetector
   |
   |  emits WakeWordDetected
   v
run_agent_task
   |
   |  StateChanged(Waking)
   |  StateChanged(Listening)
   |  SpeechToText::transcribe_from_microphone(8.0)
   |  WakeResume::resume
   v
UserInput event
   |
   v
Agent::handle -> OpenRouter -> tool loop -> final text
   |
   +-> Response event
   +-> TextToSpeech::speak_cancellable (awaited)
   +-> StateChanged(Hidden)
   +-> UI event channel

The overlay receives state/tool/response events and draws an animated
butterfly. It does not render the response text.

Typed terminal lines bypass wake detection and STT and are delivered directly
as `UserInput`, so the text and `computer` tool paths remain available even
when the voice models are missing.
```

## Wake-word detector

`WakeWordDetector` uses CPAL to capture the same selected microphone as STT
(`HYUSK_INPUT_DEVICE`, or the default), converts input
to mono, resamples it to 16 kHz, and runs `livekit-wakeword` inference in a
blocking worker. The detector keeps roughly 2.5 seconds of audio, runs
predictions every two hundred milliseconds and applies a confidence threshold
of `0.93`. A score of `0.94` or higher satisfies the strong single-hit path.

The constructor requires a LiveKit-compatible ONNX classifier. The model should
produce a prediction named `hey_hyusk`; the code falls back to the highest
prediction if that key is absent. The bundled Whisper `.bin` files and Piper
`.onnx` files are not wake-word classifiers.

Wake detection is optional. It is controlled by:

| Variable | Default | Behavior |
| --- | --- | --- |
| `WAKE_WORD_ENABLED` | `1` | `0`, `false`, `off`, or `no` skips wake detection. |
| `WAKE_WORD_MODEL` | `models/hey_hyusk.onnx`, then fallbacks | Path to a LiveKit-compatible classifier. |
| `WAKE_WORD_MODELS` | - | Comma-separated classifiers; all are scored and the highest confidence wins. |
| `WAKE_WORD_THRESHOLD` | `0.93` | Detection confidence threshold. Lower is more sensitive. |
| `WAKE_WORD_STRONG_THRESHOLD` | `0.94` | A single score at or above this level wakes immediately. |
| `WAKE_WORD_MIN_HITS` | `1` | Qualifying inference windows needed to wake. |
| `WAKE_WORD_HIT_WINDOW_MS` | `1000` | Window used when more than one hit is configured. |
| `WAKE_WORD_MIN_RMS` | `0.003` | Adaptive noise-gate floor. Lower accepts quieter speech. |
| `WAKE_WORD_COOLDOWN_MS` | `1500` | Minimum time between detections. |
| `WAKE_WORD_DENOISE` | `0` | Optional RNNoise preprocessing; training and runtime preprocessing must match. |
| `WAKE_WORD_STEP_MS` | `200` | Interval between inference passes. |

The detector tracks a slowly-moving noise floor, peak-normalizes each inference
window, and scores the configured model candidates. A configured
`WAKE_WORD_MODELS` list overrides automatic discovery. RNNoise can suppress
stationary fan and hiss noise, but it should be enabled only when the wake model
was trained with matching preprocessing. Personal models produced by the
bundled trainer normally use `WAKE_WORD_DENOISE=0`.

The repository does not bundle `models/hey_hyusk.onnx`. Run
`scripts/download-wake-word.sh` to fetch the temporary LiveKit `hey_livekit.onnx`
classifier and say **"Hey LiveKit"**. The detector uses the highest prediction
when the configured classifier does not emit a `hey_hyusk` key.

A missing or incompatible model logs a warning and disables only wake
detection. The rest of the runtime, the UI, and the `computer` tool still start.

After a detection, the detector pauses on a shared `WakeResume` signal. This
releases the microphone so `SpeechToText` can record the command. The runtime
calls `WakeResume::resume` after transcription, whether transcription succeeded
or failed, and the detector clears its audio buffer before resuming.

A new typed message cancels the active agent turn. During a running text-model
task, a wake detection starts a speech probe while the task continues; only a
nonempty spoken instruction replaces it. An empty/failed probe does not cancel
the task. Duplicate captures and stale follow-up events do not interrupt work.
Nova's active voice session keeps its own streaming microphone/barge-in path.
The Stop control remains immediate. Completed tool results stay available for
verification when an actual new instruction interrupts an action.

## Speech-to-text

`SpeechToText` in `src/speech/speech.rs`:

- selects the default CPAL input device;
- captures F32, I16, or U16 microphone samples;
- downmixes interleaved input to mono;
- resamples to 16 kHz;
- continuously suppresses noise with RNNoise when `STT_DENOISE=1`;
- uses neural speech probability with onset/offset hysteresis for endpointing;
- rejects recordings with no sustained speech and filters Whisper's blank/noise markers;
- pads short recordings with silence;
- transcribes with `whisper-rs` 0.16 using the model at `STT_MODEL`.

The runtime records up to eight seconds after wake, stopping after speech ends,
and applies a 45-second timeout around the whole transcription call. Capture
runs on a cancellable blocking worker, so Stop does not block the event loop.
Whisper requests wait asynchronously for a single inference permit; cancelled
waiters never start model loading or inference. The model is loaded lazily and
reused, with a fresh decoding state for each request. Nova's direct audio path
does not load Whisper.

`HYUSK_STT_TIMEOUT_SECS` defaults to 30 seconds and covers both waiting for the
worker and executing local transcription. A timeout, Stop, or a dropped request
signals a scoped native abort callback. Cancelled work cannot return a partial
transcript, and the worker retains its permit until native execution actually
ends, preventing overlapping abandoned inferences.

Native cancellation is cooperative between compute nodes, not a forced thread
kill. First-time model loading and an individual native compute operation may
still finish before the worker releases its resources. The outer 45-second
voice-runtime cap remains independent of this inference budget.

To verify the native abort path without recording or uploading audio, run:

```sh
cargo test --bin hyusk_agent \
  speech::speech::tests::whisper_native_cancel_reuses_cached_context \
  -- --ignored --exact --nocapture
```

This opt-in test requires `models/ggml-base.en.bin`. It warms the cached model,
runs synthetic PCM through native inference, drops the in-flight request,
checks that the native callback observes cancellation, and verifies a later
request can reuse the same model.

Empty transcripts, STT
errors, and timeouts are treated as a missed command: the detector resumes and
the UI returns to `Hidden`.

### Shared noise/echo-cancelled input

`./scripts/install-autostart.sh --voice-audio` installs the optional user-service
audio override. It routes wake and local STT through `hyusk_aec_source` using
PipeWire's `PIPEWIRE_NODE`, and routes Nova input/output through the matching
pair. The source uses WebRTC noise suppression and high-pass filtering, with
automatic gain increases disabled. No global default device is changed.
On service start the filter follows the selected physical source/sink; restart
Hyusk after changing microphones. `HYUSK_AEC_MASTER_SOURCE` and
`HYUSK_AEC_MASTER_SINK` can pin physical devices deliberately.

Do not stack wake RNNoise unnecessarily: the personal classifier's training
uses `WAKE_WORD_DENOISE=0`; the shared PipeWire source supplies the environmental
filtering. Noise suppression is not speaker identification and cannot reliably
remove someone else's speech or repair already clipped audio. Check capture
gain and mic boost if the log reports clipping; `scripts/set-mic-gain.sh 60 0`
is an adjustable starting point, not a universal calibration.

The default model path is `models/ggml-base.en.bin`. If the model cannot be
loaded, the runtime logs a warning and continues in text-only mode.

## Text-to-speech

`run_agent_task` calls `TextToSpeech::speak_cancellable`. On Linux this uses
`tokio::process` children with `kill_on_drop`, so a new message or wake word
stops playback instead of talking over the replacement request. The UI moves to
`Hidden` after playback completes or is cancelled.

On Linux it tries Piper first:

```text
PIPER_MODEL (default: models/en_US-lessac-medium.onnx)
        -> piper
        -> temporary WAV
        -> paplay or aplay
```

If Piper cannot run, it tries `espeak-ng`, then `espeak`. macOS uses the `say`
command and Windows uses PowerShell's speech synthesizer. These executables are
external to the Rust binary and must be installed separately.

## Desktop overlay

`ui::run` creates a 160 x 160 transparent, undecorated, always-on-top `eframe`
window. `OrbApp` draws a black circle with a state-colored ring and the
butterfly inside it. It requests a top-center position and focus while active;
GNOME Wayland may ignore both requests, so the compositor may place the window
elsewhere.

The butterfly changes motion and intensity for these states:

- `Waking`
- `Listening`
- `Thinking`
- `Working`
- `Speaking`

`Response` events are retained in memory but not drawn. The overlay is an
activity indicator rather than a complete agent interface.

A GNOME Shell extension can replace the orb with a color-coded butterfly in the
top bar. See [gnome-indicator.md](gnome-indicator.md).

## Remaining limitations

- Wake detection requires a compatible local classifier and is disabled when it
  is absent.
- Onset detection is heuristic: a command that starts quietly or lasts only a
  fraction of a second can be discarded as "no sustained speech", and capture is
  capped at eight seconds, so a long dictation is truncated.
- There is no echo cancellation. While the agent is producing output the clap
  scanner is disabled and the wake-word threshold is raised so it does not
  trigger on its own TTS, but the wake word stays live. During a text-model turn
  a wake word starts a speech probe while the turn continues; only a non-empty
  transcript replaces the turn and stops Linux TTS. Nova Sonic keeps its own
  streaming barge-in path.
- The `eframe` orb does not display transcripts or response text; the GNOME
  response card and the Noctalia tooltip show the latest response.
- The **Stop** control ends the active turn, and **Stop listening** ends an
  in-flight recording. Local Whisper cancellation is cooperative between
  compute nodes, so an individual native compute operation may still finish
  before its worker exits.
- GNOME Wayland input and screenshots use the desktop portals and require a
  one-time user approval dialog; see [tools.md](tools.md) for `computer`
  behavior.
- Microphone and TTS errors are logged but are not reflected as distinct UI
  states.

## Integration checklist

Useful next steps for the voice/UI layer:

1. Render the latest transcript or response in the `eframe` orb; the GNOME card
   and the Noctalia tooltip already show it.
2. Implement the desktop-portal screenshot approval flow for GNOME Wayland.
3. Surface microphone, STT, model, tool, and TTS failures as distinct states.
