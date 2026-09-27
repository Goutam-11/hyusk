package io.github.hyusk.mobile.services

import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.graphics.Color
import android.graphics.Canvas
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.GradientDrawable
import android.text.TextUtils
import android.view.Gravity
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import io.github.hyusk.mobile.automation.PhoneAgentTaskManager
import io.github.hyusk.mobile.automation.PhoneTaskPhase
import io.github.hyusk.mobile.voice.HyuskSpeech
import io.github.hyusk.mobile.voice.NovaSonicVoiceSession
import io.github.hyusk.mobile.security.NovaSonicSettingsStore
import io.github.hyusk.mobile.voice.isVoiceStopCommand
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

class HyuskVoiceInteractionService : VoiceInteractionService()

class HyuskVoiceInteractionSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = HyuskVoiceInteractionSession(this)
}

class HyuskVoiceInteractionSession(private val sessionService: VoiceInteractionSessionService) : VoiceInteractionSession(sessionService) {
    private data class SpeechRequest(
        val text: String,
        val append: Boolean,
        val completesTurn: Boolean,
        val listenAfter: Boolean,
    )

    private val mainHandler = Handler(Looper.getMainLooper())
    private var tts: TextToSpeech? = null
    private var ttsReady = false
    private var pendingSpeech: SpeechRequest? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val taskManager = PhoneAgentTaskManager.get(sessionService.applicationContext)
    private val nova = NovaSonicVoiceSession(sessionService.applicationContext)
    private val novaSettingsStore = NovaSonicSettingsStore(sessionService.applicationContext)
    private var novaActive = false
    private var recognizer: android.speech.SpeechRecognizer? = null
    private var statusView: TextView? = null
    private var inputView: EditText? = null
    private var orbView: AssistantButterflyView? = null
    private var sessionToken = 0L
    private var lastCommand = ""
    private var lastCommandAt = 0L
    private var visible = false
    private var lastDeliveredRevision = 0L
    private var finalUtteranceId: String? = null
    private var utteranceCounter = 0L
    private var listenAfterFinalSpeech = false

    init {
        tts = TextToSpeech(sessionService) { status ->
            val engine = tts ?: return@TextToSpeech
            if (status == TextToSpeech.SUCCESS) {
                HyuskSpeech.configure(engine)
                engine.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                    override fun onStart(utteranceId: String?) {
                        mainHandler.post {
                            setVisualState(AssistantVisualState.SPEAKING, statusView?.text?.toString())
                        }
                    }

                    override fun onDone(utteranceId: String?) {
                        mainHandler.post {
                            if (utteranceId != finalUtteranceId) return@post
                            finalUtteranceId = null
                            if (listenAfterFinalSpeech) {
                                listenAfterFinalSpeech = false
                                beginListening()
                                return@post
                            }
                            setVisualState(AssistantVisualState.IDLE, "Done")
                            val token = sessionToken
                            mainHandler.postDelayed({ if (token == sessionToken) hide() }, 900)
                        }
                    }

                    @Deprecated("Deprecated in Java")
                    override fun onError(utteranceId: String?) {
                        mainHandler.post {
                            setVisualState(AssistantVisualState.ERROR, "Voice output unavailable")
                        }
                    }
                })
                ttsReady = true
                pendingSpeech?.also {
                    pendingSpeech = null
                    speak(it.text, it.append, it.completesTurn, it.listenAfter)
                }
            }
        }
    }

    @Suppress("DEPRECATION")
    override fun onCreate() {
        super.onCreate()
        scope.launch {
            taskManager.state.collect { task ->
                if (!visible || task.revision <= lastDeliveredRevision) return@collect
                lastDeliveredRevision = task.revision
                if (novaActive) {
                    if (task.phase == PhoneTaskPhase.Working) setVisualState(AssistantVisualState.THINKING, task.message)
                    return@collect
                }
                when (task.phase) {
                    PhoneTaskPhase.Working -> {
                        setVisualState(AssistantVisualState.THINKING, task.message)
                        if (task.speakProgress) speakProgress(task.message)
                    }
                    PhoneTaskPhase.Idle -> Unit
                    else -> {
                        setVisualState(AssistantVisualState.SPEAKING, task.message)
                        speak(task.message, append = true, completesTurn = true, listenAfter = task.needsReply)
                    }
                }
            }
        }
        scope.launch {
            nova.state.collect { voice ->
                if (!visible || !novaActive) return@collect
                when {
                    voice.error != null -> setVisualState(AssistantVisualState.ERROR, voice.error)
                    voice.speaking -> setVisualState(AssistantVisualState.SPEAKING, voice.transcript)
                    voice.listening -> setVisualState(AssistantVisualState.LISTENING, voice.transcript.ifBlank { "Listening…" })
                    else -> setVisualState(AssistantVisualState.IDLE, "Nova conversation ended")
                }
            }
        }
        window.window?.apply {
            setBackgroundDrawable(ColorDrawable(Color.TRANSPARENT))
            addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
            setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
            attributes = attributes.apply { dimAmount = 0.18f }
        }
    }

    override fun onCreateContentView(): View {
        val density = sessionService.resources.displayMetrics.density
        return FrameLayout(sessionService).apply {
            setBackgroundColor(Color.TRANSPARENT)
            importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_YES
            contentDescription = "Hyusk voice assistant"

            val panel = LinearLayout(sessionService).apply {
                orientation = LinearLayout.VERTICAL
                gravity = Gravity.CENTER_HORIZONTAL
                elevation = 22 * density
                setPadding(
                    (20 * density).toInt(),
                    (18 * density).toInt(),
                    (20 * density).toInt(),
                    (18 * density).toInt(),
                )
                background = GradientDrawable().apply {
                    setColor(Color.argb(247, 5, 6, 7))
                    cornerRadius = 32 * density
                    setStroke((1 * density).toInt().coerceAtLeast(1), Color.rgb(42, 45, 51))
                }

                addView(LinearLayout(sessionService).apply {
                    orientation = LinearLayout.HORIZONTAL
                    gravity = Gravity.CENTER_VERTICAL
                    addView(View(sessionService).apply {
                        background = GradientDrawable().apply {
                            shape = GradientDrawable.OVAL
                            setColor(Color.rgb(111, 231, 212))
                        }
                    }, LinearLayout.LayoutParams((8 * density).toInt(), (8 * density).toInt()).apply {
                        marginEnd = (10 * density).toInt()
                    })
                    addView(TextView(sessionService).apply {
                        text = "HYUSK"
                        setTextColor(Color.rgb(244, 242, 237))
                        textSize = 12f
                        letterSpacing = .28f
                        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
                    })
                    addView(TextView(sessionService).apply {
                        text = "VOICE"
                        setTextColor(Color.rgb(142, 146, 153))
                        textSize = 11f
                        letterSpacing = .12f
                        gravity = Gravity.END
                    }, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
                }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT))

                addView(AssistantButterflyView(sessionService).apply {
                    orbView = this
                    contentDescription = "Hyusk is listening"
                }, LinearLayout.LayoutParams((156 * density).toInt(), (156 * density).toInt()).apply {
                    topMargin = (2 * density).toInt()
                })

                addView(TextView(sessionService).apply {
                    statusView = this
                    text = "Listening…"
                    setTextColor(Color.WHITE)
                    textSize = 17f
                    gravity = Gravity.CENTER
                    maxLines = 4
                    ellipsize = TextUtils.TruncateAt.END
                    setPadding((6 * density).toInt(), 0, (6 * density).toInt(), 0)
                    typeface = Typeface.create("sans-serif", Typeface.NORMAL)
                }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
                    bottomMargin = (16 * density).toInt()
                })

                addView(LinearLayout(sessionService).apply {
                    orientation = LinearLayout.HORIZONTAL
                    gravity = Gravity.CENTER_VERTICAL
                    setPadding((5 * density).toInt(), 0, (5 * density).toInt(), 0)
                    background = GradientDrawable().apply {
                        setColor(Color.rgb(17, 19, 23))
                        cornerRadius = 24 * density
                        setStroke((1 * density).toInt().coerceAtLeast(1), Color.rgb(42, 45, 51))
                    }

                    addView(EditText(sessionService).apply {
                        inputView = this
                        hint = "Ask Hyusk…"
                        setHintTextColor(Color.rgb(142, 146, 153))
                        setTextColor(Color.rgb(244, 242, 237))
                        textSize = 15f
                        setSingleLine(true)
                        imeOptions = EditorInfo.IME_ACTION_SEND
                        background = ColorDrawable(Color.TRANSPARENT)
                        setPadding((12 * density).toInt(), 0, (8 * density).toInt(), 0)
                        setOnEditorActionListener { _, actionId, _ ->
                            if (actionId == EditorInfo.IME_ACTION_SEND) {
                                submitTypedCommand()
                                true
                            } else false
                        }
                    }, LinearLayout.LayoutParams(0, (50 * density).toInt(), 1f))

                    addView(TextView(sessionService).apply {
                        text = "↑"
                        textSize = 24f
                        gravity = Gravity.CENTER
                        setTextColor(Color.rgb(5, 6, 7))
                        contentDescription = "Send command"
                        isClickable = true
                        isFocusable = true
                        background = GradientDrawable().apply {
                            shape = GradientDrawable.OVAL
                            setColor(Color.rgb(244, 242, 237))
                        }
                        setOnClickListener { submitTypedCommand() }
                    }, LinearLayout.LayoutParams((42 * density).toInt(), (42 * density).toInt()))
                }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, (52 * density).toInt()))
            }
            addView(panel, FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM or Gravity.CENTER_HORIZONTAL,
            ).apply {
                leftMargin = (14 * density).toInt()
                rightMargin = (14 * density).toInt()
                bottomMargin = (18 * density).toInt()
            })
        }
    }

    private fun submitTypedCommand() {
        val command = inputView?.text?.toString()?.trim().orEmpty()
        if (command.isBlank()) return
        inputView?.text?.clear()
        inputView?.clearFocus()
        (sessionService.getSystemService(InputMethodManager::class.java))
            ?.hideSoftInputFromWindow(inputView?.windowToken, 0)
        sessionToken++
        recognizer?.cancel()
        recognizer?.destroy()
        recognizer = null
        if (novaActive) nova.submitText(command) else submitCommand(command)
    }

    override fun onShow(args: Bundle?, showFlags: Int) {
        super.onShow(args, showFlags)
        visible = true
        val task = taskManager.state.value
        lastDeliveredRevision = task.revision
        scope.launch {
            val settings = novaSettingsStore.settings.first()
            if (!visible) return@launch
            novaActive = settings.enabled
            if (settings.enabled) {
                setVisualState(AssistantVisualState.LISTENING, "Connecting Nova voice…")
                nova.start(settings, { request ->
                    val before = taskManager.state.value.revision
                    taskManager.submit(request)
                    taskManager.state.first { it.revision > before && it.phase !in setOf(PhoneTaskPhase.Working, PhoneTaskPhase.Idle) }.message
                }, onStopTask = { taskManager.stop(); hide() })
            } else if (task.phase == PhoneTaskPhase.Working) {
                setVisualState(AssistantVisualState.THINKING, task.message)
            } else beginListening()
        }
    }

    private fun beginListening() {
        recognizer?.cancel()
        recognizer?.destroy()
        recognizer = null
        val token = ++sessionToken
        setVisualState(AssistantVisualState.LISTENING, "Listening…")
        val intent = Intent(android.speech.RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(android.speech.RecognizerIntent.EXTRA_LANGUAGE_MODEL, android.speech.RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(android.speech.RecognizerIntent.EXTRA_LANGUAGE, "en-IN")
            putExtra(android.speech.RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        }
        val recognitionComponent = HyuskRecognitionService.findSystemRecognizer(sessionService)
        recognizer = if (recognitionComponent != null) {
            android.speech.SpeechRecognizer.createSpeechRecognizer(sessionService, recognitionComponent)
        } else {
            android.speech.SpeechRecognizer.createSpeechRecognizer(sessionService)
        }.also {
            it.setRecognitionListener(object : android.speech.RecognitionListener {
                override fun onReadyForSpeech(params: Bundle?) = setVisualState(AssistantVisualState.LISTENING, "Listening…")
                override fun onBeginningOfSpeech() = setVisualState(AssistantVisualState.LISTENING, "Listening…")
                override fun onRmsChanged(rmsdB: Float) { orbView?.setMicrophoneLevel(rmsdB) }
                override fun onBufferReceived(buffer: ByteArray?) = Unit
                override fun onEndOfSpeech() = setVisualState(AssistantVisualState.THINKING, "Thinking…")
                override fun onError(error: Int) {
                    if (token != sessionToken) return
                    setVisualState(AssistantVisualState.ERROR, "I couldn't hear that")
                    speak("I couldn't hear that. Please try again, sir.")
                }
                override fun onResults(results: Bundle?) {
                    if (token != sessionToken) return
                    val text = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty()
                    setVisualState(
                        if (text.isBlank()) AssistantVisualState.ERROR else AssistantVisualState.THINKING,
                        if (text.isBlank()) "I didn’t catch that" else text,
                    )
                    submitCommand(text)
                }
                override fun onPartialResults(partialResults: Bundle?) {
                    val partial = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty()
                    if (partial.isNotBlank()) setVisualState(AssistantVisualState.LISTENING, partial)
                }
                override fun onEvent(eventType: Int, params: Bundle?) = Unit
            })
            runCatching { it.startListening(intent) }.onFailure { failure ->
                setVisualState(AssistantVisualState.ERROR, failure.message ?: "Speech recognition unavailable")
            }
        }
    }

    private fun submitCommand(raw: String) {
        val command = raw.trim()
        if (isVoiceStopCommand(command)) {
            taskManager.stop()
            hide()
            return
        }
        val normalized = command.lowercase().replace(Regex("\\s+"), " ")
        val now = SystemClock.elapsedRealtime()
        if (normalized.isNotBlank() && normalized == lastCommand && now - lastCommandAt < 4_000L) return
        lastCommand = normalized
        lastCommandAt = now
        if (command.isBlank()) {
            speak("Yes, sir. How can I help?")
            return
        }

        setVisualState(AssistantVisualState.THINKING, "Working…")
        val started = taskManager.submit(command)
        if (!started) setVisualState(AssistantVisualState.THINKING, "A task is already running. Wait for it to finish or stop it in Hyusk.")
    }

    private fun setVisualState(state: AssistantVisualState, message: String?) {
        orbView?.setState(state)
        orbView?.contentDescription = "Hyusk is ${state.name.lowercase()}"
        message?.takeIf { it.isNotBlank() }?.let { statusView?.text = it }
    }

    private fun speakProgress(text: CharSequence) {
        speak(text, append = false, completesTurn = false, listenAfter = false)
    }

    fun speak(text: CharSequence) {
        speak(text, append = false, completesTurn = true, listenAfter = false)
    }

    private fun speak(
        text: CharSequence,
        append: Boolean,
        completesTurn: Boolean,
        listenAfter: Boolean,
    ) {
        val raw = text.toString()
        val spoken = HyuskSpeech.normalize(raw)
        if (spoken.isBlank()) return
        setVisualState(AssistantVisualState.SPEAKING, raw)
        val request = SpeechRequest(spoken, append, completesTurn, listenAfter)
        if (!ttsReady) {
            pendingSpeech = request
            return
        }
        val utteranceId = "hyusk-${if (completesTurn) "final" else "progress"}-${++utteranceCounter}"
        if (completesTurn) {
            finalUtteranceId = utteranceId
            listenAfterFinalSpeech = listenAfter
        }
        tts?.speak(
            spoken,
            if (append) TextToSpeech.QUEUE_ADD else TextToSpeech.QUEUE_FLUSH,
            null,
            utteranceId,
        )
    }

    override fun onHide() {
        visible = false
        nova.stop()
        novaActive = false
        sessionToken++
        listenAfterFinalSpeech = false
        finalUtteranceId = null
        recognizer?.cancel()
        recognizer?.destroy()
        recognizer = null
        tts?.stop()
        super.onHide()
    }

    override fun onDestroy() {
        recognizer?.destroy()
        nova.close()
        scope.cancel()
        tts?.shutdown()
        tts = null
        super.onDestroy()
    }
}

private enum class AssistantVisualState { IDLE, LISTENING, THINKING, SPEAKING, ERROR }

private class AssistantButterflyView(context: android.content.Context) : View(context) {
    private val started = System.nanoTime()
    private var state = AssistantVisualState.LISTENING
    private var targetLevel = 0f
    private var displayedLevel = 0f
    private val homeButterfly = resources.getDrawable(io.github.hyusk.mobile.R.drawable.hyusk_butterfly_mark, context.theme)

    fun setState(value: AssistantVisualState) {
        state = value
        invalidate()
    }

    fun setMicrophoneLevel(rmsDb: Float) {
        targetLevel = ((rmsDb + 2f) / 12f).coerceIn(0f, 1f)
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val t = (System.nanoTime() - started) / 1_000_000_000f
        displayedLevel += (targetLevel - displayedLevel) * 0.18f
        targetLevel *= 0.92f
        val cadence = when (state) {
            AssistantVisualState.LISTENING -> 3.8f
            AssistantVisualState.THINKING -> 2.3f
            AssistantVisualState.SPEAKING -> 5.2f
            else -> 1.5f
        }
        val pulse = (kotlin.math.sin(t * cadence) + 1f) * 0.5f
        val scale = 1f + (pulse - .5f) * .035f + displayedLevel * .045f
        val side = (minOf(width, height) * .72f).toInt()
        canvas.save()
        canvas.scale(scale, scale, width / 2f, height / 2f)
        homeButterfly.setBounds(
            (width - side) / 2,
            (height - side) / 2,
            (width + side) / 2,
            (height + side) / 2,
        )
        homeButterfly.alpha = if (state == AssistantVisualState.ERROR) 180 else 255
        homeButterfly.draw(canvas)
        canvas.restore()
        postInvalidateOnAnimation()
    }
}
