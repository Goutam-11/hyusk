# Laptop agent refactor and bug-fix pass

Scope: laptop orchestration, conversation persistence, OpenAI-compatible
provider decoding, and browser/process tools. This pass does not change the
Android app. See [architecture.md](architecture.md) for the module ownership
map and debugging guide.

## Verified issues addressed

| Issue | Correction | Regression coverage |
| --- | --- | --- |
| Browser executable guesses failed on Flatpak installs and the model described failures as approval requests. | Normalize Chrome aliases, retain spawn error causes, distinguish execution errors from runtime approval in prompts and tool feedback. | App aliases, missing executable errors, routine-launch policy. |
| Prose followed by a fenced response envelope leaked JSON into the spoken/displayed answer. | Decode a terminal structured envelope; keep ordinary JSON examples intact. Strip provider control tags from tool preambles. | Reply parsing and preamble tests. |
| Recorder timeout dropped an async future without stopping its blocking capture worker. | Cancel the capture child token, not the outer listen lifecycle, so completion and UI cleanup still run. | Timeout and external-cancellation tests. |
| Nova early returns could detach microphone/audio workers. | Cancel-on-drop capture guard and abort-on-drop audio-sender guard; retain normal shutdown awaits. | Audio/capture cleanup guard test. |
| A failed browser text insertion could erase the original input first. | Select existing content without clearing it; reject hidden or unsupported controls before insertion. | Fill preparation script test and the opt-in live CDP fixture. |
| Session saves silently failed and reused a shared temporary filename. | Log failure category, use unique create-new temporary files with private permissions before writing, atomically rename, and clean up only the owned temporary file. | Roundtrip/permissions, injected write failure, injected rename failure. |
| Streamed provider errors and refusal text were lost behind generic empty-response errors. | Preserve explicit error/refusal information, reject unexpected roles, and cap streaming wire data at 4 MiB. | Local HTTP stream fixtures, including an oversized unterminated stream. |
| Some nominally read-only shell commands could write files or launch external helpers. | Classify output/script options, preserving routine inspection commands and ordinary filenames. | Public approval-policy tests, including trailing flags and long-option abbreviations. |

## Second pass: speech, desktop bridge, scheduling, and delegation

- **Accessibility:** malformed non-object requests no longer kill the
  persistent Python bridge. Find/read requests are bounded, negative match
  indices are rejected, rejected focus/text operations are reported, and
  coordinate-click fallbacks require nonempty bounds. Window D-Bus calls have
  a five-second limit; timed-out mutations explicitly have unknown outcomes.
  Interrupted requests discard their owned bridge rather than leaving a stale
  reply for the next call; the deadline includes stdin writes and flushing.
- **Scheduling:** a worker starts only after registration, preventing an
  immediately due action from being aborted while delivering its event. A
  failure to persist removal leaves the schedule armed for a bounded retry
  rather than firing with an old pending entry still on disk.
- **Device pairing:** failed device-store writes restore memory state and
  release the reserved pairing secret. Successful pairing consumes the
  secret; concurrent requests cannot reuse a reserved secret.
- **Background tasks:** running/queued jobs are capped at sixteen, shutdown
  cancels workers, and a cancelled task cannot claim a late completion.
  Completion senders retain their execution permits while waiting for event
  delivery, bounding blocked senders without discarding reports on an
  arbitrary short timeout.
- **Speech:** cancellation is observed during blocked Piper input writes and
  full live-audio queues. Capture pipe reads no longer prevent cancellation.
  PCM16 bytes are aligned across chunk boundaries, with a final incomplete
  sample reported as an error. Speech cleanup preserves combining marks for
  supported Indian scripts instead of stripping vowel signs.
- **Media:** leading-sign seek offsets are converted to playerctl's
  trailing-sign relative notation. Invalid offsets and repeat modes fail
  before subprocess execution; empty playerctl errors retain an exit-status
  explanation.

Regression coverage includes Python AT-SPI fixtures, full-channel audio
cancellation/order tests, scheduler event-backpressure tests, failed pairing
persistence/retry, delegation admission/cancellation, and seek normalization.
These checks do not send phone actions or use paid model requests.

The subsequent Whisper upgrade replaces the legacy 0.5 binding with 0.16 and
adds a scoped native abort callback. Queued requests are cancellable and old
inference is asked to stop cooperatively. Model loading and an individual
native compute operation are not forcibly interrupted; the execution permit
is retained until the native worker exits. See [voice-and-ui.md](voice-and-ui.md)
for the timeout and lifecycle configuration.

The later agent-loop comparison and interaction changes are documented in
[agent-loop-research.md](agent-loop-research.md). It records the upstream
DeepSeek Harness, Pi, and OpenCode patterns reviewed, the interruption/schema
fixes applied, and the remaining long-turn context and observability work.

## Validation boundaries

- Unit and local HTTP fixtures do not require a paid model call.
- The live CDP fixture creates and closes only its own local test tab; it does
  not enumerate and close user tabs.
- Microphone quality, acoustic interruption, provider-specific behavior, and
  app-specific accessibility still need live validation. Passing tests do not
  establish that every desktop task will succeed.
- Session schema versions 1 and 2 remain readable. Temporary-file replacement
  is atomic; this is not a cross-process conversation merge or a guarantee of
  durability through sudden power loss.
- Text-model and Nova loops remain distinct because their input/output
  lifecycles differ. Both use the existing shared tool registry and approval
  classifier; extracting them does not grant new permissions.
