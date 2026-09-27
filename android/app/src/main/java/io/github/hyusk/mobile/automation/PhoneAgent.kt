package io.github.hyusk.mobile.automation

import io.github.hyusk.mobile.data.HyuskRepository
import io.github.hyusk.mobile.network.LocalModelClient
import io.github.hyusk.mobile.security.LocalModelSettings
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

enum class PhoneAgentProgressKind { Status, ModelAcknowledgement }

/** A status may update the UI; only a model acknowledgement is suitable for speech. */
data class PhoneAgentProgress(
    val text: String,
    val kind: PhoneAgentProgressKind = PhoneAgentProgressKind.Status,
) {
    val shouldSpeak: Boolean get() = kind == PhoneAgentProgressKind.ModelAcknowledgement
}

data class PhoneAgentResult(
    val message: String,
    val executed: Boolean,
    val checkpointed: Boolean = false,
    val needsReply: Boolean = false,
    val replyType: String = "none",
    val taskId: String? = null,
    val autoContinue: Boolean = false,
)

/** Self-reliant phone loop: deterministic actions first, model-selected actions second. */
class PhoneAgent(
    private val router: LocalCommandRouter,
    private val executor: DeviceActionExecutor,
    private val model: LocalModelClient,
    private val repository: HyuskRepository,
) {
    private data class PendingAction(
        val action: String,
        val arguments: JSONObject,
        val signature: String,
    )

    /* One device has one foreground UI. Serialize it so separate assistant sessions
       cannot concurrently press controls or consume each other's checkpoint. */
    private val turnMutex = Mutex()

    suspend fun run(
        prompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit = {},
        internalContinuation: Boolean = false,
    ): PhoneAgentResult = turnMutex.withLock {
        runLocked(prompt, settings, onProgress, internalContinuation)
    }

    private suspend fun runLocked(
        prompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit,
        internalContinuation: Boolean,
    ): PhoneAgentResult {
        val cleanPrompt = prompt.trim()
        if (cleanPrompt.isBlank()) return PhoneAgentResult("Tell me what you want me to do.", false)

        if (!internalContinuation) repository.recordTurn("user", cleanPrompt)
        val isConfirmation = CONFIRMATION.matches(cleanPrompt)
        val isDecline = DECLINE.matches(cleanPrompt)
        val isContinuation = internalContinuation || CONTINUATION.matches(cleanPrompt)
        val awaiting = repository.latestAwaitingAgentRun()?.takeIf { (_, state) -> isFreshAwaiting(state) }
        val pending = awaiting?.second?.optJSONObject("pending_confirmation").pendingAction()

        if (isDecline && awaiting != null && pending != null) {
            repository.completeAgentRun(awaiting.first, "Confirmation declined by the user.", "cancelled")
            return finish("Okay, I will not do that.", false, taskId = awaiting.first)
        }

        val resumed = when {
            awaiting != null && resumesAwaitingRun(cleanPrompt, pending != null) -> awaiting
            isContinuation && awaiting == null -> repository.latestPausedAgentRun() ?: repository.latestAgentRun()
            else -> null
        }
        if (awaiting != null && resumed?.first != awaiting.first) {
            // A new command is not implicitly an answer to an old question.
            // In particular, it must never revive an abandoned approval.
            repository.completeAgentRun(awaiting.first, "Superseded by a new user request.", "cancelled")
        }

        val taskId = resumed?.first ?: UUID.randomUUID().toString()
        val savedState = resumed?.second
        val taskPrompt = savedState?.optString("prompt")?.takeIf(String::isNotBlank) ?: cleanPrompt
        val observations = savedState?.optJSONArray("observations").toStringList().toMutableList()
        val attempts = savedState?.optJSONObject("attempts").toAttemptMap()
        val resumedPending = savedState?.optJSONObject("pending_confirmation").pendingAction()

        savedState?.optJSONObject("in_flight")?.pendingAction()?.let { action ->
            addObservation(
                observations,
                "The previous ${action.action} action was interrupted before its result was known. " +
                    "Verify the current state before repeating it.",
            )
        }

        if (resumed != null && !isContinuation && !(isConfirmation && resumedPending != null)) {
            addObservation(observations, "User follow-up: $cleanPrompt")
        }

        if (!(isConfirmation && resumedPending != null)) {
            saveCheckpoint(taskId, taskPrompt, observations, attempts)
        }
        val result = try {
            withTimeoutOrNull(TURN_TIMEOUT_MILLIS) {
                if (isConfirmation && resumedPending != null) {
                    runConfirmedAction(
                        taskId,
                        taskPrompt,
                        settings,
                        onProgress,
                        observations,
                        attempts,
                        resumedPending,
                    )
                } else {
                    runWithinDeadline(taskPrompt, settings, onProgress, taskId, observations, attempts)
                }
            }
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable) {
                if (cancelled is UserStoppedTask) {
                    repository.completeAgentRun(taskId, "Stopped by the user.", "cancelled")
                } else {
                    // Process or UI interruption keeps a checkpoint for explicit continuation.
                    repository.setAgentRunStatus(taskId, "paused")
                }
            }
            throw cancelled
        } catch (failure: Exception) {
            repository.completeAgentRun(taskId, failure.message ?: "Task failed", "failed")
            throw failure
        }

        if (result != null) {
            if (!result.checkpointed && !result.needsReply) {
                repository.completeAgentRun(
                    taskId,
                    result.message,
                    if (result.executed) "completed" else "finished",
                )
            }
            return result.copy(taskId = taskId)
        }

        repository.setAgentRunStatus(taskId, "paused")
        return finish(
            "This task reached its current execution window. I saved the verified progress, so saying continue will resume without repeating completed actions.",
            false,
            checkpointed = true,
            taskId = taskId,
        )
    }

    private suspend fun runConfirmedAction(
        taskId: String,
        taskPrompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit,
        observations: MutableList<String>,
        attempts: MutableMap<String, Int>,
        pending: PendingAction,
    ): PhoneAgentResult {
        val previousAttempts = attempts.getOrDefault(pending.signature, 0)
        if (previousAttempts >= attemptLimit(pending.action)) {
            return stopForNoProgress(taskId, taskPrompt, observations, attempts)
        }

        attempts[pending.signature] = previousAttempts + 1
        // Store the attempt before executing. If Android kills this process or the
        // turn is cancelled during the action, a later continuation must verify
        // rather than blindly repeat a possibly completed external effect.
        saveCheckpoint(taskId, taskPrompt, observations, attempts, inFlight = pending)
        publishStatus(onProgress, "Running confirmed ${pending.action.replace('.', ' ')}…")
        currentCoroutineContext().ensureActive()
        executeAndObserve(
            action = pending.action,
            arguments = pending.arguments,
            observationLabel = "Confirmed action",
            observations = observations,
            onProgress = onProgress,
        )
        saveCheckpoint(taskId, taskPrompt, observations, attempts)

        // Approval authorizes one action, not completion of the whole task. Resume
        // model planning from the resulting device state without replaying the
        // original deterministic command/workflow dispatch.
        return runWithinDeadline(
            taskPrompt,
            settings,
            onProgress,
            taskId,
            observations,
            attempts,
            resumeAfterAction = true,
        )
    }

    private suspend fun runWithinDeadline(
        taskPrompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit,
        taskId: String,
        observations: MutableList<String>,
        attempts: MutableMap<String, Int>,
        resumeAfterAction: Boolean = false,
    ): PhoneAgentResult {
        if (!resumeAfterAction) {
            Regex("(?i)^remember(?: that)?\\s+(.+)$").find(taskPrompt)?.let {
                repository.remember(it.groupValues[1])
                return finish("I’ll remember that.", true, taskId = taskId)
            }
            if (taskPrompt.matches(Regex("(?i)^forget (?:all|everything)(?: you know| about me)?[.! ]*$"))) {
                repository.forgetAllMemories()
                return finish("I deleted all durable memories. Recent conversation is still retained temporarily.", true, taskId = taskId)
            }
            if (taskPrompt.matches(Regex("(?i).*(what|show|tell).*(remember|know about me).*"))) {
                val memories = repository.recalledMemories()
                return finish(
                    if (memories.isEmpty()) "I don’t have any saved facts yet." else memories.joinToString("\n• ", "I remember:\n• "),
                    false,
                    taskId = taskId,
                )
            }

            repository.workflowForCommand(taskPrompt)?.let { workflow ->
                publishStatus(onProgress, "Running ${workflow.name} locally…")
                saveCheckpoint(taskId, taskPrompt, observations, attempts) { state ->
                    state.put("kind", "workflow").put("label", workflow.name)
                }
                currentCoroutineContext().ensureActive()
                val outcome = runCatching {
                    executeWorkflowDefinition(repository.decryptWorkflowDefinition(workflow), executor)
                }
                return outcome.fold(
                    onSuccess = { completed -> finish("Completed ${workflow.name}: $completed.", true, taskId = taskId) },
                    onFailure = { failure ->
                        finish("${workflow.name} stopped: ${failure.message ?: "workflow failed"}.", false, taskId = taskId)
                    },
                )
            }
            router.execute(taskPrompt)?.let { direct ->
                if (direct.success) {
                    val launchedPackage = direct.data.optString("package")
                    if (launchedPackage.isBlank()) return finish(direct.message, true, taskId = taskId)
                    val label = direct.data.optString("label", launchedPackage)
                    publishStatus(onProgress, "Opening $label…")
                    if (waitForPackage(launchedPackage, APP_TRANSITION_TIMEOUT_MS)) {
                        addObservation(observations, "Verified foreground app: $label ($launchedPackage)")
                        return finish(direct.message, true, taskId = taskId)
                    }
                    addObservation(observations,
                        "App launch was requested for $label, but Android did not expose it as the foreground accessibility window. Inspect the current screen before retrying.")
                    publishStatus(onProgress, "I opened $label but couldn't verify its screen; checking what Android shows.")
                } else {
                    addObservation(observations, "Deterministic action failed: ${direct.message}")
                    publishStatus(onProgress, "Couldn't complete the direct phone action; checking the current screen. ${direct.message}")
                }
            }
        }

        val memory = repository.memoryContext(taskPrompt)
        val spokenProgress = mutableSetOf<String>()
        var invalidPlans = 0
        var completionReviewed = false
        var unresolvedActionIssue = lastActionIssue(observations)
        repeat(MAX_ACTIONS) actionLoop@ { step ->
            currentCoroutineContext().ensureActive()
            refreshUiObservation(observations)
            publishStatus(onProgress, "Working · ${step + 1} verified action${if (step == 0) "" else "s"}")
            val answer = try {
                model.complete(settings, taskPrompt, memory, modelObservations(observations, attempts))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                val issue = unresolvedActionIssue
                if (issue != null) {
                    throw IllegalStateException("Model recovery failed after $issue: ${failure.message ?: "model request failed"}", failure)
                }
                throw failure
            }
            val structured = parseObject(answer)
            if (structured == null) {
                addObservation(observations, "The model returned an invalid action plan; it was not executed or accepted as completion: ${answer.take(300)}")
                saveCheckpoint(taskId, taskPrompt, observations, attempts)
                if (++invalidPlans < 2) return@actionLoop
                saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused")
                return finish("The model did not provide a valid action plan. Your progress is saved; choose another model or say continue.", false, checkpointed = true, taskId = taskId)
            }
            invalidPlans = 0

            structured.optString("progress_text").trim().takeIf(String::isNotBlank)?.let { progress ->
                if (spokenProgress.add(progress)) {
                    onProgress(PhoneAgentProgress(progress, PhoneAgentProgressKind.ModelAcknowledgement))
                }
            }

            val reply = structured.optString("reply").trim()
            val needsReply = structured.optBoolean("needs_reply", false)
            val replyType = normalizedReplyType(structured.optString("reply_type"), needsReply)
            if (reply.isNotBlank()) {
                if (unresolvedActionIssue != null) {
                    val issue = unresolvedActionIssue
                    val message = if (needsReply) "$reply I couldn't continue because $issue."
                    else "I paused because I couldn't verify this step: $issue. Your progress is saved; tell me how you'd like me to proceed."
                    saveCheckpoint(
                        taskId,
                        taskPrompt,
                        observations,
                        attempts,
                        status = if (needsReply) "awaiting_user" else "paused",
                        replyType = if (needsReply) replyType else "none",
                    )
                    return finish(
                        message,
                        false,
                        checkpointed = !needsReply,
                        needsReply = needsReply,
                        replyType = replyType,
                        taskId = taskId,
                    )
                }
                if (!needsReply && attempts.isNotEmpty() && !completionReviewed) {
                    completionReviewed = true
                    addObservation(observations,
                        "Proposed final reply: ${reply.take(700)}. Before ending, compare every part of the original user request with verified action results and the latest screen. If anything remains, return the next action; reply only if all parts are done.")
                    saveCheckpoint(taskId, taskPrompt, observations, attempts)
                    return@actionLoop
                }
                if (needsReply) {
                    saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "awaiting_user", replyType = replyType)
                }
                return finish(reply, observations.isNotEmpty(), needsReply = needsReply, replyType = replyType, taskId = taskId)
            }

            val action = structured.optString("action").trim()
            if (action.isBlank()) {
                addObservation(observations, "The model returned neither a reply nor an action. Return one valid JSON action plan.")
                saveCheckpoint(taskId, taskPrompt, observations, attempts)
                if (++invalidPlans < 2) return@actionLoop
                saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused")
                return finish("The model did not provide an action. Your progress is saved; say continue after choosing a more capable model.", false, checkpointed = true, taskId = taskId)
            }
            completionReviewed = false
            val arguments = structured.optJSONObject("arguments") ?: JSONObject()
            if (action == "memory.remember") {
                val fact = arguments.optString("text").trim()
                if (fact.isBlank()) {
                    addObservation(observations, "Memory was not saved because text was empty.")
                } else {
                    repository.remember(fact, arguments.optString("kind", "fact"))
                    addObservation(observations, "Saved durable on-device memory: $fact")
                }
                saveCheckpoint(taskId, taskPrompt, observations, attempts)
                return@actionLoop
            }

            val signature = "$action:${arguments}|screen=${screenFingerprint(observations)}"
            val previousAttempts = attempts.getOrDefault(signature, 0)
            if (previousAttempts >= attemptLimit(action)) {
                return stopForNoProgress(taskId, taskPrompt, observations, attempts, unresolvedActionIssue)
            }
            val pending = PendingAction(action, arguments, signature)
            if (executor.risk(action, arguments) != ActionRisk.Safe) {
                saveCheckpoint(
                    taskId,
                    taskPrompt,
                    observations,
                    attempts,
                    status = "awaiting_user",
                    pending = pending,
                    replyType = "confirmation",
                )
                return finish(
                    "This action can affect your data or contact someone. Say confirm to allow it once.",
                    false,
                    needsReply = true,
                    replyType = "confirmation",
                    taskId = taskId,
                )
            }

            attempts[signature] = previousAttempts + 1
            saveCheckpoint(taskId, taskPrompt, observations, attempts, inFlight = pending)
            publishStatus(onProgress, "Running ${action.replace('.', ' ')}…")
            currentCoroutineContext().ensureActive()
            val actionIssue = executeAndObserve(action, arguments, "Step ${step + 1}", observations, onProgress)
            if (actionIssue != null) unresolvedActionIssue = actionIssue
            else if (action !in setOf("apps.list", "ui.snapshot")) unresolvedActionIssue = null
            saveCheckpoint(taskId, taskPrompt, observations, attempts)
        }
        saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused")
        val issue = unresolvedActionIssue ?: lastActionIssue(observations)
        return finish(
            if (issue == null) "This execution window ended; verified progress is saved."
            else "This execution window ended after $issue. I saved the verified progress; say continue to resume or give me another way to proceed.",
            false,
            checkpointed = true,
            taskId = taskId,
            autoContinue = true,
        )
    }

    private suspend fun executeAndObserve(
        action: String,
        arguments: JSONObject,
        observationLabel: String,
        observations: MutableList<String>,
        onProgress: (PhoneAgentProgress) -> Unit = {},
    ): String? {
        val beforeTree = latestScreenTree(observations)
        if (action in UI_TARGET_ACTIONS && beforeTree == null) {
            val issue = "$action was not attempted because no accessibility snapshot is available"
            addObservation(observations, "$observationLabel: $issue")
            publishStatus(onProgress, "I can't see the phone's current controls, so I didn't guess. Check Hyusk Accessibility and try again.")
            return issue
        }
        val result = executor.execute(action, arguments)
        var screenChanged: Boolean? = null
        if (action in UI_CHANGING_ACTIONS && result.success) {
            screenChanged = when {
                action == "apps.launch" && result.data.optString("package").isNotBlank() -> {
                    val packageName = result.data.optString("package")
                    val verified = waitForPackage(packageName, APP_TRANSITION_TIMEOUT_MS)
                    if (verified) addObservation(observations, "Verified foreground app: ${result.data.optString("label", packageName)} ($packageName)")
                    verified
                }
                action == "ui.set_text" -> waitForTextVisible(
                    beforeTree,
                    arguments.optString("text"),
                    UI_TRANSITION_TIMEOUT_MS,
                    observations,
                )
                else -> {
                    val timeout = if (action == "url.open") APP_TRANSITION_TIMEOUT_MS else UI_TRANSITION_TIMEOUT_MS
                    waitForUiAfterAction(beforeTree, timeout, observations)
                }
            }
        }
        val outcome = when {
            !result.success -> "failed"
            screenChanged == false -> "was dispatched, but no accessibility change was verified"
            else -> "succeeded"
        }
        addObservation(
            observations,
            "$observationLabel: $action $outcome: ${result.message}. Data: ${result.data}",
        )
        val issue = when {
            !result.success -> "$action failed: ${result.message}"
            screenChanged == false -> "$action was dispatched, but the accessibility screen did not change; ${result.message}"
            else -> null
        }
        if (issue != null) {
            publishStatus(onProgress, "Couldn't verify $action; checking another route. ${result.message}")
        }
        return issue
    }

    private fun lastActionIssue(observations: List<String>): String? {
        val lastAction = observations.lastOrNull {
            it.startsWith("Step ") || it.startsWith("Confirmed action:") || it.startsWith("Deterministic action failed:")
        } ?: return null
        if (!lastAction.contains(" failed:") && !lastAction.contains("no accessibility change was verified")) return null
        return lastAction.substringAfter(": ").substringBefore(". Data:").take(500)
    }

    private fun refreshUiObservation(observations: MutableList<String>) {
        val screen = executor.execute("ui.snapshot", JSONObject())
        if (!screen.success) return
        val tree = screen.data.optString("tree")
        if (tree.isNotBlank() && tree != latestScreenTree(observations))
            addObservation(observations, "Current accessibility tree: $tree")
    }

    private suspend fun waitForUiAfterAction(
        beforeTree: String?,
        timeoutMillis: Long,
        observations: MutableList<String>,
    ): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMillis
        var lastTree: String? = null
        var stableSamples = 0
        var changed = false
        while (System.currentTimeMillis() < deadline) {
            currentCoroutineContext().ensureActive()
            delay(UI_POLL_INTERVAL_MILLIS)
            val screen = executor.execute("ui.snapshot", JSONObject())
            if (!screen.success) continue

            val tree = screen.data.optString("tree")
            if (tree.isBlank()) continue
            if (beforeTree != null && tree != beforeTree) changed = true
            if (changed && tree == lastTree) {
                stableSamples++
                if (stableSamples >= 1) {
                    addObservation(observations, "Current accessibility tree: $tree")
                    return true
                }
            } else {
                stableSamples = 0
            }
            lastTree = tree
        }

        // Keep the latest observation even when Android exposed no detectable
        // screen change; the model must not treat dispatch success as proof of progress.
        lastTree?.let { addObservation(observations, "Current accessibility tree: $it") }
        return changed
    }

    private fun latestScreenTree(observations: List<String>): String? = observations
        .lastOrNull { it.startsWith(SCREEN_OBSERVATION_PREFIX) }
        ?.removePrefix(SCREEN_OBSERVATION_PREFIX)

    private suspend fun waitForPackage(packageName: String, timeoutMillis: Long): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMillis
        while (System.currentTimeMillis() < deadline) {
            currentCoroutineContext().ensureActive()
            val snapshot = executor.execute("ui.snapshot", JSONObject())
            val tree = snapshot.data.optJSONObject("tree")
            if (snapshot.success && tree?.optString("package") == packageName) return true
            delay(UI_POLL_INTERVAL_MILLIS)
        }
        return false
    }

    private suspend fun waitForTextVisible(
        beforeTree: String?,
        expected: String,
        timeoutMillis: Long,
        observations: MutableList<String>,
    ): Boolean {
        if (expected.isBlank()) return false
        val deadline = System.currentTimeMillis() + timeoutMillis
        var latestTree: String? = null
        while (System.currentTimeMillis() < deadline) {
            currentCoroutineContext().ensureActive()
            val snapshot = executor.execute("ui.snapshot", JSONObject())
            val tree = snapshot.data.optJSONObject("tree")?.toString()
            if (snapshot.success && !tree.isNullOrBlank()) {
                latestTree = tree
                if (tree.contains(expected)) {
                    if (tree != beforeTree) addObservation(observations, "Current accessibility tree: $tree")
                    return true
                }
            }
            delay(UI_POLL_INTERVAL_MILLIS)
        }
        latestTree?.let { if (it != beforeTree) addObservation(observations, "Current accessibility tree: $it") }
        return false
    }

    private fun screenFingerprint(observations: List<String>): String {
        val tree = latestScreenTree(observations) ?: return "unknown"
        val digest = MessageDigest.getInstance("SHA-256").digest(tree.toByteArray(Charsets.UTF_8))
        return digest.take(SCREEN_FINGERPRINT_BYTES).joinToString("") { byte ->
            (byte.toInt() and 0xff).toString(16).padStart(2, '0')
        }
    }

    private suspend fun saveCheckpoint(
        taskId: String,
        prompt: String,
        observations: List<String>,
        attempts: Map<String, Int>,
        status: String = "active",
        pending: PendingAction? = null,
        inFlight: PendingAction? = null,
        replyType: String = "none",
        mutate: (JSONObject) -> Unit = {},
    ) {
        val state = checkpoint(prompt, observations, attempts)
        pending?.let { state.put("pending_confirmation", it.toJson()) }
        inFlight?.let { state.put("in_flight", it.toJson()) }
        if (status == "awaiting_user") {
            state.put("awaiting_user", JSONObject()
                .put("reply_type", replyType)
                .put("created_at", System.currentTimeMillis()))
        }
        mutate(state)
        repository.saveAgentRun(taskId, state, status)
    }

    private suspend fun stopForNoProgress(
        taskId: String,
        taskPrompt: String,
        observations: List<String>,
        attempts: Map<String, Int>,
        unresolvedIssue: String? = null,
    ): PhoneAgentResult {
        saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused")
        val issue = unresolvedIssue ?: lastActionIssue(observations)
        return finish(
            if (issue == null) "I paused because the same action produced no verified screen change. Your progress is saved; tell me another way to proceed or continue after checking the phone."
            else "I paused because I couldn't complete this step: $issue. I saved your progress; tell me another way to proceed or continue after checking the phone.",
            false,
            checkpointed = true,
            taskId = taskId,
        )
    }

    private suspend fun finish(
        message: String,
        executed: Boolean,
        checkpointed: Boolean = false,
        needsReply: Boolean = false,
        replyType: String = "none",
        taskId: String? = null,
        autoContinue: Boolean = false,
    ): PhoneAgentResult {
        if (!autoContinue) repository.recordTurn("assistant", message)
        return PhoneAgentResult(message, executed, checkpointed, needsReply, replyType, taskId, autoContinue)
    }

    private fun publishStatus(onProgress: (PhoneAgentProgress) -> Unit, text: String) {
        onProgress(PhoneAgentProgress(text))
    }

    private fun parseObject(raw: String): JSONObject? = runCatching {
        val clean = raw.trim().replace(Regex("^```(?:json)?\\s*", RegexOption.IGNORE_CASE), "")
            .replace(Regex("\\s*```$"), "").trim()
        val first = clean.indexOf('{')
        val last = clean.lastIndexOf('}')
        require(first >= 0 && last > first)
        JSONObject(clean.substring(first, last + 1))
    }.getOrNull()

    private fun addObservation(observations: MutableList<String>, value: String) {
        observations += value.take(MAX_OBSERVATION_CHARS)
        while (observations.size > MAX_OBSERVATIONS) observations.removeAt(0)
    }

    private fun modelObservations(observations: List<String>, attempts: Map<String, Int>): List<String> {
        if (attempts.isEmpty()) return observations
        val ledger = attempts.entries.toList().takeLast(MAX_ACTION_LEDGER_ENTRIES).joinToString("\n") { (signature, count) ->
            val action = signature.substringBefore(SCREEN_SIGNATURE_SEPARATOR)
            "Already attempted ($count) on the same screen: ${action.take(MAX_ACTION_LEDGER_ENTRY_CHARS)}"
        }
        return observations + "Persistent action ledger; verify state before repeating any entry:\n$ledger"
    }

    private fun checkpoint(prompt: String, observations: List<String>, attempts: Map<String, Int>): JSONObject = JSONObject()
        .put("prompt", prompt)
        .put("completed_steps", attempts.values.sum())
        .put("observations", JSONArray().also { array -> observations.forEach(array::put) })
        .put("attempts", JSONObject().also { objectValue -> attempts.forEach { (key, count) -> objectValue.put(key, count) } })

    private fun PendingAction.toJson(): JSONObject = JSONObject()
        .put("action", action)
        .put("arguments", arguments)
        .put("signature", signature)

    private fun JSONObject?.pendingAction(): PendingAction? {
        val value = this ?: return null
        val action = value.optString("action").trim()
        if (action.isBlank()) return null
        val arguments = value.optJSONObject("arguments") ?: JSONObject()
        val signature = value.optString("signature").ifBlank { "$action:$arguments" }
        return PendingAction(action, arguments, signature)
    }

    private fun JSONObject?.toAttemptMap(): MutableMap<String, Int> {
        val value = this ?: return mutableMapOf()
        val attempts = mutableMapOf<String, Int>()
        val keys = value.keys()
        while (keys.hasNext()) {
            val key = keys.next()
            value.optInt(key, 0).takeIf { it > 0 }?.let { attempts[key] = it }
        }
        return attempts
    }

    private fun JSONArray?.toStringList(): List<String> = buildList {
        val array = this@toStringList ?: return@buildList
        for (index in 0 until array.length()) array.optString(index).takeIf(String::isNotBlank)?.let(::add)
    }

    private fun normalizedReplyType(value: String, needsReply: Boolean): String = when (value.lowercase()) {
        "question", "choice", "confirmation" -> value.lowercase()
        else -> if (needsReply) "question" else "none"
    }

    private fun isFreshAwaiting(state: JSONObject): Boolean {
        val createdAt = state.optJSONObject("awaiting_user")?.optLong("created_at", 0L) ?: 0L
        return createdAt > 0 && System.currentTimeMillis() - createdAt <= AWAITING_REPLY_WINDOW_MILLIS
    }

    private fun attemptLimit(action: String): Int = if (action in REPEATABLE_ACTIONS) 4 else 2

    internal companion object {
        val CONFIRMATION = Regex("(?i)^(yes|confirm|allow|do it|go ahead)[.! ]*$")
        val DECLINE = Regex("(?i)^(no|cancel|stop|don't|do not)[.! ]*$")
        val CONTINUATION = Regex("(?i)^(continue|resume|keep going)[.! ]*$")
        fun resumesAwaitingRun(input: String, hasPendingConfirmation: Boolean): Boolean =
            if (hasPendingConfirmation) CONFIRMATION.matches(input) else CONTINUATION.matches(input)
        const val MAX_ACTIONS = 64
        const val TURN_TIMEOUT_MILLIS = 600_000L
        const val AWAITING_REPLY_WINDOW_MILLIS = 15 * 60_000L
        const val MAX_OBSERVATIONS = 6
        const val MAX_OBSERVATION_CHARS = 8_000
        const val MAX_ACTION_LEDGER_ENTRIES = 24
        const val MAX_ACTION_LEDGER_ENTRY_CHARS = 320
        const val SCREEN_OBSERVATION_PREFIX = "Current accessibility tree: "
        const val SCREEN_SIGNATURE_SEPARATOR = "|screen="
        const val SCREEN_FINGERPRINT_BYTES = 8
        const val UI_POLL_INTERVAL_MILLIS = 120L
        const val UI_TRANSITION_TIMEOUT_MS = 1_400L
        const val APP_TRANSITION_TIMEOUT_MS = 2_400L
        val REPEATABLE_ACTIONS = setOf(
            "ui.snapshot", "ui.scroll", "ui.swipe", "ui.tap", "volume.up", "volume.down", "media.next", "media.previous",
        )
        val UI_CHANGING_ACTIONS = setOf(
            "apps.launch", "url.open", "web.search", "system.settings", "system.back", "system.home",
            "system.recents", "system.notifications", "system.quick_settings", "ui.click", "ui.long_click",
            "ui.set_text", "ui.scroll", "ui.swipe", "ui.tap", "ui.press_enter", "camera.open", "phone.dial", "sms.compose",
        )
        val UI_TARGET_ACTIONS = setOf(
            "ui.click", "ui.long_click", "ui.set_text", "ui.scroll", "ui.tap", "ui.swipe", "ui.press_enter",
        )
    }
}
