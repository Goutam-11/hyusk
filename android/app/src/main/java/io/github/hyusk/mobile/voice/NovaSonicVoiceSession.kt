package io.github.hyusk.mobile.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Build
import android.media.audiofx.AcousticEchoCanceler
import android.util.Base64
import android.util.Log
import androidx.core.content.ContextCompat
import aws.sdk.kotlin.runtime.auth.credentials.StaticCredentialsProvider
import aws.sdk.kotlin.services.bedrockruntime.BedrockRuntimeClient
import aws.sdk.kotlin.services.bedrockruntime.model.BidirectionalInputPayloadPart
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamInput
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamRequest
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamOutput
import aws.sdk.kotlin.services.bedrockruntime.model.ModelStreamErrorException
import aws.smithy.kotlin.runtime.auth.awscredentials.Credentials
import io.github.hyusk.mobile.security.NovaSonicSettings
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ClosedSendChannelException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.consumeAsFlow
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

private data class PendingPhoneTool(
    val id: String,
    val name: String,
    val request: String?,
    val validationError: String?,
)

internal fun isBedrockStreamCancellation(failure: Throwable): Boolean {
    var cause: Throwable? = failure
    while (cause != null) {
        if (cause.javaClass.simpleName == "StreamResetException" &&
            cause.message?.contains("CANCEL", ignoreCase = true) == true) return true
        cause = cause.cause
    }
    return false
}

internal fun isUncaughtBedrockWriterReset(failure: Throwable): Boolean =
    isBedrockStreamCancellation(failure) && failure.stackTrace.any {
        it.className.startsWith("aws.smithy.kotlin.runtime.http.engine.okhttp.") ||
            it.className.startsWith("aws.smithy.kotlin.runtime.io.SdkByteReadChannel")
    }

internal fun bedrockCredentialKind(accessKeyId: String): String = when {
    accessKeyId.startsWith("ABSK") -> "bedrock-api-key-unsupported-for-bidirectional-stream"
    accessKeyId.startsWith("ASIA") -> "temporary-iam-credentials-require-session-token"
    accessKeyId.startsWith("AKIA") -> "iam-access-key"
    else -> "unknown-credential-format"
}

/**
 * The Smithy/OkHttp request-body coroutine is owned by the SDK, not our Nova
 * scope. A server-side HTTP/2 CANCEL can escape that coroutine even when the
 * stream call is caught, and Android's default handler then kills the whole
 * process (including Accessibility). Keep the original handler for every
 * other failure; only this already-terminated SDK writer is recoverable.
 */
private object BedrockWriterCrashGuard {
    @Synchronized fun install() {
        if (Thread.getDefaultUncaughtExceptionHandler() is Handler) return
        Thread.setDefaultUncaughtExceptionHandler(Handler(Thread.getDefaultUncaughtExceptionHandler()))
    }

    private class Handler(
        private val fallback: Thread.UncaughtExceptionHandler?,
    ) : Thread.UncaughtExceptionHandler {
        override fun uncaughtException(thread: Thread, failure: Throwable) {
            if (isUncaughtBedrockWriterReset(failure)) {
                Log.w("HyuskNovaVoice", "Bedrock request writer reset; keeping the app alive", failure)
                return
            }
            fallback?.uncaughtException(thread, failure)
        }
    }
}

/** One live, signed Bedrock stream. The microphone stays open across turns. */
class NovaSonicVoiceSession(private val context: Context) : AutoCloseable {
    private val instanceId = UUID.randomUUID().toString().take(8)
    private val streamExceptionHandler = CoroutineExceptionHandler { _, failure ->
        if (isBedrockStreamCancellation(failure)) {
            Log.w(TAG, "Bedrock live stream reset during shutdown", failure)
            mutableState.value = mutableState.value.copy(
                listening = false,
                speaking = false,
                error = "Nova voice stream closed while stopping. Tap the microphone to reconnect.",
            )
        } else {
            Log.e(TAG, "Uncaught Nova voice coroutine failure", failure)
            mutableState.value = mutableState.value.copy(
                listening = false,
                speaking = false,
                error = "Nova voice failed: ${failure.message ?: failure.javaClass.simpleName}",
            )
        }
    }
    private val scope = CoroutineScope(Dispatchers.Main.immediate + SupervisorJob() + streamExceptionHandler)
    private val mutableState = MutableStateFlow(VoiceState())
    val state: StateFlow<VoiceState> = mutableState
    private var session: Job? = null
    private var input: Channel<InvokeModelWithBidirectionalStreamInput>? = null
    private var audioOutput: Channel<ByteArray>? = null
    private var playback: AudioTrack? = null
    private val playbackLock = Any()
    private var playbackDrainJob: Job? = null
    @Volatile private var playbackBusy = false
    @Volatile private var playbackFramesWritten = 0L
    @Volatile private var playbackVersion = 0L
    @Volatile private var playbackHeadOrigin = 0L
    @Volatile private var recorder: AudioRecord? = null
    @Volatile private var recorderJob: Job? = null
    private var generation = 0L
    private var lastActivity = 0L
    private var outputRole = ""
    private var outputStage = ""
    private var outputType = ""
    private var pendingTool: PendingPhoneTool? = null
    private val handledToolIds = LinkedHashSet<String>()
    private var toolScope: CoroutineScope? = null
    private var promptName = ""
    private var runId = ""
    @Volatile private var promptStartedGeneration = -1L
    @Volatile private var audioStartedGeneration = -1L
    @Volatile private var microphoneFrames = 0L
    @Volatile private var audibleFrames = 0L
    @Volatile private var responseEvents = 0L
    @Volatile private var audioInputBytes = 0L
    @Volatile private var firstResponseElapsedMs = -1L
    @Volatile private var runStartedAtMs = 0L
    @Volatile private var toolInProgress = false
    @Volatile private var turnInProgress = false
    private var stopTask: (() -> Unit)? = null

    fun start(settings: NovaSonicSettings, onPhoneTask: suspend (String) -> String, onStopTask: () -> Unit = {}) {
        BedrockWriterCrashGuard.install()
        val previousSession = session
        stop()
        val currentGeneration = generation
        runId = UUID.randomUUID().toString().take(8)
        runStartedAtMs = android.os.SystemClock.elapsedRealtime()
        stopTask = onStopTask
        turnInProgress = false
        pendingTool = null
        handledToolIds.clear()
        microphoneFrames = 0L
        audibleFrames = 0L
        responseEvents = 0L
        audioInputBytes = 0L
        firstResponseElapsedMs = -1L
        val credentialKind = bedrockCredentialKind(settings.accessKeyId)
        Log.i(TAG, "[$instanceId/$runId] start model=${settings.modelId} region=${settings.region} " +
            "credentialKind=$credentialKind generation=$currentGeneration")
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            mutableState.value = VoiceState(error = "Microphone permission is required for Nova voice.")
            return
        }
        if (settings.accessKeyId.isBlank() || settings.secretAccessKey.isBlank()) {
            mutableState.value = VoiceState(error = "Add your AWS IAM key in Settings to use Nova voice.")
            return
        }
        if (credentialKind.startsWith("bedrock-api-key")) {
            mutableState.value = VoiceState(error = "Nova live voice requires an AWS IAM access-key ID and secret, not a Bedrock API key.")
            return
        }
        if (credentialKind.startsWith("temporary-iam")) {
            mutableState.value = VoiceState(error = "Nova live voice needs an IAM session token for temporary credentials. Use a long-lived IAM access key or add session-token support.")
            return
        }
        mutableState.value = VoiceState(listening = true)
        lastActivity = System.currentTimeMillis()
        session = scope.launch {
            try {
                previousSession?.join()
                stream(settings, onPhoneTask, currentGeneration)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                val streamError = failure as? ModelStreamErrorException
                Log.e(TAG, "[$instanceId/$runId] session failed type=${failure.javaClass.simpleName} " +
                    "originalStatus=${streamError?.originalStatusCode} originalMessage=${streamError?.originalMessage} " +
                    "awsErrorCode=${streamError?.sdkErrorMetadata?.errorCode} " +
                    "awsErrorType=${streamError?.sdkErrorMetadata?.errorType} " +
                    "awsRequestId=${streamError?.sdkErrorMetadata?.requestId} " +
                    "elapsedMs=${android.os.SystemClock.elapsedRealtime() - runStartedAtMs} " +
                    "micFrames=$microphoneFrames audibleFrames=$audibleFrames audioBytes=$audioInputBytes " +
                    "responseEvents=$responseEvents firstResponseMs=$firstResponseElapsedMs", failure)
                if (generation == currentGeneration) mutableState.value = mutableState.value.copy(
                    listening = false, speaking = false,
                    error = "Nova voice failed: ${failure.message ?: failure.javaClass.simpleName}",
                )
            } finally {
                if (generation == currentGeneration) {
                    input?.close()
                    input = null
                    audioOutput?.close()
                    audioOutput = null
                    playbackDrainJob?.cancel()
                    releasePlayback()
                    mutableState.value = mutableState.value.copy(listening = false, speaking = false)
                }
            }
        }
    }

    private suspend fun stream(settings: NovaSonicSettings, onPhoneTask: suspend (String) -> String, currentGeneration: Long) {
        val provider = StaticCredentialsProvider(Credentials(settings.accessKeyId, settings.secretAccessKey))
        val client = BedrockRuntimeClient {
            region = settings.region
            credentialsProvider = provider
        }
        try {
            val channel = Channel<InvokeModelWithBidirectionalStreamInput>(Channel.BUFFERED)
            input = channel
            promptName = "hyusk-${UUID.randomUUID()}"
            val systemName = "hyusk-system"
            send(JSONObject().put("sessionStart", JSONObject()
                .put("inferenceConfiguration", JSONObject().put("maxTokens", 2048).put("topP", 0.9).put("temperature", 0.4))
                .put("turnDetectionConfiguration", JSONObject().put("endpointingSensitivity", "MEDIUM"))))
            val toolSpec = JSONObject().put("name", PHONE_DELEGATION_TOOL)
                .put("description", "Delegate the user's complete phone request to Hyusk's autonomous Android agent. This accepts arbitrary phone tasks; it is not a fixed task whitelist. The delegated agent has separate tools for installed apps, the accessibility tree, screenshot OCR, taps, text entry, scroll/gestures, media controls, browser/search, system navigation, timers, and memory. Wait for its actual result and report blockers accurately.")
                // Preserve the exact wire shape used by the last working phone
                // build. Nova Sonic's streaming tool schema documents have
                // conflicting string-vs-object examples; changing it while
                // diagnosing audio had risked destabilizing the live path.
                .put("inputSchema", JSONObject().put("json",
                    "{\"type\":\"object\",\"properties\":{\"request\":{\"type\":\"string\"}},\"required\":[\"request\"]}"))
            val toolConfiguration = JSONObject().put("tools", org.json.JSONArray().put(JSONObject().put("toolSpec", toolSpec)))
                .put("toolChoice", JSONObject().put("auto", JSONObject()))
            send(JSONObject().put("promptStart", JSONObject()
                .put("promptName", promptName)
                .put("textOutputConfiguration", JSONObject().put("mediaType", "text/plain"))
                .put("audioOutputConfiguration", JSONObject().put("mediaType", "audio/lpcm")
                    .put("sampleRateHertz", 24000).put("sampleSizeBits", 16).put("channelCount", 1)
                    .put("voiceId", "matthew").put("encoding", "base64").put("audioType", "SPEECH"))
                .put("toolUseOutputConfiguration", JSONObject().put("mediaType", "application/json"))
                .put("toolConfiguration", toolConfiguration)))
            promptStartedGeneration = currentGeneration
            send(JSONObject().put("contentStart", textStart(systemName, "SYSTEM", false)))
            send(JSONObject().put("textInput", content(systemName,
                "You are Hyusk, Goutam Sharma’s concise voice assistant. For any request to inspect or operate the phone—including arbitrary app, screen, text-entry, gesture, media, browser, timer, and system tasks—call delegate_phone_agent with the user's complete request. This tool delegates to Hyusk's autonomous phone agent; it is not a list of tasks you are limited to. Do not tell the user you lack phone-control capabilities merely because this voice layer has one delegation tool. Wait for the delegated agent's verified result, then report what actually happened or its specific blocker. Never claim success from a dispatched action alone. Answer ordinary conversation naturally. If the user says go away, stop the task and end the voice session.")))
            send(JSONObject().put("contentEnd", end(systemName)))

            val workerScope = CoroutineScope(currentCoroutineContext())
            toolScope = workerScope
            val microphoneJob = workerScope.launch(Dispatchers.IO) { recordMicrophone(currentGeneration) }
            this.recorderJob = microphoneJob
            val outputChannel = Channel<ByteArray>(24)
            audioOutput = outputChannel
            var sdkInputEvents = 0L
            val playbackJob = workerScope.launch(Dispatchers.IO) {
                for (pcm in outputChannel) {
                    if (generation != currentGeneration) break
                    playbackBusy = true
                    try {
                        try {
                            play(pcm, currentGeneration)
                            if (mutableState.value.error == SPEAKER_INTERRUPTED_MESSAGE) {
                                mutableState.value = mutableState.value.copy(error = null)
                            }
                        } catch (failure: Exception) {
                            if (failure is CancellationException) throw failure
                            // A speaker route change or flush can make one write
                            // fail. Do not let the playback child cancel Nova's
                            // microphone and network session.
                            Log.w(TAG, "[$instanceId/$runId] speaker write failed; resetting track: ${failure.message}", failure)
                            releasePlayback()
                            mutableState.value = mutableState.value.copy(
                                speaking = false,
                                error = SPEAKER_INTERRUPTED_MESSAGE,
                            )
                        }
                    } finally {
                        playbackBusy = false
                    }
                }
            }
            val idleJob = workerScope.launch {
                while (isActive) {
                    delay(1_000)
                    if (System.currentTimeMillis() - lastActivity > 30_000L &&
                        !mutableState.value.speaking && !toolInProgress && !turnInProgress) {
                        stop()
                        break
                    }
                }
            }
            try {
                // The SDK can perform synchronous socket work before its suspend
                // points. An exception handler alone does not leave Main.
                withContext(Dispatchers.IO + streamExceptionHandler) {
                    Log.i(TAG, "[$instanceId/$runId] invoking bidirectional stream; queued initialization events")
                    client.invokeModelWithBidirectionalStream(InvokeModelWithBidirectionalStreamRequest {
                        modelId = settings.modelId
                        body = channel.consumeAsFlow().onEach { item ->
                            val bytes = (item as? InvokeModelWithBidirectionalStreamInput.Chunk)?.value?.bytes
                            val event = bytes?.let { runCatching { JSONObject(it.toString(Charsets.UTF_8)).optJSONObject("event") }.getOrNull() }
                            val name = event?.keys()?.asSequence()?.firstOrNull() ?: "unknown"
                            sdkInputEvents++
                            if (name != "audioInput" || sdkInputEvents == 1L || sdkInputEvents % 32L == 0L) {
                                Log.d(TAG, "[$instanceId/$runId] sdk-input[$sdkInputEvents] event=$name bytes=${bytes?.size ?: 0} " +
                                    "audioFrames=$microphoneFrames audioBytes=$audioInputBytes")
                            }
                        }
                    }) { response ->
                        Log.i(TAG, "[$instanceId/$runId] response stream opened after ${android.os.SystemClock.elapsedRealtime() - runStartedAtMs}ms")
                        response.body?.collect { chunk ->
                            val bytes = (chunk as? InvokeModelWithBidirectionalStreamOutput.Chunk)?.value?.bytes ?: return@collect
                            handleEvent(JSONObject(bytes.toString(Charsets.UTF_8)).optJSONObject("event") ?: return@collect, onPhoneTask)
                        }
                    }
                }
                throw IllegalStateException("Nova closed the live stream. Tap the microphone to start a new conversation.")
            } finally {
                microphoneJob.cancelAndJoin()
                Log.i(TAG, "[$instanceId/$runId] stream cleanup microphoneFrames=$microphoneFrames audioBytes=$audioInputBytes sdkInputs=$sdkInputEvents responseEvents=$responseEvents")
                if (this.recorderJob === microphoneJob) this.recorderJob = null
                idleJob.cancel()
                outputChannel.close()
                playbackJob.cancelAndJoin()
                toolScope = null
            }
        } finally {
            withContext(Dispatchers.IO + NonCancellable) { client.close() }
        }
    }

    private suspend fun recordMicrophone(currentGeneration: Long) {
        val minBuffer = AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        require(minBuffer > 0) { "Microphone does not support 16 kHz PCM" }
        val recorder = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, 16000,
            AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(minBuffer, 4096))
        require(recorder.state == AudioRecord.STATE_INITIALIZED) { "Could not open microphone" }
        this.recorder = recorder
        val echoCanceller = if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(recorder.audioSessionId) else null
        echoCanceller?.enabled = true
        Log.i(TAG, "[$instanceId/$runId] mic opened source=VOICE_COMMUNICATION rate=16000 channels=1 buffer=$minBuffer " +
            "audioSession=${recorder.audioSessionId} echoCancellerAvailable=${AcousticEchoCanceler.isAvailable()} echoCancellerEnabled=${echoCanceller?.enabled == true}")
        val audioName = "hyusk-audio"
        try {
            send(JSONObject().put("contentStart", JSONObject().put("promptName", promptName)
                .put("contentName", audioName).put("type", "AUDIO").put("interactive", true).put("role", "USER")
                .put("audioInputConfiguration", JSONObject().put("mediaType", "audio/lpcm")
                    .put("sampleRateHertz", 16000).put("sampleSizeBits", 16).put("channelCount", 1)
                    .put("audioType", "SPEECH").put("encoding", "base64"))))
            audioStartedGeneration = currentGeneration
            recorder.startRecording()
            val frame = ByteArray(1024)
            while (currentCoroutineContext().isActive && input != null && generation == currentGeneration) {
                val count = recorder.read(frame, 0, frame.size)
                if (count < 0) {
                    // stop() interrupts AudioRecord.read() with ERROR_INVALID_OPERATION
                    // on some devices. Do not spin/log until the stream coroutine exits.
                    if (!currentCoroutineContext().isActive || recorder.recordingState != AudioRecord.RECORDSTATE_RECORDING) break
                    throw IllegalStateException("Microphone read failed ($count)")
                }
                if (count == 0) continue
                microphoneFrames++
                if ((0 until count - 1 step 2).any { index ->
                        val sample = (frame[index].toInt() and 0xff) or (frame[index + 1].toInt() shl 8)
                        kotlin.math.abs(sample.toShort().toInt()) > 500
                    }) audibleFrames++
                audioInputBytes += count
                val pcm = if (count == frame.size) frame else frame.copyOf(count)
                // Keep capturing while playback runs. Nova's server-side turn
                // detector emits the authoritative interruption event; a local
                // RMS gate here can mistake speaker echo for the user and mute the
                // rest of an otherwise valid response until the next completion.
                send(JSONObject().put("audioInput", content(audioName, Base64.encodeToString(pcm, Base64.NO_WRAP))))
                if (microphoneFrames == 1L || microphoneFrames % 32L == 0L) {
                    Log.d(TAG, "[$instanceId/$runId] mic frame=$microphoneFrames bytes=$count audibleFrames=$audibleFrames " +
                        "sinceStartMs=${android.os.SystemClock.elapsedRealtime() - runStartedAtMs}")
                }
            }
        } finally {
            runCatching { recorder.stop() }
            recorder.release()
            if (this.recorder === recorder) this.recorder = null
            echoCanceller?.release()
            // The session owner sends contentEnd, promptEnd, and sessionEnd in
            // order after this producer has stopped. Cleanup must not write to
            // a channel the SDK may no longer be draining.
        }
    }

    private suspend fun handleEvent(event: JSONObject, onPhoneTask: suspend (String) -> String) {
        responseEvents++
        if (firstResponseElapsedMs < 0L) {
            firstResponseElapsedMs = android.os.SystemClock.elapsedRealtime() - runStartedAtMs
            Log.i(TAG, "[$instanceId/$runId] first response event after ${firstResponseElapsedMs}ms")
        }
        val eventName = event.keys().asSequence().firstOrNull() ?: "unknown"
        if (eventName != "audioOutput" || responseEvents == 1L || responseEvents % 32L == 0L) {
            Log.d(TAG, "[$instanceId/$runId] response[$responseEvents] event=$eventName")
        }
        if (event.has("completionStart")) {
            playbackVersion++
            playbackDrainJob?.cancel()
        }
        event.optJSONObject("contentStart")?.let { start ->
            outputRole = start.optString("role")
            outputType = start.optString("type")
            outputStage = runCatching { JSONObject(start.optString("additionalModelFields")).optString("generationStage") }.getOrDefault("")
        }
        event.optJSONObject("textOutput")?.optString("content")?.takeIf(String::isNotBlank)?.let { text ->
            if (outputRole == "USER") {
                lastActivity = System.currentTimeMillis()
                turnInProgress = true
                mutableState.value = mutableState.value.copy(transcript = text)
                if (isVoiceStopCommand(text)) {
                    stopTask?.invoke()
                    stop()
                }
            } else if (outputStage != "SPECULATIVE") {
                mutableState.value = mutableState.value.copy(transcript = text)
            }
        }
        event.optJSONObject("audioOutput")?.optString("content")?.takeIf(String::isNotBlank)?.let { encoded ->
            val output = audioOutput
            if (output != null) {
                val pcm = Base64.decode(encoded, Base64.DEFAULT)
                playbackVersion++
                playbackDrainJob?.cancel()
                mutableState.value = mutableState.value.copy(speaking = true)
                try {
                    // Backpressure the event reader instead of silently dropping PCM
                    // when the phone's speaker is slower than Nova's stream.
                    output.send(pcm)
                } catch (_: ClosedSendChannelException) {
                    // Expected if the user stops the live session during playback.
                }
            }
        }
        event.optJSONObject("toolUse")?.let { tool ->
            if (outputRole != "TOOL" || outputType != "TOOL") {
                throw IllegalStateException("Nova returned toolUse outside a TOOL content block.")
            }
            if (pendingTool != null) {
                throw IllegalStateException("Nova returned a second toolUse before closing the first.")
            }
            val id = tool.optString("toolUseId").trim()
            if (id.isEmpty()) throw IllegalStateException("Nova toolUse is missing toolUseId.")
            if (!handledToolIds.add(id)) {
                throw IllegalStateException("Nova repeated toolUseId '$id'; refusing to run it twice.")
            }
            if (handledToolIds.size > 128) handledToolIds.remove(handledToolIds.first())

            val name = tool.optString("toolName").trim()
            var validationError: String? = if (tool.optString("role") != "TOOL") {
                "Tool event had an unexpected role."
            } else if (name.isEmpty()) {
                "Tool event is missing toolName."
            } else null
            var request: String? = null
            if (validationError == null) {
                val content = tool.opt("content") as? String
                val arguments = content?.let { raw ->
                    runCatching { JSONObject(raw) }.getOrElse {
                        validationError = "Tool arguments were not valid JSON."
                        null
                    }
                }
                if (content == null) validationError = "Tool arguments were missing."
                request = arguments?.optString("request")?.trim()?.takeIf(String::isNotEmpty)
                if (request == null && validationError == null) {
                    validationError = "$PHONE_DELEGATION_TOOL requires a non-empty request string."
                }
            }
            pendingTool = PendingPhoneTool(id, name, request, validationError)
        }
        val toolEnd = event.optJSONObject("contentEnd")
        if (toolEnd?.optString("type") == "TOOL") {
            if (toolEnd.optString("stopReason") != "TOOL_USE") {
                throw IllegalStateException("Nova closed a TOOL block with an unexpected stop reason.")
            }
            val tool = pendingTool
                ?: throw IllegalStateException("Nova closed a TOOL block without a toolUse event.")
            pendingTool = null
            toolInProgress = true
            val contentName = "hyusk-tool-${UUID.randomUUID()}"
            val worker = toolScope
            if (worker == null) {
                sendToolResult(tool.id, contentName, "Phone tool executor is unavailable.")
                toolInProgress = false
                lastActivity = System.currentTimeMillis()
            } else worker.launch {
                val result = try {
                    when {
                        tool.validationError != null -> "Phone tool request rejected: ${tool.validationError}"
                        tool.name != PHONE_DELEGATION_TOOL -> "Unsupported phone delegation tool '${tool.name}'."
                        else -> onPhoneTask(tool.request!!)
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    "Phone task failed: ${failure.message ?: failure.javaClass.simpleName}"
                }
                try {
                    sendToolResult(tool.id, contentName, result)
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    // The live session may have been stopped while the task ran.
                } finally {
                    toolInProgress = false
                    lastActivity = System.currentTimeMillis()
                }
            }
        }
        val stopReason = event.optJSONObject("contentEnd")?.optString("stopReason")
        if (stopReason == "INTERRUPTED" || event.has("interruptionEvent")) {
            clearPlaybackQueue()
        }
        if (event.has("completionEnd")) {
            lastActivity = System.currentTimeMillis()
            turnInProgress = false
            finishPlaybackTurn()
        }
    }

    private suspend fun sendToolResult(toolId: String, contentName: String, result: String) {
        send(JSONObject().put("contentStart", JSONObject().put("promptName", promptName)
            .put("contentName", contentName).put("interactive", false).put("type", "TOOL").put("role", "TOOL")
            .put("toolResultInputConfiguration", JSONObject().put("toolUseId", toolId)
                .put("type", "TEXT").put("textInputConfiguration", JSONObject().put("mediaType", "text/plain")))))
        send(JSONObject().put("toolResult", content(contentName, JSONObject().put("result", result).toString())))
        send(JSONObject().put("contentEnd", end(contentName)))
    }

    private fun clearPlaybackQueue() {
        playbackVersion++
        playbackDrainJob?.cancel()
        audioOutput?.let { queue -> while (queue.tryReceive().isSuccess) Unit }
        synchronized(playbackLock) {
            runCatching { playback?.pause(); playback?.flush() }
            playbackFramesWritten = 0L
            playback?.let { playbackHeadOrigin = playbackHeadPosition(it) }
        }
        mutableState.value = mutableState.value.copy(speaking = false)
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private fun finishPlaybackTurn() {
        val worker = toolScope ?: return
        val version = playbackVersion
        playbackDrainJob?.cancel()
        playbackDrainJob = worker.launch(Dispatchers.IO) {
            while (isActive && playbackVersion == version) {
                val output = audioOutput
                if ((output == null || output.isEmpty) && !playbackBusy) {
                    val track = playback ?: break
                    val targetFrames = playbackFramesWritten
                    while (isActive && playbackVersion == version && track === playback &&
                        playbackHeadFrames(track) < targetFrames) {
                        delay(20)
                    }
                    if (isActive && playbackVersion == version && track === playback &&
                        playbackFramesWritten == targetFrames) {
                        withContext(Dispatchers.Main.immediate) {
                            mutableState.value = mutableState.value.copy(speaking = false)
                        }
                    }
                    break
                }
                delay(20)
            }
        }
    }

    private fun playbackHeadPosition(track: AudioTrack): Long = track.playbackHeadPosition.toLong() and 0xffff_ffffL

    private fun playbackHeadFrames(track: AudioTrack): Long =
        (playbackHeadPosition(track) - playbackHeadOrigin) and 0xffff_ffffL

    private fun play(pcm: ByteArray, currentGeneration: Long) = synchronized(playbackLock) {
        if (generation != currentGeneration) return@synchronized
        val track = playback ?: AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build())
            .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(24000).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setBufferSizeInBytes(24000).setTransferMode(AudioTrack.MODE_STREAM).build()
            .also {
                if (it.state != AudioTrack.STATE_INITIALIZED) {
                    it.release()
                    throw IllegalStateException("Speaker could not initialize")
                }
                playbackFramesWritten = 0L
                playbackHeadOrigin = playbackHeadPosition(it)
                playback = it
            }
        if (track.playState != AudioTrack.PLAYSTATE_PLAYING) track.play()
        mutableState.value = mutableState.value.copy(speaking = true)
        var offset = 0
        while (offset < pcm.size) {
            if (generation != currentGeneration) break
            val written = track.write(pcm, offset, pcm.size - offset, AudioTrack.WRITE_BLOCKING)
            if (written <= 0) {
                val route = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    track.routedDevice?.let { "${it.type}:${it.id}" } ?: "none"
                } else "unknown"
                Log.e(TAG, "[$instanceId/$runId] AudioTrack.write returned $written offset=$offset pcmBytes=${pcm.size} " +
                    "trackState=${track.state} playState=${track.playState} session=${track.audioSessionId} " +
                    "rate=${track.sampleRate} bufferFrames=${track.bufferSizeInFrames} head=${track.playbackHeadPosition} " +
                    "route=$route api=${Build.VERSION.SDK_INT} generation=$generation expectedGeneration=$currentGeneration")
                throw IllegalStateException("Speaker playback failed ($written, track=${track.state}/${track.playState}, route=$route)")
            }
            offset += written
            playbackFramesWritten += written / 2L
        }
    }

    private fun releasePlayback() = synchronized(playbackLock) {
        playback?.let { track ->
            runCatching { track.stop() }
            runCatching { track.release() }
        }
        playback = null
        playbackFramesWritten = 0L
        playbackHeadOrigin = 0L
    }

    private suspend fun send(value: JSONObject) {
        val eventName = value.keys().asSequence().firstOrNull() ?: "unknown"
        if (eventName != "audioInput" || microphoneFrames == 0L || microphoneFrames % 32L == 0L) {
            Log.d(TAG, "[$instanceId/$runId] enqueue event=$eventName")
        }
        input?.send(eventChunk(value))
    }

    private fun eventChunk(value: JSONObject): InvokeModelWithBidirectionalStreamInput =
        InvokeModelWithBidirectionalStreamInput.Chunk(BidirectionalInputPayloadPart {
            bytes = JSONObject().put("event", value).toString().toByteArray(Charsets.UTF_8)
        })

    private fun textStart(name: String, role: String, interactive: Boolean) = JSONObject()
        .put("promptName", promptName).put("contentName", name).put("type", "TEXT")
        .put("interactive", interactive).put("role", role)
        .put("textInputConfiguration", JSONObject().put("mediaType", "text/plain"))

    private fun content(name: String, value: String) = JSONObject()
        .put("promptName", promptName).put("contentName", name).put("content", value)

    private fun end(name: String) = JSONObject().put("promptName", promptName).put("contentName", name)

    fun submitText(text: String) {
        if (text.isBlank() || input == null) return
        if (isVoiceStopCommand(text)) { stop(); return }
        scope.launch {
            val name = "hyusk-text-${UUID.randomUUID()}"
            send(JSONObject().put("contentStart", textStart(name, "USER", true)))
            send(JSONObject().put("textInput", content(name, text.trim())))
            send(JSONObject().put("contentEnd", end(name)))
            lastActivity = System.currentTimeMillis()
            turnInProgress = true
        }
    }

    fun stop() {
        val stopGeneration = ++generation
        Log.i(TAG, "[$instanceId/$runId] stop generation=$stopGeneration micFrames=$microphoneFrames responses=$responseEvents")
        turnInProgress = false
        stopTask = null
        runCatching { recorder?.stop() }
        recorder = null
        val channel = input
        val stoppingPrompt = promptName
        val microphone = recorderJob
        synchronized(playbackLock) { runCatching { playback?.pause(); playback?.flush() } }
        playbackVersion++
        playbackDrainJob?.cancel()
        synchronized(playbackLock) {
            playbackFramesWritten = 0L
            playback?.let { playbackHeadOrigin = playbackHeadPosition(it) }
        }
        audioOutput?.close()
        audioOutput = null
        val stoppingSession = session
        session = null
        mutableState.value = mutableState.value.copy(listening = false, speaking = false)
        if (stoppingSession == null) {
            releasePlayback()
            return
        }
        scope.launch {
            // Stop the audio producer first. Its contentEnd must be sent before
            // sessionEnd; racing these events makes Bedrock reset the HTTP/2
            // request body while the SDK is still writing audio (which can crash
            // the process with an uncaught StreamResetException).
            val microphoneStopped = withTimeoutOrNull(STREAM_MIC_STOP_MS) {
                microphone?.join()
                true
            } == true
            if (!microphoneStopped) {
                microphone?.cancel()
                // A stalled microphone must not block input closure forever.
                withTimeoutOrNull(STREAM_END_SEND_MS) { microphone?.join() }
            }
            withTimeoutOrNull(STREAM_END_SEND_MS) {
                if (audioStartedGeneration == stopGeneration - 1L) {
                    channel?.send(eventChunk(JSONObject().put("contentEnd", JSONObject()
                        .put("promptName", stoppingPrompt).put("contentName", "hyusk-audio"))))
                }
                if (promptStartedGeneration == stopGeneration - 1L) {
                    channel?.send(eventChunk(JSONObject().put("promptEnd", JSONObject().put("promptName", stoppingPrompt))))
                }
                channel?.send(eventChunk(JSONObject().put("sessionEnd", JSONObject())))
            }
            channel?.close()
            if (input === channel) input = null
            val completed = withTimeoutOrNull(STREAM_STOP_GRACE_MS) {
                stoppingSession.join()
                true
            } == true
            if (!completed) stoppingSession.cancel()
            stoppingSession.join()
            if (generation == stopGeneration && session == null) {
                playbackDrainJob?.cancel()
                releasePlayback()
            }
        }
    }

    override fun close() {
        stop()
        // Let the stream's graceful end and recorder cleanup finish before
        // destroying the scope; immediate cancellation reintroduces the race.
        scope.launch {
            delay(STREAM_END_SEND_MS + STREAM_STOP_GRACE_MS + 100L)
            scope.coroutineContext[Job]?.cancel()
        }
    }

    private companion object {
        const val TAG = "HyuskNovaVoice"
        const val SPEAKER_INTERRUPTED_MESSAGE = "Speaker output was interrupted. Hyusk is still listening."
        const val PHONE_DELEGATION_TOOL = "delegate_phone_agent"
        const val STREAM_END_SEND_MS = 600L
        const val STREAM_MIC_STOP_MS = 2_000L
        const val STREAM_STOP_GRACE_MS = 8_000L
    }
}
