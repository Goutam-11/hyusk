package io.github.hyusk.mobile.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.util.Base64
import androidx.core.content.ContextCompat
import aws.sdk.kotlin.runtime.auth.credentials.StaticCredentialsProvider
import aws.sdk.kotlin.services.bedrockruntime.BedrockRuntimeClient
import aws.sdk.kotlin.services.bedrockruntime.model.BidirectionalInputPayloadPart
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamInput
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamRequest
import aws.sdk.kotlin.services.bedrockruntime.model.InvokeModelWithBidirectionalStreamOutput
import aws.smithy.kotlin.runtime.auth.awscredentials.Credentials
import io.github.hyusk.mobile.security.NovaSonicSettings
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ClosedSendChannelException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.consumeAsFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/** One live, signed Bedrock stream. The microphone stays open across turns. */
class NovaSonicVoiceSession(private val context: Context) : AutoCloseable {
    private val scope = CoroutineScope(Dispatchers.Main.immediate + SupervisorJob())
    private val mutableState = MutableStateFlow(VoiceState())
    val state: StateFlow<VoiceState> = mutableState
    private var session: Job? = null
    private var input: Channel<InvokeModelWithBidirectionalStreamInput>? = null
    private var audioOutput: Channel<ByteArray>? = null
    private var playback: AudioTrack? = null
    private var playbackDrainJob: Job? = null
    @Volatile private var playbackBusy = false
    @Volatile private var playbackFramesWritten = 0L
    @Volatile private var playbackVersion = 0L
    @Volatile private var playbackHeadOrigin = 0L
    @Volatile private var recorder: AudioRecord? = null
    private var generation = 0L
    private var lastActivity = 0L
    private var outputRole = ""
    private var outputStage = ""
    private var outputType = ""
    private var pendingTool: JSONObject? = null
    private var toolScope: CoroutineScope? = null
    private var promptName = ""
    private var suppressAudio = false
    @Volatile private var toolInProgress = false
    @Volatile private var turnInProgress = false
    private var stopTask: (() -> Unit)? = null

    fun start(settings: NovaSonicSettings, onPhoneTask: suspend (String) -> String, onStopTask: () -> Unit = {}) {
        val previousSession = session
        stop()
        val currentGeneration = generation
        stopTask = onStopTask
        suppressAudio = false
        turnInProgress = false
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            mutableState.value = VoiceState(error = "Microphone permission is required for Nova voice.")
            return
        }
        if (settings.accessKeyId.isBlank() || settings.secretAccessKey.isBlank()) {
            mutableState.value = VoiceState(error = "Add your AWS IAM key in Settings to use Nova voice.")
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
                    playback?.let { runCatching { it.stop() }; it.release() }
                    playback = null
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
            val toolSpec = JSONObject().put("name", "phone_task")
                .put("description", "Perform a multi-step task on this Android phone with native apps and accessibility. Await verified completion; never claim success before this tool returns.")
                .put("inputSchema", JSONObject().put("json", "{\"type\":\"object\",\"properties\":{\"request\":{\"type\":\"string\"}},\"required\":[\"request\"]}"))
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
            send(JSONObject().put("contentStart", textStart(systemName, "SYSTEM", false)))
            send(JSONObject().put("textInput", content(systemName,
                "You are Hyusk, Goutam Sharma’s concise voice assistant. For any request to inspect or operate the phone—including opening apps, finding contacts, or messaging—call phone_task immediately with the complete requested task; do not merely describe steps. Wait for its verified result, then report what actually happened or the specific blocker. Never claim success from a dispatched action alone. Answer ordinary conversation naturally. If the user says go away, stop the task and end the voice session.")))
            send(JSONObject().put("contentEnd", end(systemName)))

            val workerScope = CoroutineScope(currentCoroutineContext())
            toolScope = workerScope
            val recorderJob = workerScope.launch(Dispatchers.IO) { recordMicrophone() }
            val outputChannel = Channel<ByteArray>(24)
            audioOutput = outputChannel
            val playbackJob = workerScope.launch(Dispatchers.IO) {
                for (pcm in outputChannel) {
                    if (generation != currentGeneration) break
                    playbackBusy = true
                    try {
                        play(pcm)
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
                client.invokeModelWithBidirectionalStream(InvokeModelWithBidirectionalStreamRequest {
                    modelId = settings.modelId
                    body = channel.consumeAsFlow()
                }) { response ->
                    response.body?.collect { chunk ->
                        val bytes = (chunk as? InvokeModelWithBidirectionalStreamOutput.Chunk)?.value?.bytes ?: return@collect
                        handleEvent(JSONObject(bytes.toString(Charsets.UTF_8)).optJSONObject("event") ?: return@collect, onPhoneTask)
                    }
                }
                throw IllegalStateException("Nova closed the live stream. Tap the microphone to start a new conversation.")
            } finally {
                recorderJob.cancelAndJoin()
                idleJob.cancel()
                outputChannel.close()
                playbackJob.cancelAndJoin()
                toolScope = null
            }
        } finally {
            client.close()
        }
    }

    private suspend fun recordMicrophone() {
        val minBuffer = AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        require(minBuffer > 0) { "Microphone does not support 16 kHz PCM" }
        val recorder = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, 16000,
            AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(minBuffer, 4096))
        require(recorder.state == AudioRecord.STATE_INITIALIZED) { "Could not open microphone" }
        this.recorder = recorder
        val echoCanceller = if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(recorder.audioSessionId) else null
        echoCanceller?.enabled = true
        val audioName = "hyusk-audio"
        try {
            send(JSONObject().put("contentStart", JSONObject().put("promptName", promptName)
                .put("contentName", audioName).put("type", "AUDIO").put("interactive", true).put("role", "USER")
                .put("audioInputConfiguration", JSONObject().put("mediaType", "audio/lpcm")
                    .put("sampleRateHertz", 16000).put("sampleSizeBits", 16).put("channelCount", 1)
                    .put("audioType", "SPEECH").put("encoding", "base64"))))
            recorder.startRecording()
            val frame = ByteArray(1024)
            var speechFrames = 0
            while (currentCoroutineContext().isActive && input != null) {
                val count = recorder.read(frame, 0, frame.size)
                if (count < 0) {
                    // stop() interrupts AudioRecord.read() with ERROR_INVALID_OPERATION
                    // on some devices. Do not spin/log until the stream coroutine exits.
                    if (!currentCoroutineContext().isActive || recorder.recordingState != AudioRecord.RECORDSTATE_RECORDING) break
                    throw IllegalStateException("Microphone read failed ($count)")
                }
                if (count == 0) continue
                val pcm = if (count == frame.size) frame else frame.copyOf(count)
                // The local RMS barge-in detector can mistake the speaker for the
                // user without acoustic echo cancellation. In that case let Nova's
                // own turn detector handle interruption instead of flushing audio.
                if (mutableState.value.speaking && echoCanceller?.enabled == true) {
                    var power = 0.0
                    for (i in 0 until count - 1 step 2) {
                        val sample = ((pcm[i + 1].toInt() shl 8) or (pcm[i].toInt() and 255)).toShort().toDouble() / 32768.0
                        power += sample * sample
                    }
                    val rms = kotlin.math.sqrt(power / (count / 2).coerceAtLeast(1))
                    speechFrames = if (rms > 0.09) speechFrames + 1 else 0
                    if (speechFrames >= 4) {
                        suppressAudio = true
                        clearPlaybackQueue()
                        speechFrames = 0
                    }
                }
                send(JSONObject().put("audioInput", content(audioName, Base64.encodeToString(pcm, Base64.NO_WRAP))))
            }
        } finally {
            runCatching { recorder.stop() }
            recorder.release()
            if (this.recorder === recorder) this.recorder = null
            echoCanceller?.release()
            runCatching { send(JSONObject().put("contentEnd", end(audioName))) }
        }
    }

    private suspend fun handleEvent(event: JSONObject, onPhoneTask: suspend (String) -> String) {
        if (event.has("completionStart")) {
            suppressAudio = false
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
            if (!suppressAudio) {
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
        }
        event.optJSONObject("toolUse")?.let { pendingTool = it }
        if (event.optJSONObject("contentEnd")?.optString("type") == "TOOL") {
            val tool = pendingTool
            pendingTool = null
            if (tool != null) {
                val arguments = runCatching { JSONObject(tool.optString("content")) }.getOrDefault(JSONObject())
                val request = arguments.optString("request").trim()
                toolInProgress = true
                val toolId = tool.optString("toolUseId")
                val contentName = "hyusk-tool-${UUID.randomUUID()}"
                val worker = toolScope
                if (worker == null) {
                    toolInProgress = false
                } else worker.launch {
                    val result = try {
                        if (tool.optString("toolName") == "phone_task" && request.isNotBlank()) onPhoneTask(request)
                        else "Unsupported phone tool request"
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (failure: Exception) {
                        "Phone task failed: ${failure.message ?: failure.javaClass.simpleName}"
                    } finally {
                        toolInProgress = false
                        lastActivity = System.currentTimeMillis()
                    }
                    try {
                        send(JSONObject().put("contentStart", JSONObject().put("promptName", promptName)
                            .put("contentName", contentName).put("interactive", false).put("type", "TOOL").put("role", "TOOL")
                            .put("toolResultInputConfiguration", JSONObject().put("toolUseId", toolId)
                                .put("type", "TEXT").put("textInputConfiguration", JSONObject().put("mediaType", "text/plain")))))
                        send(JSONObject().put("toolResult", content(contentName, JSONObject().put("result", result).toString())))
                        send(JSONObject().put("contentEnd", end(contentName)))
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (_: Exception) {
                        // The live session may have been stopped while the task ran.
                    }
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

    private fun clearPlaybackQueue() {
        playbackVersion++
        playbackDrainJob?.cancel()
        audioOutput?.let { queue -> while (queue.tryReceive().isSuccess) Unit }
        runCatching { playback?.pause(); playback?.flush() }
        playbackFramesWritten = 0L
        playback?.let { playbackHeadOrigin = playbackHeadPosition(it) }
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

    private fun play(pcm: ByteArray) {
        val track = playback ?: AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build())
            .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(24000).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setBufferSizeInBytes(24000).setTransferMode(AudioTrack.MODE_STREAM).build()
            .also {
                playbackFramesWritten = 0L
                playbackHeadOrigin = playbackHeadPosition(it)
                playback = it
            }
        if (track.playState != AudioTrack.PLAYSTATE_PLAYING) track.play()
        mutableState.value = mutableState.value.copy(speaking = true)
        var offset = 0
        while (offset < pcm.size) {
            val written = track.write(pcm, offset, pcm.size - offset, AudioTrack.WRITE_BLOCKING)
            if (written <= 0) throw IllegalStateException("Speaker playback failed ($written)")
            offset += written
            playbackFramesWritten += written / 2L
        }
    }

    private suspend fun send(value: JSONObject) {
        val bytes = JSONObject().put("event", value).toString().toByteArray(Charsets.UTF_8)
        input?.send(InvokeModelWithBidirectionalStreamInput.Chunk(BidirectionalInputPayloadPart { this.bytes = bytes }))
    }

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
        turnInProgress = false
        stopTask = null
        runCatching { recorder?.stop() }
        recorder = null
        val channel = input
        input = null
        runCatching { playback?.pause(); playback?.flush() }
        playbackVersion++
        playbackDrainJob?.cancel()
        playbackFramesWritten = 0L
        playback?.let { playbackHeadOrigin = playbackHeadPosition(it) }
        audioOutput?.close()
        audioOutput = null
        val stoppingSession = session
        session = null
        mutableState.value = mutableState.value.copy(listening = false, speaking = false)
        if (stoppingSession == null) {
            playback?.let { runCatching { it.stop() }; it.release() }
            playback = null
            return
        }
        scope.launch {
            // Finish enqueueing the terminal event before closing the request
            // body. A non-blocking trySend can silently lose it when the audio
            // channel is full, leaving the service to cancel an open session.
            val terminal = InvokeModelWithBidirectionalStreamInput.Chunk(
                BidirectionalInputPayloadPart {
                    bytes = JSONObject().put("event", JSONObject().put("sessionEnd", JSONObject()))
                        .toString().toByteArray(Charsets.UTF_8)
                },
            )
            withTimeoutOrNull(STREAM_END_SEND_MS) {
                channel?.send(terminal)
            }
            channel?.close()
            val completed = withTimeoutOrNull(STREAM_STOP_GRACE_MS) {
                stoppingSession.join()
                true
            } == true
            if (!completed) stoppingSession.cancel()
            stoppingSession.join()
            if (generation == stopGeneration && session == null) {
                playbackDrainJob?.cancel()
                playback?.let { runCatching { it.stop() }; it.release() }
                playback = null
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
        const val STREAM_END_SEND_MS = 600L
        const val STREAM_STOP_GRACE_MS = 2_500L
    }
}
