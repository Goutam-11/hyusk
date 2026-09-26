# Hyusk Android Agent

The Android companion is a personal, sideloaded controller for Android 14–16.
It can execute deterministic phone actions locally, ask laptop Hyusk to reason
about a request, and call a separately configured OpenAI-compatible phone model
when the laptop is offline. The phone agent journals an observed run for up to
64 actions or ten minutes per execution window. Every model-selected operation goes through the
local action executor, and native results plus bounded accessibility snapshots
are returned to the model before it can declare the task complete. App
navigation and messaging are handled as inspectable UI steps rather than one
unverified model claim.

## Conversation memory and agent loop

Recent user/assistant turns and explicit durable facts are stored in Room with
payloads encrypted by an Android Keystore AES-GCM key. Hyusk retains up to 300
conversation turns and sends the model the newest 14 plus up to six older turns
selected by relevance, alongside up to eight relevant durable facts. The
current user request is always sent as the current user message; recalled
context is supplemental and cannot replace it.

Say `remember that ...` to save a durable fact, and ask `what do you remember
about me` to inspect saved facts. Model calls receive recent conversation,
relevant memories, the most recent bounded observations, and a bounded ledger
of earlier action signatures from the current run. A normal action may be
retried once, navigation/snapshot actions may repeat a few times. Checkpoints
are encrypted in Room, so `continue` or `resume` restores the original goal,
verified observations, and action ledger instead of starting the task again.

Phone and laptop storage are currently separate. Sending a phone request to
the paired laptop joins the laptop conversation, but automatic bidirectional
conversation and memory merge remains intentionally disabled until conflict,
deletion, and encryption semantics are defined.

Workflows use connected action cards rather than editable JSON. The Open app
card searches Android's real launcher catalog, stores the package identifier,
and can refresh after apps are installed or removed. Existing version-one step
arrays are migrated when opened and saved as version-two node definitions.

## Development setup

Install Android SDK 36, Platform-Tools, and JDK 17. Android Studio is optional;
the command-line SDK and Gradle wrapper are sufficient. The Fedora system JDK
may be newer than the Android Gradle Plugin supports, so point `JAVA_HOME` at a
JDK 17 runtime when building.

Check the machine before opening the project:

```bash
./scripts/check-android-env.sh
```

Then open `android/` in Android Studio or build from a shell:

```bash
cd android
./gradlew assembleDebug
./gradlew installDebug
```

For a local build, `models/alexa.onnx` is copied into the APK as `alexa.onnx`
by Gradle. The model remains ignored and is never committed. Settings includes
an Alexa phrase test using Android speech recognition; this validates the
microphone and language configuration while the low-power ONNX wake path is
calibrated with its matching feature-extractor assets.

With USB debugging enabled, install the debug APK with:

```bash
adb devices
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

The phone Settings screen accepts an OpenAI-compatible base URL, model name,
and API key. Credentials are encrypted with Android Keystore-backed storage;
use **Test selected model** to require a real, non-empty model reply (catalog
access alone is not enough).
For OpenRouter use `https://openrouter.ai/api/v1`, a model slug such as
`openai/gpt-4o-mini`, and paste the key with or without the `Bearer` prefix.
Hyusk normalizes either form, applies explicit model-call timeouts, and retries
one transient timeout, rate-limit, or provider failure. The searchable model
picker reads the provider's `/models` endpoint, keeps its encrypted cache for
six hours, supports manual refresh, and retains manual model-ID entry for
providers that do not expose a catalog.
For Bedrock Mantle use `https://bedrock-mantle.us-east-1.api.aws/v1` and a
Mantle model ID such as `openai.gpt-oss-20b`. The `-1:0` foundation-model ID
belongs to the separate Bedrock Runtime endpoint; saved profiles with that
suffix should be changed in Settings before testing.

### Optional Alexa background wake

Settings → **Enable Alexa in background** starts an opt-in foreground service.
Android shows an ongoing microphone notification while Hyusk listens in short
speech-recognition windows. When it hears “Alexa”, it opens Hyusk and starts
the command listener; text after the wake word is passed directly into the
current turn. Disable it from the same button or the notification's Stop
action. This fallback is visible and permission-gated, but less battery
efficient than a device-specific low-power ONNX wake pipeline. Android may
still stop it under aggressive battery policies, so the assistant gesture or
Quick Settings tile remains the reliable fallback.

The APK is intended for a personal device. Do not distribute a build containing
your model-provider credential or pairing state.

## First-run permissions

The onboarding page links to each Android settings screen. Enable only the
capabilities you intend to use:

1. Microphone for push-to-talk, speech recognition, and optional wake word.
2. Notification permission for task state and emergency stop.
3. Accessibility service for semantic UI inspection, click, text, scrolling,
   global navigation, taps, and swipe gestures.
4. Notification access only if Hyusk should read notification titles or offer
   official inline replies.
5. Default digital assistant if the power-button/assistant gesture and the
   most reliable background voice lifecycle are desired.

Accessibility does not bypass Android security. Secure windows, protected
authentication/payment UI, and apps with incomplete accessibility metadata may
not be controllable. Hyusk does not use root or read another app's private data.

## Pairing with Fedora Hyusk

Start the laptop link explicitly:

```bash
export HYUSK_LINK_ENABLED=1
export HYUSK_LINK_BIND=0.0.0.0:4488
cargo run
```

Open the laptop Hyusk extension → **Connect phone → Generate fresh code**, copy
the code, and transfer it privately to the phone's Devices screen. Use
**Enable laptop link** there once if the link is off. The phone rejects expired
codes, pins the laptop TLS identity, and generates a random device secret stored
through Android's Keystore-backed encrypted storage. Pairing secrets expire and
are never used as permanent bearer credentials.

The Devices screen is the single place for pairing and connection management.
It accepts the copied JSON, then saves and connects in one operation. Settings links back to Devices
instead of maintaining a second, disconnected pairing form.

## Local voice workflows

The Flows screen builds ordered shortcuts with native action cards; users never
need to edit JSON. A shortcut always responds to its name and may have additional
comma-separated voice phrases, such as `start study mode` or `begin focus`.
Matching and execution happen locally before any model request, so deterministic
workflows continue to run during provider outages or rate limits. Runs record the
actual number of completed actions on the Activity page.

For LAN use, allow TCP port 4488 only on the trusted Fedora zone. Prefer a
Tailscale address when away from home; Hyusk does not require a public relay.
Revoke a lost device from laptop link settings before pairing it again.

## Wake model assets

Personal wake recordings and ONNX models remain ignored by Git. The Android
project reserves an ignored asset path for the phone-specific ONNX pipeline;
the current release uses Android's assistant gesture, Quick Settings, and
push-to-talk as reliable invocation paths while the phone feature-extractor
assets are calibrated separately. Keep laptop and phone thresholds separate:
microphone processing and physical placement differ substantially.

Push-to-talk, the Quick Settings tile, and the assistant gesture remain
available if continuous software wake is stopped by Android, battery policy, or
microphone privacy controls. Wake capture pauses while Hyusk speaks so its own
voice cannot retrigger it.

## Safety model

Navigation, app launching, links, web search, media, volume, timers, alarms,
camera launch, dialer/message composition, sharing, clipboard writes, and
read-only device status run immediately. Hyusk asks before sensitive actions
such as sending, paying, purchasing, deleting, uninstalling, transferring, or
publishing through a generic UI action. Dial and message actions open the
system composer; they do not directly place a call or send a message.

The emergency-stop action in the ongoing notification and Quick Settings tile
cancels tasks, disables remote execution for the current session, and stops
microphone capture. Logs omit API keys, pairing secrets, and message bodies.

## Troubleshooting

- **Laptop unavailable:** verify both devices can reach the displayed LAN or
  Tailscale address and that Fedora's selected firewall zone permits port 4488.
- **Clicks do nothing:** open Android Accessibility settings and verify Hyusk is
  enabled. Some apps expose no useful semantic nodes; screenshot-assisted
  gestures are the fallback.
- **Wake stops in background:** make Hyusk the default assistant, allow its
  ongoing microphone notification, and remove vendor battery restrictions.
- **Speech recognition unavailable:** install/download the device's `en-IN`
  speech model or use the installed network recognizer.
- **A voice request runs twice:** install the newest APK. Final speech results
  carry a unique event ID and identical commands are suppressed for four
  seconds; older builds could submit both the partial and final transcript.
- **OpenRouter returns 401:** re-enter the key without surrounding quotes and
  verify the model ID. A 402 means the account needs credits; 429 means it is
  temporarily rate-limited.
