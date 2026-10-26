package io.github.hyusk.mobile.automation

import io.github.hyusk.mobile.data.HyuskRepository
import io.github.hyusk.mobile.network.LocalModelClient
import io.github.hyusk.mobile.network.ModelToolExchange
import io.github.hyusk.mobile.security.LocalModelSettings
import io.github.hyusk.mobile.services.HyuskAccessibilityService
import io.github.hyusk.mobile.services.accessibilityTextMatchRank
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

internal data class EditableFieldLocator(val id: String, val bounds: String)

/** Resolve the editable control from the pre-action summary, never from matching text elsewhere. */
internal fun editableFieldLocator(summary: JSONObject, target: String): EditableFieldLocator? {
    val controls = summary.optJSONArray("interactive_controls") ?: return null
    val candidates = (0 until controls.length()).mapNotNull { controls.optJSONObject(it) }
        .filter { it.optBoolean("editable") }
    val matching = if (target.isBlank()) {
        candidates.filter { it.optBoolean("focused") }
    } else {
        val ranked = candidates.map { control ->
            val labelRank = accessibilityTextMatchRank(control.optString("label"), null, target)
            val id = control.optString("id")
            val idRank = if (id.equals(target.trim(), ignoreCase = true) ||
                id.substringAfterLast('/').equals(target.trim(), ignoreCase = true)) 0 else Int.MAX_VALUE
            control to minOf(labelRank, idRank)
        }
        val best = ranked.minOfOrNull { it.second } ?: Int.MAX_VALUE
        ranked.filter { it.second == best && best != Int.MAX_VALUE }.map { it.first }
    }
    val locators = matching.map { EditableFieldLocator(it.optString("id"), it.optString("bounds")) }
        .distinctBy { it.id to it.bounds }
    return locators.singleOrNull()
}

/** True only when the same identified editable node exposes the exact requested value. */
internal fun editableFieldValueMatches(snapshot: JSONObject, locator: EditableFieldLocator, expected: String): Boolean {
    if (expected.isBlank()) return false // Empty values are omitted by Android's accessibility tree.
    val editableNodes = mutableListOf<JSONObject>()
    fun collect(node: JSONObject) {
        if (node.optBoolean("editable")) editableNodes += node
        val children = node.optJSONArray("children") ?: return
        for (index in 0 until children.length()) children.optJSONObject(index)?.let(::collect)
    }
    collect(snapshot)

    val byId = if (locator.id.isBlank()) emptyList() else editableNodes.filter { it.optString("id") == locator.id }
    val exactBounds = if (locator.bounds.isBlank()) emptyList() else editableNodes.filter { it.optString("bounds") == locator.bounds }
    val identified = when {
        byId.isNotEmpty() && exactBounds.any { it in byId } -> exactBounds.filter { it in byId }
        byId.size == 1 -> byId
        exactBounds.size == 1 -> exactBounds
        else -> emptyList()
    }.distinct()
    val field = identified.singleOrNull() ?: return false
    return field.has("text") && field.optString("text") == expected
}

internal fun setTextPostconditionVerified(beforeSummary: String?, afterSnapshot: JSONObject, target: String, expected: String): Boolean {
    val before = beforeSummary?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return false
    val locator = editableFieldLocator(before, target) ?: return false
    return editableFieldValueMatches(afterSnapshot, locator, expected)
}

internal fun latestFreshScreenObservation(
    observations: List<String>,
    screenPrefix: String,
    failurePrefix: String,
): String? {
    val screenIndex = observations.indexOfLast { it.startsWith(screenPrefix) }
    val failureIndex = observations.indexOfLast { it.startsWith(failurePrefix) }
    if (failureIndex > screenIndex) return null
    return observations.getOrNull(screenIndex)?.removePrefix(screenPrefix)
}

private fun treeHasTruncationMarker(value: JSONObject): Boolean {
    if (value.optBoolean("truncated")) return true
    val children = value.optJSONArray("children") ?: return false
    for (index in 0 until children.length()) {
        val child = children.optJSONObject(index) ?: continue
        if (treeHasTruncationMarker(child)) return true
    }
    return false
}

private fun hasMoreDistinctVisibleText(tree: JSONObject, limit: Int): Boolean {
    val seen = linkedSetOf<String>()
    fun visit(node: JSONObject): Boolean {
        for (key in listOf("text", "description")) {
            val value = node.optString(key).trim()
            if (value.isNotBlank() && value != "null" && seen.add(value) && seen.size > limit) return true
        }
        val children = node.optJSONArray("children") ?: return false
        for (index in 0 until children.length()) {
            val child = children.optJSONObject(index) ?: continue
            if (visit(child)) return true
        }
        return false
    }
    return visit(tree)
}

internal fun snapshotTruncationMetadata(tree: JSONObject, controlLimit: Int, textLimit: Int): JSONObject {
    val controls = tree.optJSONArray("interactive_controls") ?: JSONArray()
    val visibleText = tree.optJSONArray("visible_text") ?: JSONArray()
    val prior = tree.optJSONObject("truncation")
    val controlsTruncated = controls.length() > controlLimit || (0 until controls.length()).any { index ->
        controls.optJSONObject(index)?.optBoolean("truncated") == true
    }
    val treeTruncated = treeHasTruncationMarker(tree) || prior?.optBoolean("tree") == true
    return JSONObject()
        .put("tree", treeTruncated)
        .put("interactive_controls", controlsTruncated || prior?.optBoolean("interactive_controls") == true)
        .put("visible_text", visibleText.length() > textLimit || hasMoreDistinctVisibleText(tree, textLimit) || prior?.optBoolean("visible_text") == true)
}

internal fun phoneActionIssue(result: ActionResult, action: String): String? =
    if (result.success) null else "$action failed: ${result.message}"

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
        val toolCallId: String? = null,
    )

    /* One device has one foreground UI. Serialize it so separate assistant sessions
       cannot concurrently press controls or consume each other's checkpoint. */
    private val turnMutex = Mutex()
    private var currentSessionId: String = "legacy"

    suspend fun run(
        prompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit = {},
        internalContinuation: Boolean = false,
        sessionId: String? = null,
    ): PhoneAgentResult = turnMutex.withLock {
        runLocked(prompt, settings, onProgress, internalContinuation, sessionId)
    }

    private suspend fun runLocked(
        prompt: String,
        settings: LocalModelSettings,
        onProgress: (PhoneAgentProgress) -> Unit,
        internalContinuation: Boolean,
        sessionId: String?,
    ): PhoneAgentResult {
        val cleanPrompt = prompt.trim()
        if (cleanPrompt.isBlank()) return PhoneAgentResult("Tell me what you want me to do.", false)
        currentSessionId = sessionId ?: repository.ensureActiveSession()

        if (!internalContinuation) repository.recordTurn("user", cleanPrompt, currentSessionId)
        val isConfirmation = CONFIRMATION.matches(cleanPrompt)
        val isDecline = DECLINE.matches(cleanPrompt)
        val isContinuation = internalContinuation || CONTINUATION.matches(cleanPrompt)
        val awaiting = repository.latestAwaitingAgentRun(currentSessionId)?.takeIf { (_, state) -> isFreshAwaiting(state) }
        val pending = awaiting?.second?.optJSONObject("pending_confirmation").pendingAction()

        if (isDecline && awaiting != null && pending != null) {
            repository.completeAgentRun(awaiting.first, "Confirmation declined by the user.", "cancelled")
            return finish("Okay, I will not do that.", false, taskId = awaiting.first)
        }

        val resumed = when {
            awaiting != null && resumesAwaitingRun(cleanPrompt, pending != null) -> awaiting
            isContinuation && awaiting == null -> repository.latestPausedAgentRun(currentSessionId) ?: repository.latestAgentRun(currentSessionId)
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
        val toolHistory = savedState?.optJSONArray("tool_history").toToolHistory().toMutableList()
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
            saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
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
                        toolHistory,
                        resumedPending,
                    )
                } else {
                    runWithinDeadline(taskPrompt, settings, onProgress, taskId, observations, attempts, toolHistory)
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
        toolHistory: MutableList<ModelToolExchange>,
        pending: PendingAction,
    ): PhoneAgentResult {
        val previousAttempts = attemptsFor(pending.action, pending.signature, attempts)
        if (previousAttempts >= attemptLimit(pending.action)) {
            return stopForNoProgress(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
        }

        attempts[pending.signature] = previousAttempts + 1
        // Store the attempt before executing. If Android kills this process or the
        // turn is cancelled during the action, a later continuation must verify
        // rather than blindly repeat a possibly completed external effect.
        saveCheckpoint(taskId, taskPrompt, observations, attempts, inFlight = pending, toolHistory = toolHistory)
        publishStatus(onProgress, "Running confirmed ${pending.action.replace('.', ' ')}…")
        currentCoroutineContext().ensureActive()
        executeAndObserve(
            action = pending.action,
            arguments = pending.arguments,
            observationLabel = "Confirmed action",
            observations = observations,
            toolHistory = toolHistory,
            toolCallId = pending.toolCallId,
            onProgress = onProgress,
        )
        saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)

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
            toolHistory,
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
        toolHistory: MutableList<ModelToolExchange>,
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
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory) { state ->
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
                    if (waitForPackage(launchedPackage, APP_TRANSITION_TIMEOUT_MS, observations)) {
                        addObservation(observations, "Verified foreground app: $label ($launchedPackage)")
                        return finish(direct.message, true, taskId = taskId)
                    }
                    addObservation(observations,
                        "App launch was requested for $label, but Android did not expose it as the foreground accessibility window. Inspect the current screen before retrying.")
                    publishStatus(onProgress, "I opened $label but couldn't verify its screen; checking what Android shows.")
                } else {
                    addObservation(observations, "Deterministic action failed: ${direct.message}")
                    direct.data.optString("requested").takeIf(String::isNotBlank)?.let { requested ->
                        recordToolExchange(
                            "apps.launch",
                            JSONObject().put("app", requested),
                            direct,
                            false,
                            direct.message,
                            observations,
                            toolHistory,
                            null,
                        )
                    }
                    publishStatus(onProgress, "Couldn't complete the direct phone action; checking the current screen. ${direct.message}")
                }
            }
        }

        val memory = repository.memoryContext(taskPrompt, currentSessionId)
        val spokenProgress = mutableSetOf<String>()
        var invalidPlans = 0
        var completionReviewed = false
        var unresolvedActionIssue = lastActionIssue(observations)
        repeat(MAX_ACTIONS) actionLoop@ { step ->
            currentCoroutineContext().ensureActive()
            refreshUiObservation(observations)
            publishStatus(onProgress, "Working · ${step + 1} verified action${if (step == 0) "" else "s"}")
            val answer = try {
                model.complete(settings, taskPrompt, memory, modelObservations(observations, attempts), toolHistory)
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
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                if (++invalidPlans < 2) return@actionLoop
                saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused", toolHistory = toolHistory)
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
                        toolHistory,
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
                    saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                    return@actionLoop
                }
                if (needsReply) {
                    saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "awaiting_user", replyType = replyType, toolHistory = toolHistory)
                }
                return finish(reply, observations.isNotEmpty(), needsReply = needsReply, replyType = replyType, taskId = taskId)
            }

            val action = structured.optString("action").trim()
            if (action.isBlank()) {
                addObservation(observations, "The model returned neither a reply nor an action. Return one valid JSON action plan.")
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                if (++invalidPlans < 2) return@actionLoop
                saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused", toolHistory = toolHistory)
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
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                return@actionLoop
            }
            if (action == "memory.update") {
                val id = arguments.optString("id").trim()
                val updated = if (id.isBlank()) false else repository.updateMemory(
                    id,
                    arguments.optString("text"),
                    arguments.optString("kind", "fact"),
                )
                addObservation(observations, if (updated) "Updated saved memory $id." else "Memory update failed: no matching saved memory or replacement text was empty.")
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                return@actionLoop
            }
            if (action == "memory.forget") {
                val id = arguments.optString("id").trim()
                val deleted = id.isNotBlank() && repository.forgetMemory(id)
                addObservation(observations, if (deleted) "Forgot saved memory $id." else "Memory deletion failed: no matching memory ID was found.")
                saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
                return@actionLoop
            }

            val signature = attemptSignature(action, arguments.toString(), screenFingerprint(observations))
            val previousAttempts = attemptsFor(action, signature, attempts)
            if (previousAttempts >= attemptLimit(action)) {
                return stopForNoProgress(taskId, taskPrompt, observations, attempts, unresolvedActionIssue, toolHistory)
            }
            val pending = PendingAction(
                action,
                arguments,
                signature,
                structured.optString("_tool_call_id").takeIf(String::isNotBlank),
            )
            if (executor.risk(action, arguments) != ActionRisk.Safe) {
                saveCheckpoint(
                    taskId,
                    taskPrompt,
                    observations,
                    attempts,
                    toolHistory,
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
            saveCheckpoint(taskId, taskPrompt, observations, attempts, inFlight = pending, toolHistory = toolHistory)
            publishStatus(onProgress, "Running ${action.replace('.', ' ')}…")
            currentCoroutineContext().ensureActive()
            val actionIssue = executeAndObserve(
                action,
                arguments,
                "Step ${step + 1}",
                observations,
                toolHistory,
                pending.toolCallId,
                onProgress,
            )
            if (actionIssue?.startsWith(ACCESSIBILITY_REQUIRED_PREFIX) == true) {
                saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused", toolHistory = toolHistory)
                return finish(
                    actionIssue,
                    false,
                    checkpointed = true,
                    taskId = taskId,
                )
            }
            if (actionIssue != null) unresolvedActionIssue = actionIssue
            else if (action !in setOf("apps.list", "ui.snapshot")) unresolvedActionIssue = null
            saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory)
        }
        saveCheckpoint(taskId, taskPrompt, observations, attempts, status = "paused", toolHistory = toolHistory)
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
        toolHistory: MutableList<ModelToolExchange>,
        toolCallId: String? = null,
        onProgress: (PhoneAgentProgress) -> Unit = {},
    ): String? {
        val beforeTree = latestScreenTree(observations)
        val hasFreshAccessibilityTree = beforeTree != null &&
            observations.lastOrNull { it.startsWith(SCREEN_READ_FAILURE_PREFIX) } == null
        val hasFreshScreenshot = observations.indexOfLast { it.startsWith(SCREENSHOT_OBSERVATION_PREFIX) } >
            observations.indexOfLast { it.startsWith(SCREEN_READ_FAILURE_PREFIX) }
        val hasGrounding = hasFreshAccessibilityTree ||
            (action in setOf("ui.tap", "ui.swipe") && hasFreshScreenshot)
        if (action in UI_TARGET_ACTIONS && (!hasGrounding || !HyuskAccessibilityService.connected.value)) {
            val issue = if (!HyuskAccessibilityService.connected.value) {
                "$ACCESSIBILITY_REQUIRED_PREFIX screen reading and taps require the Android Accessibility service. Turn on Settings → Accessibility → Hyusk, then retry."
            } else if (hasFreshScreenshot && action !in setOf("ui.tap", "ui.swipe")) {
                "$action was not attempted because Android has no current semantic accessibility tree. The latest screenshot can ground a coordinate tap or swipe, but not a named-control action."
            } else observations.lastOrNull { it.startsWith(SCREEN_READ_FAILURE_PREFIX) }
                ?.removePrefix(SCREEN_READ_FAILURE_PREFIX)
                ?: "$action was not attempted because no current screen observation is available"
            addObservation(observations, "$observationLabel: $issue")
            recordToolExchange(action, arguments, ActionResult(false, issue), false, issue, observations, toolHistory, toolCallId)
            publishStatus(onProgress, "I can't see the phone's current controls, so I didn't guess. Check Hyusk Accessibility and try again.")
            return issue
        }
        val result = if (action == "ui.screenshot") {
            val service = HyuskAccessibilityService.instance
            if (service == null) {
                ActionResult(false, "$ACCESSIBILITY_REQUIRED_PREFIX screenshot capture requires the Android Accessibility service to be enabled")
            } else {
                runCatching {
                    val data = service.screenshotText()
                    ActionResult(true, "Captured the screen and read visible text locally", data)
                }.getOrElse { failure ->
                    ActionResult(false, failure.message ?: "Android screenshot/OCR failed")
                }
            }
        } else executor.execute(action, arguments)
        if (action == "ui.screenshot" && result.success) {
            observations.removeAll { it.startsWith(SCREEN_READ_FAILURE_PREFIX) }
            addObservation(observations, "$SCREENSHOT_OBSERVATION_PREFIX${result.data}")
        }
        var screenChanged: Boolean? = null
        if (action in UI_CHANGING_ACTIONS && result.success) {
            screenChanged = when {
                action == "apps.launch" && result.data.optString("package").isNotBlank() -> {
                    val packageName = result.data.optString("package")
                    val verified = waitForPackage(packageName, APP_TRANSITION_TIMEOUT_MS, observations)
                    if (verified) addObservation(observations, "Verified foreground app: ${result.data.optString("label", packageName)} ($packageName)")
                    verified
                }
                action == "ui.set_text" -> waitForEditableFieldValue(
                    beforeTree,
                    arguments.optString("target"),
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
            screenChanged == false -> "was accepted by Android; no accessibility-tree transition was observed (this alone does not mean it failed)"
            else -> "succeeded"
        }
        addObservation(
            observations,
            "$observationLabel: $action $outcome: ${result.message}. Data: ${result.data}",
        )
        val issue = phoneActionIssue(result, action)
        if (issue != null) {
            publishStatus(onProgress, "Couldn't verify $action; checking another route. ${result.message}")
        }
        recordToolExchange(action, arguments, result, screenChanged, issue, observations, toolHistory, toolCallId)
        return issue
    }

    private fun recordToolExchange(
        action: String,
        arguments: JSONObject,
        result: ActionResult,
        screenChanged: Boolean?,
        issue: String?,
        observations: List<String>,
        toolHistory: MutableList<ModelToolExchange>,
        toolCallId: String?,
    ) {
        val recordedData = when (action) {
            "ui.snapshot" -> result.data.optJSONObject("tree")?.let(::summarizeScreen) ?: JSONObject()
            "apps.list" -> JSONObject().put("apps", result.data.optJSONArray("apps")?.let { apps ->
                JSONArray().apply { for (index in 0 until minOf(apps.length(), 40)) put(apps.opt(index)) }
            } ?: JSONArray())
            else -> result.data
        }
        val payload = JSONObject()
            .put("success", result.success)
            .put("verification", when {
                !result.success -> "failed"
                action == "ui.set_text" && screenChanged == true -> "postcondition_verified"
                screenChanged == true -> "screen_changed"
                screenChanged == false -> "accepted_unverified"
                else -> "accepted_by_android"
            })
            .put("message", result.message)
            .put("data", recordedData)
        issue?.let { payload.put("issue", it) }
        latestScreenTree(observations)?.let { raw ->
            runCatching { summarizeScreen(JSONObject(raw)) }.getOrNull()?.let { payload.put("current_screen", it) }
        }
        toolHistory += ModelToolExchange(
            id = toolCallId ?: "hyusk_${UUID.randomUUID()}",
            action = action,
            actionCallJson = JSONObject().put("action", action).put("arguments", arguments).toString(),
            resultJson = payload.toString(),
        )
        while (toolHistory.size > MAX_TOOL_HISTORY) toolHistory.removeAt(0)
    }

    private fun summarizeScreen(tree: JSONObject): JSONObject {
        val controls = JSONArray()
        tree.optJSONArray("interactive_controls")?.let { source ->
            for (index in 0 until minOf(source.length(), MAX_SCREEN_CONTROLS)) {
                source.optJSONObject(index)?.let(controls::put)
            }
        }
        val visibleText = JSONArray()
        collectScreenText(tree, visibleText, linkedSetOf())
        return JSONObject()
            .put("package", tree.optString("package"))
            .put("truncation", snapshotTruncationMetadata(tree, MAX_SCREEN_CONTROLS, MAX_SCREEN_TEXT_ITEMS))
            .put("interactive_controls", controls)
            .put("visible_text", visibleText)
    }

    private fun collectScreenText(value: JSONObject, output: JSONArray, seen: MutableSet<String>) {
        if (output.length() >= MAX_SCREEN_TEXT_ITEMS) return
        listOf("text", "description").forEach { key ->
            value.optString(key).trim().takeIf { it.isNotBlank() && it != "null" && seen.add(it) }
                ?.let(output::put)
        }
        val children = value.optJSONArray("children") ?: return
        for (index in 0 until children.length()) {
            children.optJSONObject(index)?.let { collectScreenText(it, output, seen) }
            if (output.length() >= MAX_SCREEN_TEXT_ITEMS) return
        }
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
        if (!screen.success) {
            val detail = if (!HyuskAccessibilityService.connected.value) {
                "$ACCESSIBILITY_REQUIRED_PREFIX the Android service is not connected. Open Settings → Accessibility → Hyusk and check its status."
            } else "Android Accessibility is connected, but it returned no active app window: ${screen.message}. The previous screen tree is stale."
            markScreenReadFailure(observations, detail)
            return
        }
        val tree = screen.data.optJSONObject("tree")
        if (tree == null) {
            markScreenReadFailure(observations, "Android returned a successful snapshot result without a screen tree. No current controls are available.")
            return
        }
        addScreenObservation(observations, tree)
    }

    private fun markScreenReadFailure(observations: MutableList<String>, detail: String) {
        observations.removeAll { it.startsWith(SCREEN_OBSERVATION_PREFIX) || it.startsWith(SCREEN_READ_FAILURE_PREFIX) }
        addObservation(observations, "$SCREEN_READ_FAILURE_PREFIX${detail.take(700)}")
    }

    private fun addScreenObservation(observations: MutableList<String>, tree: JSONObject) {
        observations.removeAll { it.startsWith(SCREEN_READ_FAILURE_PREFIX) }
        val summary = summarizeScreen(tree)
        val observation = "$SCREEN_OBSERVATION_PREFIX$summary"
        if (observation != observations.lastOrNull { it.startsWith(SCREEN_OBSERVATION_PREFIX) }) {
            addObservation(observations, observation)
        }
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
            if (!screen.success) {
                markScreenReadFailure(observations, "Could not read the screen after the action: ${screen.message}")
                continue
            }

            val rawTree = screen.data.optJSONObject("tree")
            if (rawTree == null) {
                markScreenReadFailure(observations, "Android returned no accessibility tree after the action.")
                continue
            }
            addScreenObservation(observations, rawTree)
            val tree = summarizeScreen(rawTree).toString()
            if (tree.isBlank()) continue
            if (beforeTree != null && tree != beforeTree) changed = true
            if (changed && tree == lastTree) {
                stableSamples++
                if (stableSamples >= 1) {
                    addScreenObservation(observations, rawTree)
                    return true
                }
            } else {
                stableSamples = 0
            }
            lastTree = tree
        }

        // Keep the latest observation even when Android exposed no detectable
        // screen change; the model must not treat dispatch success as proof of progress.
        // Only a successfully read tree can become current evidence. Failures above
        // deliberately evict the old tree instead of allowing stale targets to survive.
        return changed
    }

    private fun latestScreenTree(observations: List<String>): String? {
        val raw = latestFreshScreenObservation(
            observations,
            SCREEN_OBSERVATION_PREFIX,
            SCREEN_READ_FAILURE_PREFIX,
        ) ?: return null
        return runCatching { summarizeScreen(JSONObject(raw)).toString() }.getOrDefault(raw)
    }

    private suspend fun waitForPackage(packageName: String, timeoutMillis: Long, observations: MutableList<String>): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMillis
        var lastFailure: String? = null
        while (System.currentTimeMillis() < deadline) {
            currentCoroutineContext().ensureActive()
            val snapshot = executor.execute("ui.snapshot", JSONObject())
            val tree = snapshot.data.optJSONObject("tree")
            if (snapshot.success && tree != null) {
                addScreenObservation(observations, tree)
                if (tree.optString("package") == packageName) return true
            } else {
                lastFailure = snapshot.message
                markScreenReadFailure(observations, "Could not verify the opened app: ${snapshot.message}")
            }
            delay(UI_POLL_INTERVAL_MILLIS)
        }
        if (lastFailure == null) {
            markScreenReadFailure(observations, "Could not verify the opened app because Android did not expose its accessibility window before timeout.")
        }
        return false
    }

    private suspend fun waitForEditableFieldValue(
        beforeTree: String?,
        target: String,
        expected: String,
        timeoutMillis: Long,
        observations: MutableList<String>,
    ): Boolean {
        if (expected.isBlank() || beforeTree.isNullOrBlank()) return false
        val deadline = System.currentTimeMillis() + timeoutMillis
        var latestTree: String? = null
        while (System.currentTimeMillis() < deadline) {
            currentCoroutineContext().ensureActive()
            val snapshot = executor.execute("ui.snapshot", JSONObject())
            val rawTree = snapshot.data.optJSONObject("tree")
            val tree = rawTree?.let(::summarizeScreen)?.toString()
            if (snapshot.success && !tree.isNullOrBlank()) {
                rawTree?.let { addScreenObservation(observations, it) }
                latestTree = tree
                if (setTextPostconditionVerified(beforeTree, rawTree, target, expected)) {
                    if (tree != beforeTree) addObservation(observations, "Current accessibility tree: $tree")
                    return true
                }
            } else {
                markScreenReadFailure(observations, "Could not verify entered text: ${snapshot.message}")
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
        toolHistory: List<ModelToolExchange> = emptyList(),
        status: String = "active",
        pending: PendingAction? = null,
        inFlight: PendingAction? = null,
        replyType: String = "none",
        mutate: (JSONObject) -> Unit = {},
    ) {
        val state = checkpoint(prompt, observations, attempts, toolHistory)
        pending?.let { state.put("pending_confirmation", it.toJson()) }
        inFlight?.let { state.put("in_flight", it.toJson()) }
        if (status == "awaiting_user") {
            state.put("awaiting_user", JSONObject()
                .put("reply_type", replyType)
                .put("created_at", System.currentTimeMillis()))
        }
        mutate(state)
        state.put("session_id", currentSessionId)
        repository.saveAgentRun(taskId, state, status, currentSessionId)
    }

    private suspend fun stopForNoProgress(
        taskId: String,
        taskPrompt: String,
        observations: List<String>,
        attempts: Map<String, Int>,
        unresolvedIssue: String? = null,
        toolHistory: List<ModelToolExchange> = emptyList(),
    ): PhoneAgentResult {
        saveCheckpoint(taskId, taskPrompt, observations, attempts, toolHistory = toolHistory, status = "paused")
        val issue = unresolvedIssue ?: lastActionIssue(observations)
        return finish(
            if (issue == null) "I couldn't make further progress with the current route. The last action was accepted, but Android exposed no screen change to verify; I have not treated that as a failure. Your progress is saved, and I can try a different route if you say continue."
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
        if (!autoContinue) repository.recordTurn("assistant", message, currentSessionId)
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
            "Attempt count $count: ${action.take(MAX_ACTION_LEDGER_ENTRY_CHARS)}. Check recent tool results and the current screen before deciding whether to retry."
        }
        return observations + "Persistent attempt ledger (not a success list):\n$ledger"
    }

    private fun checkpoint(
        prompt: String,
        observations: List<String>,
        attempts: Map<String, Int>,
        toolHistory: List<ModelToolExchange>,
    ): JSONObject = JSONObject()
        .put("prompt", prompt)
        .put("session_id", currentSessionId)
        .put("completed_steps", attempts.values.sum())
        .put("observations", JSONArray().also { array -> observations.forEach(array::put) })
        .put("attempts", JSONObject().also { objectValue -> attempts.forEach { (key, count) -> objectValue.put(key, count) } })
        .put("tool_history", JSONArray().also { array -> toolHistory.forEach { array.put(it.toJson()) } })

    private fun PendingAction.toJson(): JSONObject = JSONObject()
        .put("action", action)
        .put("arguments", arguments)
        .put("signature", signature)
        .put("tool_call_id", toolCallId)

    private fun JSONObject?.pendingAction(): PendingAction? {
        val value = this ?: return null
        val action = value.optString("action").trim()
        if (action.isBlank()) return null
        val arguments = value.optJSONObject("arguments") ?: JSONObject()
        val signature = value.optString("signature").ifBlank { "$action:$arguments" }
        return PendingAction(action, arguments, signature, value.optString("tool_call_id").takeIf(String::isNotBlank))
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

    private fun JSONArray?.toToolHistory(): List<ModelToolExchange> = buildList {
        val array = this@toToolHistory ?: return@buildList
        for (index in 0 until array.length()) {
            array.optJSONObject(index)?.let(ModelToolExchange::fromJson)?.let(::add)
        }
    }

    private fun normalizedReplyType(value: String, needsReply: Boolean): String = when (value.lowercase()) {
        "question", "choice", "confirmation" -> value.lowercase()
        else -> if (needsReply) "question" else "none"
    }

    private fun isFreshAwaiting(state: JSONObject): Boolean {
        val createdAt = state.optJSONObject("awaiting_user")?.optLong("created_at", 0L) ?: 0L
        return createdAt > 0 && System.currentTimeMillis() - createdAt <= AWAITING_REPLY_WINDOW_MILLIS
    }

    private fun attemptLimit(action: String): Int = attemptLimitFor(action)

    internal companion object {
        val CONFIRMATION = Regex("(?i)^(yes|confirm|allow|do it|go ahead)[.! ]*$")
        val DECLINE = Regex("(?i)^(no|cancel|stop|don't|do not)[.! ]*$")
        val CONTINUATION = Regex("(?i)^(continue|resume|keep going)[.! ]*$")
        fun resumesAwaitingRun(input: String, hasPendingConfirmation: Boolean): Boolean =
            if (hasPendingConfirmation) CONFIRMATION.matches(input) else CONTINUATION.matches(input)

        internal fun attemptSignature(action: String, arguments: String, screen: String): String =
            if (action in SINGLE_ATTEMPT_ACTIONS) "$action|screen=side-effect"
            else "$action:$arguments|screen=$screen"

        internal fun attemptsFor(
            action: String,
            signature: String,
            attempts: Map<String, Int>,
        ): Int {
            if (action !in SINGLE_ATTEMPT_ACTIONS) return attempts.getOrDefault(signature, 0)
            return attempts.entries
                .filter { (key, _) -> key == signature ||
                    (key.startsWith("$action:") && key.contains(SCREEN_SIGNATURE_SEPARATOR)) }
                .maxOfOrNull { it.value } ?: 0
        }

        internal fun attemptLimitFor(action: String): Int = when {
            action in SINGLE_ATTEMPT_ACTIONS -> 1
            action in REPEATABLE_ACTIONS -> 4
            else -> 2
        }

        const val MAX_ACTIONS = 64
        const val TURN_TIMEOUT_MILLIS = 600_000L
        const val AWAITING_REPLY_WINDOW_MILLIS = 15 * 60_000L
        const val MAX_OBSERVATIONS = 6
        const val MAX_OBSERVATION_CHARS = 8_000
        const val MAX_ACTION_LEDGER_ENTRIES = 24
        const val MAX_ACTION_LEDGER_ENTRY_CHARS = 320
        const val MAX_TOOL_HISTORY = 8
        const val MAX_SCREEN_CONTROLS = 24
        const val MAX_SCREEN_TEXT_ITEMS = 24
        const val SCREEN_OBSERVATION_PREFIX = "Current accessibility tree: "
        const val SCREENSHOT_OBSERVATION_PREFIX = "Current screenshot OCR: "
        const val SCREEN_READ_FAILURE_PREFIX = "Current screen read failed: "
        const val ACCESSIBILITY_REQUIRED_PREFIX = "Hyusk Accessibility is off:"
        const val SCREEN_SIGNATURE_SEPARATOR = "|screen="
        const val SCREEN_FINGERPRINT_BYTES = 8
        const val UI_POLL_INTERVAL_MILLIS = 120L
        const val UI_TRANSITION_TIMEOUT_MS = 1_400L
        const val APP_TRANSITION_TIMEOUT_MS = 2_400L
        val REPEATABLE_ACTIONS = setOf(
            "ui.snapshot", "ui.screenshot", "ui.scroll", "ui.swipe", "ui.tap", "media.play", "media.pause", "media.status",
        )
        val SINGLE_ATTEMPT_ACTIONS = setOf("volume.up", "volume.down", "media.next", "media.previous")
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
