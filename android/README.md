# Hyusk Android companion

This is a single-module Kotlin/Jetpack Compose Material 3 app (`io.github.hyusk.mobile`) targeting API 36 with minSdk 30. It is designed to pair with a trusted Hyusk provider over JSON-RPC/WebSocket and keeps provider settings and workflow/memory payloads encrypted with an Android Keystore AES-GCM key.

Open `android/` in Android Studio or run `./gradlew assembleDebug` with the Android SDK and JDK 17. The Gradle wrapper is checked in, so a system Gradle install is unnecessary.

In **Settings → Phone model**, choose Router, OpenAI, Bedrock, or Custom. Each
choice restores its own encrypted API key, model, and endpoint; the selection
applies immediately to the phone agent. Use **Refresh** to load that provider's
models, choose one, then **Save phone model**. Bedrock defaults to the
`us-east-1` Mantle endpoint; edit the region in its URL if needed. The phone
and laptop keep separate credentials and provider selections—they do not sync
API keys over the device link. Custom provider URLs require HTTPS, except for
loopback development endpoints.

**Nova live voice** is a separate opt-in path under Settings. Enter an AWS
region, a Nova Sonic Bedrock Runtime model ID, and a dedicated IAM access key
ID/secret key; then enable **Use Nova live voice for the microphone**. Unlike
the OpenAI-compatible Bedrock Mantle profile, this uses AWS-signed
bidirectional streaming directly from the phone, with live microphone input,
Nova's spoken output, interruption, and a `phone_task` tool that delegates
multi-step device work to the existing verified phone agent. It works without
the laptop link. The home mic and Android default-assistant overlay use this
mode when enabled; turning it off restores Android speech recognition/TTS.
Keys are encrypted by Android Keystore and never sent to the laptop. Create a
dedicated least-privilege IAM key with Bedrock model-invocation access. Nova
usage may incur AWS charges. The debug APK can be built without a connected
phone, but microphone routing, speaker echo cancellation, and AWS model access
must be verified on the device after installation.
Bedrock streams have a finite lifetime; when AWS closes a stream, Hyusk shows
an error and requires a tap to reconnect. Conversation context is not yet
carried automatically across that reconnect.

The accessibility, notification listener, voice interaction, microphone, and Quick Settings capabilities are opt-in Android system surfaces. The UI links to their settings pages and every action executor checks the process-wide emergency stop gate.
