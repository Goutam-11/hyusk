package io.github.hyusk.mobile.services

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognitionService
import android.speech.SpeechRecognizer

/**
 * Android requires every VoiceInteractionService to publish a companion
 * RecognitionService. Hyusk delegates recognition to the phone's existing
 * system engine so selecting Hyusk does not replace its speech model.
 */
class HyuskRecognitionService : RecognitionService() {
    private val mainHandler = Handler(Looper.getMainLooper())
    private var delegate: SpeechRecognizer? = null

    override fun onStartListening(intent: Intent, callback: Callback) {
        mainHandler.post {
            releaseDelegate()
            val component = findSystemRecognizer(this)
            if (component == null) {
                callback.error(SpeechRecognizer.ERROR_CLIENT)
                return@post
            }

            delegate = SpeechRecognizer.createSpeechRecognizer(this, component).also { recognizer ->
                recognizer.setRecognitionListener(ForwardingListener(callback))
                runCatching { recognizer.startListening(intent) }
                    .onFailure {
                        callback.error(SpeechRecognizer.ERROR_CLIENT)
                        releaseDelegate()
                    }
            }
        }
    }

    override fun onStopListening(callback: Callback) {
        mainHandler.post { delegate?.stopListening() }
    }

    override fun onCancel(callback: Callback) {
        mainHandler.post {
            delegate?.cancel()
            releaseDelegate()
        }
    }

    override fun onDestroy() {
        releaseDelegate()
        super.onDestroy()
    }

    private fun releaseDelegate() {
        delegate?.destroy()
        delegate = null
    }

    private inner class ForwardingListener(private val callback: Callback) : RecognitionListener {
        override fun onReadyForSpeech(params: Bundle?) = callback.readyForSpeech(params ?: Bundle())
        override fun onBeginningOfSpeech() = callback.beginningOfSpeech()
        override fun onRmsChanged(rmsdB: Float) = callback.rmsChanged(rmsdB)
        override fun onBufferReceived(buffer: ByteArray?) = callback.bufferReceived(buffer ?: byteArrayOf())
        override fun onEndOfSpeech() = callback.endOfSpeech()
        override fun onError(error: Int) {
            callback.error(error)
            releaseDelegate()
        }
        override fun onResults(results: Bundle?) {
            callback.results(results ?: Bundle())
            releaseDelegate()
        }
        override fun onPartialResults(partialResults: Bundle?) = callback.partialResults(partialResults ?: Bundle())
        override fun onEvent(eventType: Int, params: Bundle?) = Unit
    }

    companion object {
        fun findSystemRecognizer(context: Context): ComponentName? {
            val services = context.packageManager.queryIntentServices(
                Intent(RecognitionService.SERVICE_INTERFACE),
                PackageManager.MATCH_DIRECT_BOOT_AWARE or PackageManager.MATCH_DIRECT_BOOT_UNAWARE,
            ).filterNot { it.serviceInfo.packageName == context.packageName }

            val preferred = services.firstOrNull {
                it.serviceInfo.packageName == "com.google.android.googlequicksearchbox"
            } ?: services.firstOrNull {
                it.serviceInfo.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_SYSTEM != 0
            } ?: services.firstOrNull()

            return preferred?.serviceInfo?.let { ComponentName(it.packageName, it.name) }
        }
    }
}
