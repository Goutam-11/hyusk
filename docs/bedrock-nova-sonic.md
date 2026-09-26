# Amazon Nova Sonic on the laptop agent

Hyusk's `bedrock-sonic` provider uses Bedrock's signed bidirectional
`InvokeModelWithBidirectionalStream` API. It does not use a Bedrock API key or
the OpenAI-compatible chat endpoint. The AWS Rust SDK reads credentials from
its normal credential chain, including AWS CLI shared profiles and SSO.

## Requirements

- An AWS CLI profile with access to Amazon Bedrock in a supported region.
- Access enabled for the `amazon.nova-2-sonic-v1:0` model and permission to
  invoke it (`bedrock:InvokeModel`).
- `AWS_REGION`/`AWS_DEFAULT_REGION`, or a region configured on the selected AWS
  profile. Set `AWS_PROFILE` if the credentials are not in the default profile.
- A working microphone and `aplay` (ALSA utilities) for response audio on
  Linux, or `paplay` when `HYUSK_OUTPUT_DEVICE` selects a sink. Hyusk streams
  denoised mono 16 kHz PCM as it records and plays Nova Sonic's mono 24 kHz
  reply chunks as they arrive.
- Whisper loads only when local transcription is used. Nova Sonic's microphone
  capture does not require the Whisper model.

If the profile uses IAM Identity Center, log in first with
`aws sso login --profile <profile-name>`.

## Select and use it

Start Hyusk normally, open the GNOME extension's model/provider selector, and
choose **Bedrock Sonic → Nova 2 Sonic · Speech to speech**. If another provider
is already configured, Hyusk keeps it as the default; selecting Sonic switches
the active laptop agent. Returning to OpenRouter, OpenAI, or Bedrock API mode is
done from the same selector. No mobile changes are involved.

The provider handles microphone audio and typed input. Sonic tool-use requests
are routed through Hyusk's existing local tool registry, approval policy,
execution deadlines, and action journal; tool results are sent back into the
same Sonic stream so it can continue the task. A request requiring approval
waits for a clear confirmation before the stored action is executed.

After a wake word, the live Sonic microphone and Bedrock stream remain open
across turns. Hyusk returns to wake-word mode after 30 seconds without a new
utterance, or immediately when you say “go away” or “stop listening.” Hyusk's
local speech-onset detector stops queued playback as you begin speaking; Nova's
interruption event then confirms the turn change so a new utterance can take
over. Repeated identical tool actions with unchanged results pause rather than
running indefinitely.
Other chat providers use local speech recognition and reopen a 30-second
follow-up listening window after each answer; they do not provide Sonic's
full-duplex speech stream.

On laptop speakers, route both Hyusk's input and output through a paired echo
canceller. `scripts/ensure-echo-cancel.sh` creates `hyusk_aec_source` and
`hyusk_aec_sink` without changing system defaults. Set
`HYUSK_SONIC_INPUT_DEVICE=hyusk_aec_source`,
`HYUSK_OUTPUT_DEVICE=hyusk_aec_sink`, and `PULSE_SINK=hyusk_aec_sink` for
Hyusk. The included `scripts/hyusk-aec-service.conf` is a user-service
drop-in for this checkout; install it in
`~/.config/systemd/user/hyusk.service.d/10-aec.conf`, reload the user daemon,
and restart Hyusk. The echo-cancel module will be re-created on service start.
Use headphones instead if the echo-cancelled source is unavailable or clips
your voice. Interruption quality still depends on the local microphone and
PipeWire's cancellation, and should be checked with a short live conversation.

## Test the AWS path

This performs a real Bedrock inference and may incur AWS charges. It captures
up to eight seconds from the microphone, prints Sonic's recognized and returned
text, and plays the generated audio. It does not execute tools.

Set `HYUSK_SONIC_DEBUG=1` to print event names, audio frame counts, and PCM
energy levels. It does not print audio payloads or credentials.

To check one read-only tool round trip, add `HYUSK_NOVA_SONIC_TEST_TOOL=1` and
ask, “What time is it?” The diagnostic exposes only a `get_time` tool and
returns the current local time; it does not run the agent's normal tools.

```bash
AWS_PROFILE=<profile-name> AWS_REGION=<region> \
  HYUSK_NOVA_SONIC_TEST=1 cargo run --bin hyusk_agent
```

Use the default profile by omitting `AWS_PROFILE`. Override the model only if
you have access to another compatible Sonic model:

```bash
BEDROCK_SONIC_MODEL=amazon.nova-2-sonic-v1:0
```

For a repeatable stream test without waiting at the microphone, point the
diagnostic at a standard 16 kHz mono PCM16 WAV file. The recording is sent to
AWS at real-time cadence. This tests the Bedrock stream and chunked playback,
but not live microphone capture or denoising:

```bash
HYUSK_NOVA_SONIC_TEST=1 \
  HYUSK_NOVA_SONIC_TEST_WAV=/path/to/recording.wav \
  cargo run --bin hyusk_agent
```

For a typed-input diagnostic, supply a text request. It sends no audio or
tools, but Sonic may wait for an active audio stream and time out, so this
does not establish that the speech path works:

```bash
AWS_PROFILE=<profile-name> AWS_REGION=<region> \
  HYUSK_NOVA_SONIC_TEST=1 \
  HYUSK_NOVA_SONIC_TEST_TEXT='Reply with one short greeting.' \
  cargo run --bin hyusk_agent
```

Without a live AWS request, run the local Rust checks:

```bash
cargo test -p hyusk_agent model::bedrock_sonic
cargo check -p hyusk_agent
```

If startup says Nova Sonic is unavailable, check the region/profile first. If
the inference request is denied, verify model access and `bedrock:InvokeModel`;
AWS CLI authentication alone does not grant Bedrock model access.
