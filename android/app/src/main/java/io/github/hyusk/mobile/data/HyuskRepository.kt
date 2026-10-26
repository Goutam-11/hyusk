package io.github.hyusk.mobile.data

import android.content.Context
import io.github.hyusk.mobile.security.EncryptedPayload
import java.util.UUID
import androidx.room.withTransaction
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import org.json.JSONObject

data class MemorySummary(
    val id: String,
    val kind: String,
    val content: String,
    val scope: String,
    val originDevice: String,
    val createdAt: Long,
)

data class RunSummary(
    val id: String,
    val label: String,
    val prompt: String,
    val result: String,
    val status: String,
    val completedSteps: Int,
    val updatedAt: Long,
    val sessionId: String,
)

data class SessionSummary(val id: String, val title: String, val active: Boolean, val updatedAt: Long)
data class ConversationTurn(val id: Long, val role: String, val content: String, val createdAt: Long)

/** Small local data boundary for the screens that manage synced records. */
class HyuskRepository(context: Context) : AutoCloseable {
    private val database = HyuskDatabase.create(context.applicationContext)
    private val payload = EncryptedPayload(context.applicationContext)

    val workflows: Flow<List<WorkflowEntity>> = database.workflows().observe()
    val sessions: Flow<List<SessionSummary>> = database.agentSessions().observe().map { records ->
        records.map { SessionSummary(it.id, it.title, it.active, it.updatedAt) }
    }
    val memories: Flow<List<MemorySummary>> = database.memories().observe().map { records ->
        records.mapNotNull { memory ->
            runCatching {
                MemorySummary(
                    id = memory.id,
                    kind = memory.kind,
                    content = payload.decrypt(memory.encryptedPayload),
                    scope = memory.scope,
                    originDevice = memory.originDevice,
                    createdAt = memory.createdAt,
                )
            }.getOrNull()
        }
    }
    val runs: Flow<List<RunSummary>> = database.agentRuns().observe().map { records ->
        records.mapNotNull { run ->
            runCatching {
                val state = JSONObject(payload.decrypt(run.encryptedState))
                RunSummary(
                    id = run.id,
                    label = state.optString("label").ifBlank {
                        if (state.optString("kind") == "workflow") "Workflow" else "Agent task"
                    },
                    prompt = state.optString("prompt"),
                    result = state.optString("result"),
                    status = run.status,
                    completedSteps = state.optInt("completed_steps", 0),
                    updatedAt = run.updatedAt,
                    sessionId = run.sessionId,
                )
            }.getOrNull()
        }
    }

    fun conversation(sessionId: String): Flow<List<ConversationTurn>> = database.agentTurns().observe(sessionId).map { records ->
        records.mapNotNull { turn ->
            runCatching {
                ConversationTurn(turn.id, turn.role, payload.decrypt(turn.encryptedContent), turn.createdAt)
            }.getOrNull()
        }
    }

    suspend fun ensureActiveSession(): String = database.withTransaction {
        database.agentSessions().active()?.id ?: run {
            val now = System.currentTimeMillis()
            val id = UUID.randomUUID().toString()
            database.agentSessions().deactivateAll()
            database.agentSessions().insert(AgentSessionEntity(id, "New conversation", true, now, now))
            id
        }
    }

    suspend fun createSession(): String {
        val now = System.currentTimeMillis()
        val id = UUID.randomUUID().toString()
        database.withTransaction {
            database.agentSessions().deactivateAll()
            database.agentSessions().insert(AgentSessionEntity(id, "New conversation", true, now, now))
        }
        return id
    }

    suspend fun activateSession(id: String): Boolean = database.withTransaction {
        val session = database.agentSessions().get(id) ?: return@withTransaction false
        database.agentSessions().deactivateAll()
        database.agentSessions().update(session.copy(active = true, updatedAt = System.currentTimeMillis()))
        true
    }

    suspend fun recordTurn(role: String, content: String, sessionId: String? = null) {
        if (content.isBlank()) return
        val id = sessionId ?: ensureActiveSession()
        val clean = content.trim()
        database.agentTurns().insert(AgentTurnEntity(role = role, encryptedContent = payload.encrypt(clean), sessionId = id))
        database.agentSessions().get(id)?.let { session ->
            val title = if (role == "user" && session.title == "New conversation") clean.replace(Regex("\\s+"), " ").take(64) else session.title
            database.agentSessions().update(session.copy(title = title, updatedAt = System.currentTimeMillis()))
        }
        database.agentTurns().trimSession(id, 300)
    }

    suspend fun remember(content: String, kind: String = "fact"): String {
        val clean = content.trim()
        require(clean.isNotBlank()) { "Memory cannot be empty" }
        database.memories().latest(100).firstOrNull { memory ->
            runCatching { payload.decrypt(memory.encryptedPayload).equals(clean, ignoreCase = true) }
                .getOrDefault(false)
        }?.let { return it.id }
        val id = UUID.randomUUID().toString()
        database.memories().insert(MemoryEntity(
            id = id,
            kind = kind,
            encryptedPayload = payload.encrypt(clean),
            originDevice = "android",
        ))
        return id
    }

    suspend fun updateMemory(id: String, content: String, kind: String = "fact"): Boolean {
        val clean = content.trim()
        require(clean.isNotBlank()) { "Memory cannot be empty" }
        val current = database.memories().get(id) ?: return false
        if (current.deleted) return false
        database.memories().update(current.copy(
            kind = kind.trim().ifBlank { current.kind },
            encryptedPayload = payload.encrypt(clean),
        ))
        return true
    }

    suspend fun forgetMemory(id: String): Boolean {
        val current = database.memories().get(id) ?: return false
        if (current.deleted) return false
        database.memories().update(current.copy(deleted = true))
        return true
    }

    suspend fun memoryContext(query: String): String {
        val sessionId = ensureActiveSession()
        return memoryContext(query, sessionId)
    }

    suspend fun memoryContext(query: String, sessionId: String): String {
        var skippedCurrentRequest = false
        val queryWords = words(query)
        val allTurns = database.agentTurns().latest(sessionId, 300).mapNotNull { turn ->
            runCatching { turn.role to payload.decrypt(turn.encryptedContent) }.getOrNull()
        }.filterNot { (role, content) ->
            // PhoneAgent records the current request before building context so
            // it survives cancellation. Avoid sending that same request twice.
            val isCurrent = !skippedCurrentRequest && role == "user" && content.trim() == query.trim()
            if (isCurrent) skippedCurrentRequest = true
            isCurrent
        }
        val recentTurns = allTurns.take(14)
        val relevantOlderTurns = allTurns.drop(14)
            .map { turn -> turn to words(turn.second).count(queryWords::contains) }
            .filter { (_, score) -> score > 0 }
            .sortedByDescending { (_, score) -> score }
            .take(6)
            .map { (turn, _) -> turn }
            .asReversed()
            .map { (role, content) -> "$role: $content" }
        val recentConversation = recentTurns.asReversed().map { (role, content) -> "$role: $content" }
        val facts = database.memories().latest(100).mapNotNull { memory ->
            runCatching { memory to payload.decrypt(memory.encryptedPayload) }.getOrNull()
        }.sortedByDescending { (_, text) -> words(text).count(queryWords::contains) }
            .take(8)
            .map { (memory, text) -> "[id=${memory.id}] ${memory.kind}: $text" }
        return buildString {
            if (facts.isNotEmpty()) appendLine("Saved memory:\n${facts.joinToString("\n")}")
            if (relevantOlderTurns.isNotEmpty()) appendLine("Earlier relevant conversation:\n${relevantOlderTurns.joinToString("\n")}")
            if (recentConversation.isNotEmpty()) appendLine("Recent conversation:\n${recentConversation.joinToString("\n")}")
        }.trim().take(12_000)
    }

    suspend fun recalledMemories(limit: Int = 12): List<String> = database.memories().latest(limit).mapNotNull {
        runCatching { payload.decrypt(it.encryptedPayload) }.getOrNull()
    }

    suspend fun forgetAllMemories() {
        database.memories().deleteAll()
    }

    suspend fun saveAgentRun(id: String, state: JSONObject, status: String = "active", sessionId: String = state.optString("session_id", "legacy")) {
        database.agentRuns().save(AgentRunEntity(id, payload.encrypt(state.toString()), status, sessionId = sessionId))
    }

    suspend fun latestAgentRun(sessionId: String): Pair<String, JSONObject>? = database.agentRuns().latestActive(sessionId)?.let { run ->
        runCatching { run.id to JSONObject(payload.decrypt(run.encryptedState)) }.getOrNull()
    }

    /** A model question or confirmation may survive an assistant UI/session restart. */
    suspend fun latestAwaitingAgentRun(sessionId: String): Pair<String, JSONObject>? =
        database.agentRuns().latestAwaitingUser(sessionId)?.let { run ->
            runCatching { run.id to JSONObject(payload.decrypt(run.encryptedState)) }.getOrNull()
        }

    /** Paused tasks are resumable only after the user explicitly says continue. */
    suspend fun latestPausedAgentRun(sessionId: String): Pair<String, JSONObject>? =
        database.agentRuns().latestPaused(sessionId)?.let { run ->
            runCatching { run.id to JSONObject(payload.decrypt(run.encryptedState)) }.getOrNull()
        }

    suspend fun markStaleRunsInterrupted() {
        database.agentRuns().markStaleInterrupted(System.currentTimeMillis() - 12 * 60_000L)
    }

    suspend fun pauseOrphanedActiveRuns() {
        database.agentRuns().pauseOrphanedActiveRuns()
    }

    suspend fun setAgentRunStatus(id: String, status: String) {
        database.agentRuns().setStatus(id, status)
    }

    suspend fun completeAgentRun(id: String, result: String, status: String = "completed", completedSteps: Int? = null) {
        val dao = database.agentRuns()
        val current = dao.get(id) ?: return
        val state = runCatching { JSONObject(payload.decrypt(current.encryptedState)) }
            .getOrElse { JSONObject() }
            .put("result", result.trim())
            .put("completed_at", System.currentTimeMillis())
        completedSteps?.let { state.put("completed_steps", it.coerceAtLeast(0)) }
        dao.save(current.copy(
            encryptedState = payload.encrypt(state.toString()),
            status = status,
            updatedAt = System.currentTimeMillis(),
        ))
    }

    private fun words(value: String): Set<String> = value.lowercase().split(Regex("[^a-z0-9]+"))
        .filter { it.length > 2 }.toSet()

    suspend fun saveWorkflow(
        id: String?,
        name: String,
        definition: String,
        scope: String,
        originDevice: String,
        enabled: Boolean,
    ): String {
        val cleanName = name.trim()
        val cleanDefinition = definition.trim()
        val cleanScope = scope.trim().ifBlank { "global" }
        val cleanDevice = originDevice.trim().ifBlank { "this-device" }
        require(cleanName.isNotBlank()) { "Workflow name is required" }
        require(cleanDefinition.isNotBlank()) { "Workflow definition is required" }

        val dao = database.workflows()
        val existing = id?.let { dao.get(it) }
        val record = if (existing == null) {
            WorkflowEntity(
                id = id ?: UUID.randomUUID().toString(),
                name = cleanName,
                encryptedDefinition = payload.encrypt(cleanDefinition),
                scope = cleanScope,
                originDevice = cleanDevice,
                enabled = enabled,
            )
        } else {
            existing.copy(
                name = cleanName,
                encryptedDefinition = payload.encrypt(cleanDefinition),
                scope = cleanScope,
                originDevice = cleanDevice,
                enabled = enabled,
                deleted = false,
                updatedAt = System.currentTimeMillis(),
            )
        }
        if (existing == null) dao.insert(record) else dao.update(record)
        return record.id
    }

    suspend fun decryptWorkflowDefinition(workflow: WorkflowEntity): String =
        payload.decrypt(workflow.encryptedDefinition)

    /** Resolve an enabled local workflow without involving a model provider. */
    suspend fun workflowForCommand(command: String): WorkflowEntity? {
        val candidate = normalizedWorkflowName(command)
        if (candidate.isBlank()) return null
        return database.workflows().enabled().firstOrNull { workflow ->
            if (normalizedWorkflowName(workflow.name) == candidate) return@firstOrNull true
            runCatching {
                val root = JSONObject(payload.decrypt(workflow.encryptedDefinition))
                val triggers = root.optJSONArray("voice_triggers") ?: return@runCatching false
                (0 until triggers.length()).any { index ->
                    normalizedWorkflowName(triggers.optString(index)) == candidate
                }
            }.getOrDefault(false)
        }
    }

    suspend fun deleteWorkflow(id: String) {
        database.workflows().get(id)?.let { workflow ->
            database.workflows().update(workflow.copy(deleted = true, updatedAt = System.currentTimeMillis()))
        }
    }

    override fun close() {
        database.close()
    }

    private fun normalizedWorkflowName(value: String): String {
        var clean = value.lowercase()
            .replace(Regex("[^a-z0-9]+"), " ")
            .trim()
            .replace(Regex("\\s+"), " ")
        val prefixes = listOf(
            "run workflow ", "run shortcut ", "execute workflow ", "start workflow ",
            "run ", "execute ", "start ", "shortcut ",
        )
        prefixes.firstOrNull(clean::startsWith)?.let { clean = clean.removePrefix(it).trim() }
        clean = clean.removePrefix("my ").trim()
        clean = clean.removeSuffix(" workflow").removeSuffix(" shortcut").trim()
        return clean
    }
}
