package io.github.hyusk.mobile.automation

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import io.github.hyusk.mobile.MainActivity
import io.github.hyusk.mobile.R
import io.github.hyusk.mobile.services.PhoneTaskForegroundService
import io.github.hyusk.mobile.data.HyuskRepository
import io.github.hyusk.mobile.network.LocalModelClient
import io.github.hyusk.mobile.security.LocalModelSettingsStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

enum class PhoneTaskPhase { Idle, Working, Finished, Paused, AwaitingReply, Failed }

internal class UserStoppedTask : CancellationException("Stopped by user")

data class PhoneTaskState(
    val phase: PhoneTaskPhase = PhoneTaskPhase.Idle,
    val message: String = "",
    val revision: Long = 0,
    val speakProgress: Boolean = false,
    val needsReply: Boolean = false,
)

/** A phone task belongs to the app process, not to an assistant overlay or Compose screen. */
class PhoneAgentTaskManager private constructor(private val context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val repository = HyuskRepository(context)
    private val agent = PhoneAgent(
        LocalCommandRouter(context), DeviceActionExecutor(context), LocalModelClient(), repository,
    )
    private val settings = LocalModelSettingsStore(context)
    private val recoveryJob = scope.launch { repository.pauseOrphanedActiveRuns() }
    private val mutableState = MutableStateFlow(PhoneTaskState())
    val state = mutableState.asStateFlow()
    private val mutableSessionId = MutableStateFlow<String?>(null)
    val activeSessionId = mutableSessionId.asStateFlow()
    private val sessionInitialization = scope.launch {
        recoveryJob.join()
        val sessionId = repository.ensureActiveSession()
        mutableSessionId.value = sessionId
        repository.latestPausedAgentRun(sessionId)?.second?.let { state ->
            val prompt = state.optString("prompt").take(72)
            publish(
                PhoneTaskPhase.Paused,
                "A saved task was interrupted${if (prompt.isBlank()) "" else ": $prompt"}. Say continue to resume it.",
            )
        }
    }
    private var activeJob: Job? = null
    private var pendingReplacement: String? = null
    private var revision = 0L

    suspend fun startNewConversation(): String {
        check(activeJob?.isActive != true) { "Stop the current phone task before changing conversations." }
        val id = repository.createSession()
        mutableSessionId.value = id
        return id
    }

    suspend fun selectConversation(id: String): Boolean {
        check(activeJob?.isActive != true) { "Stop the current phone task before changing conversations." }
        val selected = repository.activateSession(id)
        if (selected) mutableSessionId.value = id
        return selected
    }

    fun submit(prompt: String): Boolean {
        if (prompt.isBlank()) return false
        if (activeJob != null) {
            // The newest instruction wins. Wait for the old run to release the
            // device before starting it, so two agents never tap the screen.
            pendingReplacement = prompt
            activeJob?.cancel(UserStoppedTask())
            return true
        }
        val backgroundProtected = startForegroundWork()
        publish(
            PhoneTaskPhase.Working,
            if (backgroundProtected) "Working…" else "Working, but Android background protection could not start. Keep Hyusk open while I work.",
        )
        val task = scope.launch(start = CoroutineStart.LAZY) {
            try {
                sessionInitialization.join()
                val selectedSettings = settings.settings.first()
                val sessionId = mutableSessionId.value ?: repository.ensureActiveSession().also { mutableSessionId.value = it }
                var result = agent.run(prompt, selectedSettings, { progress ->
                    publish(PhoneTaskPhase.Working, progress.text, progress.shouldSpeak)
                }, sessionId = sessionId)
                var windows = 1
                while (result.autoContinue && windows < MAX_EXECUTION_WINDOWS) {
                    publish(PhoneTaskPhase.Working, "Continuing from verified progress…")
                    result = agent.run("continue", selectedSettings, { progress ->
                        publish(PhoneTaskPhase.Working, progress.text, progress.shouldSpeak)
                    }, internalContinuation = true, sessionId = sessionId)
                    windows++
                }
                if (result.autoContinue) {
                    result = result.copy(message = "I reached the task safety limit. Your verified progress is saved; say continue to resume.")
                }
                val phase = when {
                    result.needsReply -> PhoneTaskPhase.AwaitingReply
                    result.checkpointed -> PhoneTaskPhase.Paused
                    else -> PhoneTaskPhase.Finished
                }
                publish(phase, result.message, needsReply = result.needsReply)
                runCatching { notifyFinished(phase) }
            } catch (cancelled: CancellationException) {
                if (cancelled is UserStoppedTask) {
                    if (pendingReplacement == null) {
                        publish(PhoneTaskPhase.Finished, "Stopped. Send a new request whenever you're ready.")
                        runCatching { notifyFinished(PhoneTaskPhase.Finished) }
                    }
                } else {
                    publish(PhoneTaskPhase.Paused, "Interrupted. Verified progress is saved; say continue to resume.")
                    runCatching { notifyFinished(PhoneTaskPhase.Paused) }
                }
            } catch (failure: Exception) {
                publish(PhoneTaskPhase.Failed, failure.message ?: "Phone task failed")
                runCatching { notifyFinished(PhoneTaskPhase.Failed) }
            } finally {
                activeJob = null
                pendingReplacement?.let { next ->
                    pendingReplacement = null
                    submit(next)
                } ?: stopForegroundWork()
            }
        }
        activeJob = task
        task.start()
        return true
    }

    fun stop() {
        pendingReplacement = null
        activeJob?.cancel(UserStoppedTask())
    }

    private fun startForegroundWork(): Boolean {
        val intent = Intent(context, PhoneTaskForegroundService::class.java)
        return runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(intent)
            else context.startService(intent)
            true
        }.onFailure { failure ->
            android.util.Log.w("HyuskPhoneTask", "Could not start foreground task notification", failure)
        }.getOrDefault(false)
    }

    private fun stopForegroundWork() {
        runCatching { context.stopService(Intent(context, PhoneTaskForegroundService::class.java)) }
    }

    private fun publish(phase: PhoneTaskPhase, message: String, speakProgress: Boolean = false, needsReply: Boolean = false) {
        mutableState.value = PhoneTaskState(phase, message, ++revision, speakProgress, needsReply)
    }

    private fun notifyFinished(phase: PhoneTaskPhase) {
        if (Build.VERSION.SDK_INT >= 33 && context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channel = "hyusk_agent_runs"
        manager.createNotificationChannel(NotificationChannel(channel, "Hyusk tasks", NotificationManager.IMPORTANCE_DEFAULT))
        val intent = PendingIntent.getActivity(
            context, 0, Intent(context, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val label = when (phase) {
            PhoneTaskPhase.Finished -> "Task finished"
            PhoneTaskPhase.Paused -> "Task paused — say continue to resume"
            PhoneTaskPhase.AwaitingReply -> "Hyusk needs your reply"
            else -> "Task stopped"
        }
        manager.notify(1702, Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_hyusk)
            .setContentTitle("Hyusk")
            .setContentText(label)
            .setContentIntent(intent)
            .setAutoCancel(true)
            .build())
    }

    companion object {
        private const val MAX_EXECUTION_WINDOWS = 3
        @Volatile private var instance: PhoneAgentTaskManager? = null

        fun get(context: Context): PhoneAgentTaskManager = instance ?: synchronized(this) {
            instance ?: PhoneAgentTaskManager(context.applicationContext).also { instance = it }
        }
    }
}
