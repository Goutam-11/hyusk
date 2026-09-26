package io.github.hyusk.mobile.voice

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

data class VoiceState(
    val listening: Boolean = false,
    val speaking: Boolean = false,
    val transcript: String = "",
    val error: String? = null,
    /** Increments only when Android delivers a final recognition result. */
    val finalResultId: Long = 0,
)

fun isVoiceStopCommand(text: String): Boolean = when (text.lowercase()
    .replace(Regex("[^\\p{L}\\p{N}]+"), " ").trim().replace(Regex("\\s+"), " ")) {
    "go away", "go away hyusk", "hyusk go away", "stop listening" -> true
    else -> false
}

class VoiceController(context: Context) : RecognitionListener, AutoCloseable {
    private val recognizer = SpeechRecognizer.createSpeechRecognizer(context)
    private var tts: TextToSpeech? = null
    private val _state = MutableStateFlow(VoiceState())
    val state: StateFlow<VoiceState> = _state

    init {
        recognizer.setRecognitionListener(this)
        tts = TextToSpeech(context.applicationContext) { status ->
            if (status == TextToSpeech.SUCCESS) {
                val engine = tts ?: return@TextToSpeech
                HyuskSpeech.configure(engine)
                engine.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                    override fun onStart(utteranceId: String?) {
                        _state.value = _state.value.copy(speaking = true, error = null)
                    }

                    override fun onDone(utteranceId: String?) {
                        _state.value = _state.value.copy(speaking = false)
                    }

                    @Deprecated("Deprecated in Java")
                    override fun onError(utteranceId: String?) {
                        _state.value = _state.value.copy(speaking = false)
                    }
                })
            }
        }
    }

    fun start() {
        tts?.stop()
        _state.value = _state.value.copy(listening = true, speaking = false, transcript = "", error = null)
        recognizer.startListening(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, "en-IN")
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, false)
        })
    }

    fun stop() { recognizer.stopListening() }

    fun stopAll() {
        recognizer.cancel()
        tts?.stop()
        _state.value = _state.value.copy(listening = false, speaking = false)
    }

    fun speak(text: String) {
        val spoken = HyuskSpeech.normalize(text)
        if (spoken.isNotBlank()) tts?.speak(spoken, TextToSpeech.QUEUE_FLUSH, null, "hyusk-response")
    }
    override fun close() { stopAll(); recognizer.destroy(); tts?.shutdown(); tts = null }
    override fun onReadyForSpeech(params: Bundle?) = Unit
    override fun onBeginningOfSpeech() = Unit
    override fun onRmsChanged(rmsdB: Float) = Unit
    override fun onBufferReceived(buffer: ByteArray?) = Unit
    // Android calls this before onResults. Keep the session logically active so a
    // partial transcript cannot be mistaken for a final command by the UI.
    override fun onEndOfSpeech() = Unit
    override fun onError(error: Int) { _state.value = _state.value.copy(listening = false, error = errorText(error)) }
    override fun onResults(results: Bundle?) = update(results, final = true)
    override fun onPartialResults(partialResults: Bundle?) = update(partialResults, final = false)
    override fun onEvent(eventType: Int, params: Bundle?) = Unit

    private fun update(results: Bundle?, final: Boolean) {
        val text = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty()
        val previous = _state.value
        _state.value = previous.copy(
            listening = !final,
            transcript = text,
            error = null,
            finalResultId = if (final && text.isNotBlank()) previous.finalResultId + 1 else previous.finalResultId,
        )
    }

    private fun errorText(code: Int) = when (code) {
        SpeechRecognizer.ERROR_NO_MATCH -> "I couldn't understand that. Try again."
        SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "I didn't hear anything."
        SpeechRecognizer.ERROR_NETWORK, SpeechRecognizer.ERROR_NETWORK_TIMEOUT -> "Speech recognition is offline."
        SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "Microphone permission is required."
        else -> "Speech recognition stopped ($code)."
    }
}
