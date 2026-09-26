package io.github.hyusk.mobile.automation

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import kotlinx.coroutines.delay

/** Portable, UI-friendly workflow definition. Version 1 step arrays are read transparently. */
data class WorkflowDefinition(
    val version: Int = 2,
    val nodes: List<WorkflowNode> = emptyList(),
    /** Optional phrases that invoke this workflow without asking the model. */
    val voiceTriggers: List<String> = emptyList(),
) {
    fun validate() {
        require(nodes.isNotEmpty()) { "Add at least one action" }
        nodes.forEachIndexed { index, node ->
            require(node.action in WorkflowNode.SUPPORTED_ACTIONS) {
                "Action ${index + 1} is not supported: ${node.action}"
            }
            when (node.action) {
                "apps.launch" -> require(node.arguments.optString("package").isNotBlank() || node.arguments.optString("app").isNotBlank()) { "Choose an app for action ${index + 1}" }
                "url.open" -> require(node.arguments.optString("url").isNotBlank()) { "Enter a link for action ${index + 1}" }
                "web.search" -> require(node.arguments.optString("query").isNotBlank()) { "Enter a search for action ${index + 1}" }
                "timer.create" -> require(node.arguments.optInt("seconds") in 1..86_400) { "Timer ${index + 1} must be between 1 second and 24 hours" }
                "workflow.delay" -> require(node.arguments.optLong("milliseconds") in 100..86_400_000) { "Wait ${index + 1} must be between 0.1 seconds and 24 hours" }
            }
        }
    }

    fun toJson(): String {
        val nodesJson = JSONArray()
        nodes.forEachIndexed { index, node ->
            nodesJson.put(JSONObject()
                .put("id", node.id)
                .put("action", node.action)
                .put("target", node.target)
                .put("arguments", node.arguments)
                .put("continue_on_error", node.continueOnError)
                .put("wait_after_ms", node.waitAfterMillis))
        }
        val edges = JSONArray()
        nodes.zipWithNext().forEach { (from, to) ->
            edges.put(JSONObject().put("from", from.id).put("to", to.id).put("condition", "success"))
        }
        return JSONObject()
            .put("version", version)
            .put("voice_triggers", JSONArray(voiceTriggers.map(String::trim).filter(String::isNotBlank).distinct()))
            .put("nodes", nodesJson)
            .put("edges", edges)
            .toString()
    }

    companion object {
        fun parse(raw: String): WorkflowDefinition {
            val root = JSONObject(raw)
            val source = root.optJSONArray("nodes") ?: root.optJSONArray("steps") ?: JSONArray()
            val nodes = buildList {
                for (index in 0 until source.length()) {
                    val item = source.optJSONObject(index) ?: continue
                    add(WorkflowNode(
                        id = item.optString("id").ifBlank { UUID.randomUUID().toString() },
                        action = item.optString("action"),
                        target = item.optString("target", "phone"),
                        arguments = item.optJSONObject("arguments") ?: JSONObject(),
                        continueOnError = item.optBoolean("continue_on_error", false),
                        waitAfterMillis = item.optLong("wait_after_ms", 350).coerceIn(0, 30_000),
                    ))
                }
            }
            val triggers = root.optJSONArray("voice_triggers")?.let { source ->
                buildList {
                    for (index in 0 until source.length()) {
                        source.optString(index).trim().takeIf(String::isNotBlank)?.let(::add)
                    }
                }
            }.orEmpty()
            return WorkflowDefinition(nodes = nodes, voiceTriggers = triggers)
        }
    }
}

data class WorkflowNode(
    val id: String = UUID.randomUUID().toString(),
    val action: String,
    val target: String = "phone",
    val arguments: JSONObject = JSONObject(),
    val continueOnError: Boolean = false,
    val waitAfterMillis: Long = 350,
) {
    fun title(): String = when (action) {
        "apps.launch" -> "Open ${arguments.optString("label").ifBlank { arguments.optString("app", "app") }}"
        "url.open" -> "Open website"
        "web.search" -> "Search the web"
        "timer.create" -> "Start timer"
        "workflow.delay" -> "Wait"
        "media.play_pause" -> "Play or pause media"
        "system.home" -> "Go Home"
        "system.notifications" -> "Open notifications"
        else -> action.replace('.', ' ').replaceFirstChar(Char::uppercase)
    }

    fun summary(): String = when (action) {
        "apps.launch" -> arguments.optString("label").ifBlank { arguments.optString("package") }
        "url.open" -> arguments.optString("url")
        "web.search" -> arguments.optString("query")
        "timer.create" -> "${arguments.optInt("seconds") / 60} minutes"
        "workflow.delay" -> "${arguments.optLong("milliseconds") / 1_000} seconds"
        else -> "Runs on ${target.replaceFirstChar(Char::uppercase)}"
    }

    companion object {
        val SUPPORTED_ACTIONS = setOf(
            "apps.launch", "url.open", "web.search", "timer.create", "workflow.delay",
            "media.play_pause", "system.home", "system.notifications",
        )
    }
}

/** Execute a validated phone workflow locally, without an LLM round trip. */
suspend fun executeWorkflowDefinition(definition: String, executor: DeviceActionExecutor): String {
    val workflow = WorkflowDefinition.parse(definition).also { it.validate() }
    var completed = 0
    workflow.nodes.forEachIndexed { index, step ->
        if (step.action == "workflow.delay") {
            delay(step.arguments.optLong("milliseconds").coerceIn(100, 86_400_000))
            completed++
            return@forEachIndexed
        }
        require(step.target != "laptop") {
            "Step ${index + 1} targets the laptop; connect it before running this workflow locally"
        }
        val result = executor.execute(step.action, step.arguments)
        if (!result.success && !step.continueOnError) {
            error("Step ${index + 1} (${step.action}): ${result.message}")
        }
        completed++
        if (step.waitAfterMillis > 0 && index < workflow.nodes.lastIndex) delay(step.waitAfterMillis)
    }
    return "$completed step${if (completed == 1) "" else "s"}"
}
