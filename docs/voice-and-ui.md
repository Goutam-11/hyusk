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
   |  SpeechToText::transcribe_from_microphone(6.0)
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
   +-> UI event channel

The overlay receives state/tool/response events and draws an animated
butterfly. It does not render the response text.

Typed terminal lines bypass wake detection and STT and are delivered directly
as `UserInput`, so the text and `computer` tool paths remain available even
when the voice models are missing.
```

## Wake-word detector

`WakeWordDetector` uses CPAL to capture the default microphone, converts input
to mono, resamples it to 16 kHz, and runs `livekit-wakeword` inference in a
blocking worker. The detector keeps roughly 2.5 seconds of audio, runs
predictions periodically, applies a confidence threshold of 0.65, and uses a
two-second cooldown.

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
| `WAKE_WORD_THRESHOLD` | `0.4` | Detection confidence threshold. Lower is more sensitive. |
| `WAKE_WORD_MIN_RMS` | `0.003` | Adaptive noise-gate floor. Lower accepts quieter speech. |
| `WAKE_WORD_COOLDOWN_MS` | `1500` | Minimum time between detections. |
| `WAKE_WORD_DENOISE` | `1` | Run RNNoise denoising before scoring. Set `0` to disable. |

The detector tracks a slowly-moving noise floor, runs RNNoise denoising on each
inference window, peak-normalizes the result, and scores up to three default
model candidates (`hey_hyusk.onnx`, `hey_livekit.onnx`, `nihao_livekit.onnx`)
when they are present. A configured `WAKE_WORD_MODELS` list overrides that
discovery. RNNoise is particularly effective against stationary fan and hiss
noise; disable it with `WAKE_WORD_DENOISE=0` if it removes too much signal.

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

A wake word or a new typed message also cancels the active agent turn: the
model request, the running tool call, and Linux TTS playback are stopped, and
the conversation rolls back to the previous complete state.

## Speech-to-text

`SpeechToText` in `src/speech/speech.rs`:

- selects the default CPAL input device;
- captures F32, I16, or U16 microphone samples;
- downmixes interleaved input to mono;
- resamples to 16 kHz;
- rejects very quiet buffers;
- pads short recordings with silence;
- transcribes with `whisper-rs` using the model at `STT_MODEL`.

The runtime records a fixed six-second window after wake and applies a
45-second timeout around the whole transcription call. Empty transcripts, STT
errors, and timeouts are treated as a missed command: the detector resumes and
the UI returns to `Hidden`.

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

The older `TextToSpeech::speak_async` helper remains but is no longer used by
the runtime, so TTS completion now tracks the UI state.

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
- STT uses a fixed recording window rather than voice-activity detection, so a
  very short or delayed command can be missed.
- There is no echo cancellation. While the agent is producing output the clap
  scanner is disabled and the wake-word threshold is raised so it does not
  trigger on its own TTS, but the wake word stays live: saying "alexa" during a
  turn barges in, stops the agent, and starts listening again. A new wake word
  also stops Linux TTS.
- The UI does not display transcripts or response text.
- Tool calls and STT have no user-facing cancellation path.
- GNOME Wayland input and screenshots use the desktop portals and require a
  one-time user approval dialog; see [tools.md](tools.md) for `computer`
  behavior.
- Microphone and TTS errors are logged but are not reflected as distinct UI
  states.

## Integration checklist

Useful next steps for the voice/UI layer:

1. Add voice-activity detection so the recording window ends at natural
   silence.
2. Render the latest transcript or response in the overlay or a companion
   window.
3. Add a barge-in/cancellation path for long model or TTS operations.
4. Implement the desktop-portal screenshot approval flow for GNOME Wayland.
5. Surface microphone, STT, model, tool, and TTS failures as distinct states.
