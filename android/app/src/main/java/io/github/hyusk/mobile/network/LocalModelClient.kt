package io.github.hyusk.mobile.network

import io.github.hyusk.mobile.data.ModelCatalogItem
import io.github.hyusk.mobile.data.ProviderIdentity
import io.github.hyusk.mobile.security.LocalModelSettings
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

data class ModelToolExchange(
    val id: String,
    val action: String,
    val actionCallJson: String,
    val resultJson: String,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id)
        .put("action", action)
        .put("action_call", actionCallJson)
        .put("result", resultJson)

    companion object {
        fun fromJson(value: JSONObject): ModelToolExchange? {
            val id = value.optString("id").takeIf(String::isNotBlank) ?: return null
            val action = value.optString("action").takeIf(String::isNotBlank) ?: return null
            return ModelToolExchange(
                id,
                action,
                value.optString("action_call").takeIf(String::isNotBlank) ?: return null,
                value.optString("result").takeIf(String::isNotBlank) ?: return null,
            )
        }
    }
}

internal data class ModelMessagePlan(
    val role: String,
    val content: String? = null,
    val toolCallId: String? = null,
    val toolName: String? = null,
    val toolArguments: String? = null,
)

internal data class PhoneToolDefinition(
    val action: String,
    val description: String,
    val properties: JSONObject,
    val required: List<String> = emptyList(),
) {
    val name: String get() = "phone_${action.replace('.', '_')}"
}

class LocalModelClient(
    private val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .writeTimeout(30, TimeUnit.SECONDS)
        .readTimeout(120, TimeUnit.SECONDS)
        .callTimeout(150, TimeUnit.SECONDS)
        .build(),
) {
    private val toolProtocolSupport = ConcurrentHashMap<String, Boolean>()

    /** Minimal inference probe: catalog access alone does not prove model access. */
    suspend fun testConnection(settings: LocalModelSettings): String = withContext(Dispatchers.IO) {
        val apiKey = ProviderIdentity.normalizeApiKey(settings.apiKey)
        require(apiKey.isNotBlank()) { "Add a phone model API key in Settings" }
        require(settings.model.isNotBlank()) { "Choose a phone model in Settings" }
        val endpoint = completionEndpoint(settings.baseUrl)
        val body = JSONObject()
            .put("model", settings.model.trim())
            .put("messages", JSONArray().put(JSONObject().put("role", "user").put("content", "Reply with OK.")))
            .put("max_tokens", 512)
            .toString()
        http.newCall(authorizedRequest(endpoint, apiKey).post(body.toRequestBody(JSON_MEDIA_TYPE)).build())
            .execute().use { response ->
                val raw = response.body?.string().orEmpty()
                if (!response.isSuccessful) throw modelError(response.code, providerError(raw))
                parseContent(raw).trim()
            }
    }

    /** Fetches the authenticated OpenAI-compatible model catalog for a provider. */
    suspend fun listModels(settings: LocalModelSettings): List<ModelCatalogItem> = withContext(Dispatchers.IO) {
        val apiKey = ProviderIdentity.normalizeApiKey(settings.apiKey)
        require(apiKey.isNotBlank()) { "Add a phone model API key in Settings" }
        val endpoint = modelsEndpoint(settings.baseUrl)
        var lastFailure: Throwable? = null
        repeat(MAX_COMPLETION_ATTEMPTS) { attempt ->
            val builder = authorizedRequest(endpoint, apiKey)
            try {
                http.newCall(builder.get().build()).execute().use { response ->
                    val responseText = response.body?.string().orEmpty()
                    if (response.isSuccessful) return@withContext parseModels(responseText)
                    val detail = providerError(responseText)
                    lastFailure = modelError(response.code, detail)
                    if (attempt < MAX_COMPLETION_ATTEMPTS - 1 && response.code in RETRYABLE_CODES) {
                        delay(retryDelayMillis(response.header("Retry-After")))
                    } else {
                        throw modelError(response.code, detail)
                    }
                }
            } catch (failure: IOException) {
                lastFailure = failure
                if (attempt < MAX_COMPLETION_ATTEMPTS - 1) delay(600L * (attempt + 1)) else throw IllegalStateException(
                    "Could not reach the model provider. Check your internet connection and provider URL. ${failure.message.orEmpty()}".trim(),
                    failure,
                )
            }
        }
        error(lastFailure?.message ?: "Model catalog request failed")
    }

    suspend fun complete(
        settings: LocalModelSettings,
        prompt: String,
        memoryContext: String = "",
        observations: List<String> = emptyList(),
        toolHistory: List<ModelToolExchange> = emptyList(),
    ): String = withContext(Dispatchers.IO) {
        val apiKey = ProviderIdentity.normalizeApiKey(settings.apiKey)
        require(apiKey.isNotBlank()) { "Add a phone model API key in Settings" }
        require(settings.model.isNotBlank()) { "Choose a phone model in Settings" }

        val endpoint = completionEndpoint(settings.baseUrl)
        val system = """You are Hyusk, an autonomous Android assistant with direct, separate tools for app launch, accessibility tree, screenshot OCR, clicks, text entry, scrolling, gestures, media, system navigation, and memory. Use the most specific phone_* tool for exactly one next action; do not wrap actions in a generic phone_task call. If native tools are unavailable, return exactly one JSON object with {"action":"name","arguments":{...},"progress_text":"optional short spoken update"}. Use {"reply":"...","needs_reply":false,"reply_type":"none"} only when the task is complete; set needs_reply true and reply_type to question, choice, or confirmation when the user must answer. progress_text is optional, must be brief and natural to say aloud, and is not a completion claim. Never claim an action succeeded until a tool result or observation says it succeeded. Do not repeat an action that already succeeded. Prefer apps.launch before UI navigation. Available actions: apps.list, apps.launch, url.open, web.search, timer.create, reminder.create, memory.remember, memory.update, memory.forget, system.back, system.home, system.recents, system.notifications, system.quick_settings, system.settings, ui.snapshot, ui.screenshot, ui.click, ui.long_click, ui.set_text, ui.scroll, ui.tap, ui.swipe, ui.press_enter, media.play, media.pause, media.play_pause, media.next, media.previous, media.status, volume.up, volume.down, device.status, camera.open, phone.dial, sms.compose, share.text, clipboard.write. For generic media requests use Android media controls; for a named track or video, navigate the requested app's UI. For memory.remember use {"text":"concise durable fact","kind":"fact or preference"}; save stable facts, preferences, and ongoing projects the user shares when likely to help later, without requiring a separate 'remember this' phrase. Never store transient requests, guesses, secrets, credentials, or private screen content. For memory.update use {"id":"memory id","text":"replacement fact","kind":"fact or preference"}; for memory.forget use {"id":"memory id"}. Only change or remove a memory when the user clearly asks. Sensitive operations must be proposed normally; Hyusk will enforce confirmation."""
        val loopGuidance = "You have a persistent checkpoint and up to sixty-four verified actions for multi-step tasks. Treat each tool result as the authoritative result of the paired action call, and base the next step on that result plus the latest current_screen. Accessibility snapshots expose the selected foreground app/window, a visible accessibility hierarchy, an interactive_controls list with exact labels/resource IDs/editable/clickable state/screen bounds, and explicit truncation flags. If controls or text are missing, the tree is truncated, the screen is a canvas/WebView, or the result conflicts with what should be visible, call phone_ui_screenshot for on-device OCR and grounded pixel bounds; use those bounds with phone_ui_tap only after checking the screen dimensions and current screenshot. Never infer that absent tree text/control does not exist. Prefer phone_ui_click with an exact unique label over coordinates. If it returns multiple matches, choose a more specific target from the listed candidates; never guess. Use phone_ui_set_text with the exact composer/search field label; success is verified only when the intended editable control exposes the exact value in a fresh snapshot. After launching an app or changing screens, inspect the latest accessibility snapshot before selecting the next target. Once a search or app has opened successfully, continue within that app and inspect its results; do not launch a different browser or media app just to repeat the same request unless the current route failed or is unavailable. A result that says dispatched but unverified is not proof of success: inspect the new screen, try a different route only when needed, or report the specific failure. For messaging tasks, verify the contact header and composed text; send only when the user clearly asked you to send. Never return completion while an action failure is unresolved. If you cannot proceed, explain the last failed action and its actual result in plain language. Keep JSON compact so it remains complete within the response limit."
        val mediaAndVerificationGuidance = "For Android media, allowed actions also include media.play, media.pause, media.status, media.play_pause, media.next, and media.previous. Use play/pause/next/previous for the active Android media session; use media.status to check whether Android reports active playback. Generic requests such as 'play music' should use media.play directly, without launching apps or spending an LLM turn. For a named song, playlist, or YouTube video, open the requested app and navigate its visible controls. Use ui.snapshot first for accessible labels and controls. If the tree is sparse, truncated, or missing the requested content/target, call ui.screenshot to OCR the screen locally; use returned text and pixel bounds to choose a coordinate tap, then re-inspect. Screenshot capture requires Hyusk Accessibility enabled; if unavailable, state that exact blocker. Screenshot OCR text may be sent to the selected model as context, but the screenshot image itself is processed on-device and never saved by Hyusk. Direct app intents and media-session controls may still work without screen access. If a successful UI click/tap has no accessibility-tree transition, report it as accepted_unverified, not failed. Never ask the user to confirm whether it worked solely because the tree stayed the same. Instead inspect the foreground app, visible text, screenshot OCR, or media.status; for playback requests, try the native media control/status route before concluding. Do not claim the target screen or playback is verified unless evidence supports it; if genuinely blocked after trying alternate signals/routes, briefly state what was accepted and what could not be observed, without asking the user to re-verify."
        val fullInstructions = "$system\n$loopGuidance\n$mediaAndVerificationGuidance"
        val relevantContext = memoryContext.take(MAX_CONTEXT_CHARS)
        val task = prompt.take(MAX_PROMPT_CHARS)
        val observedState = compactObservations(observations)
        val userContext = buildString {
            if (relevantContext.isNotBlank()) appendLine("Relevant private on-device context:\n$relevantContext")
            appendLine("User task:\n$task")
            observedState.forEach { appendLine("Observed phone state:\n$it") }
        }
        val compatibilityKey = "${ProviderIdentity.normalizeBaseUrl(settings.baseUrl)}|${settings.model.trim()}"
        var lastFailure: Throwable? = null
        repeat(MAX_COMPLETION_ATTEMPTS) { attempt ->
            val nativeTools = !settings.model.contains("gemma", ignoreCase = true) &&
                toolProtocolSupport[compatibilityKey] != false
            val body = completionBody(
                model = settings.model.trim(),
                instructions = fullInstructions,
                userContext = userContext,
                toolHistory = toolHistory,
                maxTokens = outputTokensForAttempt(attempt),
                nativeTools = nativeTools,
            )
            val builder = authorizedRequest(endpoint, apiKey)
            try {
                http.newCall(builder.post(body.toRequestBody(JSON_MEDIA_TYPE)).build()).execute().use { response ->
                    val responseText = response.body?.string().orEmpty()
                    if (response.isSuccessful) {
                        toolProtocolSupport.putIfAbsent(compatibilityKey, nativeTools)
                        return@withContext parseContent(responseText)
                    }
                    val detail = providerError(responseText)
                    if (nativeTools && isToolProtocolRejection(response.code, detail)) {
                        toolProtocolSupport[compatibilityKey] = false
                        val fallbackBody = completionBody(
                            model = settings.model.trim(),
                            instructions = fullInstructions,
                            userContext = userContext,
                            toolHistory = toolHistory,
                            maxTokens = outputTokensForAttempt(attempt),
                            nativeTools = false,
                        )
                        http.newCall(authorizedRequest(endpoint, apiKey)
                            .post(fallbackBody.toRequestBody(JSON_MEDIA_TYPE)).build()).execute().use { fallback ->
                            val fallbackText = fallback.body?.string().orEmpty()
                            if (fallback.isSuccessful) return@withContext parseContent(fallbackText)
                            val fallbackDetail = providerError(fallbackText)
                            lastFailure = modelError(fallback.code, fallbackDetail)
                            if (attempt < MAX_COMPLETION_ATTEMPTS - 1 && fallback.code in RETRYABLE_CODES) {
                                delay(retryDelayMillis(fallback.header("Retry-After")))
                            } else {
                                throw modelError(fallback.code, fallbackDetail)
                            }
                        }
                        return@repeat
                    }
                    lastFailure = modelError(response.code, detail)
                    if (attempt < MAX_COMPLETION_ATTEMPTS - 1 && response.code in RETRYABLE_CODES) {
                        delay(retryDelayMillis(response.header("Retry-After")))
                    } else {
                        throw modelError(response.code, detail)
                    }
                }
            } catch (failure: IOException) {
                lastFailure = failure
                if (attempt < MAX_COMPLETION_ATTEMPTS - 1) {
                    delay(600L * (attempt + 1))
                } else if (failure is EmptyModelResponse) {
                    val detail = failure.message.orEmpty()
                    val message = if (detail.startsWith("The model response was truncated")) {
                        "The phone model ran out of response space. Please try that request again or choose a model with a larger output limit."
                    } else {
                        "The provider returned an empty model response after $MAX_COMPLETION_ATTEMPTS attempts. Try another model or retry later."
                    }
                    throw IllegalStateException(
                        "$message ${detail.takeIf { it.isNotBlank() && !it.startsWith("The model response was truncated") }.orEmpty()}".trim(),
                        failure,
                    )
                } else {
                    throw IllegalStateException(
                        "Could not reach the phone model after $MAX_COMPLETION_ATTEMPTS attempts. Check your connection and provider URL. ${failure.message.orEmpty()}".trim(),
                        failure,
                    )
                }
            }
        }
        error(lastFailure?.message ?: "Phone model request failed")
    }

    private fun authorizedRequest(endpoint: String, apiKey: String): Request.Builder = Request.Builder()
        .url(endpoint)
        .header("Authorization", "Bearer $apiKey")
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .apply {
            if (endpoint.contains("openrouter.ai", ignoreCase = true)) {
                header("HTTP-Referer", "https://github.com/goutamsharma/hyusk")
                header("X-OpenRouter-Title", "Hyusk")
            }
        }

    private fun completionEndpoint(raw: String): String {
        val base = ProviderIdentity.normalizeBaseUrl(raw)
        return when {
            base.endsWith("/chat/completions", ignoreCase = true) -> base
            base.equals("https://openrouter.ai", ignoreCase = true) -> "$base/api/v1/chat/completions"
            else -> "$base/chat/completions"
        }
    }

    private fun modelsEndpoint(raw: String): String {
        val base = ProviderIdentity.normalizeBaseUrl(raw)
        return when {
            base.endsWith("/models", ignoreCase = true) -> base
            base.endsWith("/chat/completions", ignoreCase = true) ->
                base.substring(0, base.length - "/chat/completions".length) + "/models"
            base.equals("https://openrouter.ai", ignoreCase = true) -> "$base/api/v1/models"
            else -> "$base/models"
        }
    }

    private fun parseModels(raw: String): List<ModelCatalogItem> {
        val root = JSONObject(raw)
        val data = root.optJSONArray("data") ?: (if (root.has("id")) JSONArray().put(root) else JSONArray())
        return buildList {
            for (index in 0 until data.length()) {
                val item = data.optJSONObject(index) ?: continue
                val id = item.optString("id").trim()
                if (id.isNotBlank()) {
                    add(ModelCatalogItem(
                        id,
                        item.optString("name").trim(),
                        item.optString("owned_by", item.optString("ownedBy")).trim(),
                    ))
                }
            }
        }.distinctBy { it.id }.sortedBy { it.id.lowercase() }
    }

    internal fun parseContent(raw: String): String {
        val choice = JSONObject(raw).optJSONArray("choices")?.optJSONObject(0)
            ?: throw EmptyModelResponse("No completion choice was included.")
        val finishReason = choice.optString("finish_reason")
            .takeUnless { it.isBlank() || it.equals("null", ignoreCase = true) }
        if (finishReason.equals("length", ignoreCase = true)) {
            throw EmptyModelResponse("The model response was truncated by the output limit (finish_reason=length).")
        }
        val message = choice.optJSONObject("message")
        val call = message?.optJSONArray("tool_calls")?.optJSONObject(0)
        val function = call?.optJSONObject("function")
        val functionName = function?.optString("name").orEmpty()
        if (functionName == LEGACY_PHONE_ACTION_FUNCTION || phoneToolAction(functionName) != null) {
            val functionArguments = runCatching { JSONObject(function?.optString("arguments").orEmpty()) }.getOrNull()
                ?: throw EmptyModelResponse("The model returned malformed phone action arguments.")
            val action = if (functionName == LEGACY_PHONE_ACTION_FUNCTION) functionArguments.optString("action")
                else phoneToolAction(functionName).orEmpty()
            if (action.isBlank()) throw EmptyModelResponse("The model called an unknown phone tool: $functionName")
            val arguments = if (functionName == LEGACY_PHONE_ACTION_FUNCTION) {
                functionArguments.optJSONObject("arguments") ?: JSONObject()
            } else functionArguments
            val parsed = JSONObject().put("action", action).put("arguments", arguments)
            call?.optString("id")?.takeIf(String::isNotBlank)?.let { parsed.put("_tool_call_id", it) }
            return parsed.toString()
        }
        val content = message?.opt("content")
        val text = when (content) {
            is String -> content
            is JSONArray -> buildString {
                for (index in 0 until content.length()) {
                    val item = content.optJSONObject(index) ?: continue
                    val part = when (val value = item.opt("text")) {
                        is String -> value
                        is JSONObject -> value.optString("value")
                        else -> item.optString("content")
                    }
                    if (part.isNotBlank() && part != "null") append(part)
                }
            }
            else -> ""
        }
        text.takeIf(String::isNotBlank)?.let { return it }
        (choice.opt("text") as? String)?.takeIf(String::isNotBlank)?.let { return it }
        (message?.opt("refusal") as? String)?.takeIf(String::isNotBlank)?.let {
            throw IllegalStateException("The selected model refused this request: $it")
        }
        throw EmptyModelResponse("Finish reason: ${finishReason ?: "unknown"}.")
    }

    /** Keep the entire observation bundle bounded, not just each individual item. */
    private fun compactObservations(observations: List<String>): List<String> {
        var remaining = MAX_OBSERVATION_TOTAL_CHARS
        val newestFirst = observations.takeLast(MAX_OBSERVATIONS).asReversed()
        val selected = newestFirst.mapNotNull { observation ->
            if (remaining <= 0) return@mapNotNull null
            val limit = minOf(MAX_OBSERVATION_CHARS, remaining)
            val compact = if (observation.startsWith(UI_TREE_PREFIX)) {
                fitUiObservation(compactUiObservation(observation), limit)
            } else {
                observation.take(limit).trim()
            }
            remaining -= compact.length
            compact.takeIf { it.isNotBlank() }
        }
        return selected.asReversed()
    }

    internal fun compactUiObservation(observation: String): String {
        if (!observation.startsWith(UI_TREE_PREFIX)) return observation
        val tree = runCatching { JSONObject(observation.removePrefix(UI_TREE_PREFIX)) }.getOrNull() ?: return observation
        val controls = tree.optJSONArray("interactive_controls") ?: JSONArray()
        val compactControls = JSONArray()
        val priorTruncation = tree.optJSONObject("truncation")
        val controlsTruncated = controls.length() > MAX_SCREEN_CONTROLS || (0 until controls.length()).any { index ->
            controls.optJSONObject(index)?.optBoolean("truncated") == true
        } || priorTruncation?.optBoolean("interactive_controls") == true
        for (index in 0 until minOf(controls.length(), MAX_SCREEN_CONTROLS)) {
            val control = controls.optJSONObject(index) ?: continue
            if (control.optBoolean("truncated")) continue
            compactControls.put(JSONObject()
                .put("label", control.optString("label"))
                .put("id", control.optString("id"))
                .put("class", control.optString("class"))
                .put("clickable", control.optBoolean("clickable"))
                .put("enabled", control.optBoolean("enabled", true))
                .put("editable", control.optBoolean("editable"))
                .put("scrollable", control.optBoolean("scrollable"))
                .put("focused", control.optBoolean("focused"))
                .put("bounds", control.optString("bounds")))
        }
        val visibleText = JSONArray()
        collectVisibleText(tree, visibleText, linkedSetOf())
        val sourceVisibleText = tree.optJSONArray("visible_text")
        val textTruncated = (sourceVisibleText?.length() ?: 0) > MAX_VISIBLE_TEXT_ITEMS ||
            priorTruncation?.optBoolean("visible_text") == true
        val truncation = JSONObject()
            .put("tree", priorTruncation?.optBoolean("tree") == true)
            .put("interactive_controls", controlsTruncated)
            .put("visible_text", textTruncated)
        return UI_TREE_PREFIX + JSONObject()
            .put("package", tree.optString("package"))
            .put("truncation", truncation)
            .put("interactive_controls", compactControls)
            .put("visible_text", visibleText)
            .toString()
    }

    /** Bound UI observations without cutting JSON mid-token, which makes it unusable to the model. */
    internal fun fitUiObservation(compacted: String, limit: Int): String {
        if (compacted.length <= limit) return compacted
        val tree = runCatching { JSONObject(compacted.removePrefix(UI_TREE_PREFIX)) }.getOrNull()
            ?: return compacted.take(limit).trim()
        val truncation = tree.optJSONObject("truncation") ?: JSONObject().also { tree.put("truncation", it) }
        truncation.put("tree", true).put("interactive_controls", true).put("visible_text", true)
        val controls = tree.optJSONArray("interactive_controls") ?: JSONArray().also { tree.put("interactive_controls", it) }
        val visibleText = tree.optJSONArray("visible_text") ?: JSONArray().also { tree.put("visible_text", it) }
        fun encode() = UI_TREE_PREFIX + tree.toString()

        var encoded = encode()
        while (encoded.length > limit && visibleText.length() > 0) {
            visibleText.remove(visibleText.length() - 1)
            encoded = encode()
        }
        while (encoded.length > limit && controls.length() > 0) {
            controls.remove(controls.length() - 1)
            encoded = encode()
        }
        if (encoded.length > limit) {
            tree.put("package", tree.optString("package").take(96))
            encoded = encode()
        }
        // The model needs a valid, explicit signal even if the caller's budget is unexpectedly tiny.
        return if (encoded.length <= limit) encoded else UI_TREE_PREFIX +
            JSONObject().put("truncation", JSONObject()
                .put("tree", true)
                .put("interactive_controls", true)
                .put("visible_text", true)).toString()
    }

    private fun collectVisibleText(value: JSONObject, output: JSONArray, seen: MutableSet<String>) {
        if (output.length() >= MAX_VISIBLE_TEXT_ITEMS) return
        listOf("text", "description").forEach { key ->
            value.optString(key).trim().takeIf { it.isNotBlank() && it != "null" && seen.add(it) }
                ?.let(output::put)
        }
        val children = value.optJSONArray("children") ?: return
        for (index in 0 until children.length()) {
            children.optJSONObject(index)?.let { collectVisibleText(it, output, seen) }
            if (output.length() >= MAX_VISIBLE_TEXT_ITEMS) return
        }
    }

    private fun isToolProtocolRejection(code: Int, detail: String): Boolean {
        if (code != 400 && code != 422) return false
        val message = detail.lowercase()
        return "tool" in message || "function" in message ||
            ("role" in message && ("alternat" in message || "invalid" in message))
    }

    private fun outputTokensForAttempt(attempt: Int): Int = when (attempt) {
        0 -> INITIAL_OUTPUT_TOKENS
        1 -> RETRY_OUTPUT_TOKENS
        else -> MAX_OUTPUT_TOKENS
    }

    private fun providerError(raw: String): String = runCatching {
        val error = JSONObject(raw).opt("error")
        when (error) {
            is JSONObject -> error.optString("message")
            is String -> error
            else -> raw
        }
    }.getOrDefault(raw).take(320)

    private fun modelError(code: Int, detail: String): IllegalStateException {
        val hint = when (code) {
            401 -> "Authentication failed (401). Re-enter the API key; do not include 'Bearer'."
            402 -> "The provider account has insufficient credits (402)."
            403 -> "Provider denied this request (403). Check account permissions or model access."
            404 -> "Provider endpoint or model was not found (404). Check the provider URL and model ID."
            408 -> "The provider timed out (408). Try again."
            413 -> "The request context is too large (413). Start a new task or reduce retained context."
            422 -> "The selected model rejected this request (422). Try another model."
            429 -> "The provider rate limit was reached (429). Wait briefly and retry."
            in 500..599 -> "The model provider is temporarily unavailable ($code)."
            else -> "Phone model returned HTTP $code."
        }
        return IllegalStateException(if (detail.isBlank()) hint else "$hint Provider: $detail")
    }

    private fun retryDelayMillis(retryAfter: String?): Long {
        val seconds = retryAfter?.toLongOrNull()?.coerceIn(1, 3) ?: 1
        return seconds * 1_000
    }

    companion object {
        private val JSON_MEDIA_TYPE = "application/json".toMediaType()
        private val RETRYABLE_CODES = setOf(408, 429, 500, 502, 503, 504)
        private const val MAX_COMPLETION_ATTEMPTS = 3
        private const val MAX_CONTEXT_CHARS = 12_000
        private const val MAX_PROMPT_CHARS = 8_000
        private const val MAX_OBSERVATIONS = 6
        private const val MAX_OBSERVATION_CHARS = 3_500
        private const val MAX_OBSERVATION_TOTAL_CHARS = 12_000
        private const val INITIAL_OUTPUT_TOKENS = 900
        private const val RETRY_OUTPUT_TOKENS = 1_200
        private const val MAX_OUTPUT_TOKENS = 1_600
        private const val LEGACY_PHONE_ACTION_FUNCTION = "hyusk_phone_action"
        private const val UI_TREE_PREFIX = "Current accessibility tree: "
        private const val MAX_TOOL_HISTORY = 8
        private const val MAX_SCREEN_CONTROLS = 24
        private const val MAX_VISIBLE_TEXT_ITEMS = 24

        private fun completionBody(
            model: String,
            instructions: String,
            userContext: String,
            toolHistory: List<ModelToolExchange>,
            maxTokens: Int,
            nativeTools: Boolean,
        ): String {
            val messages = JSONArray()
            buildMessagePlan(model, instructions, userContext, toolHistory, nativeTools).forEach { planned ->
                val message = JSONObject().put("role", planned.role)
                if (planned.toolCallId != null && planned.role == "assistant") {
                    message.put("content", JSONObject.NULL)
                        .put("tool_calls", JSONArray().put(JSONObject()
                            .put("id", planned.toolCallId)
                            .put("type", "function")
                            .put("function", JSONObject()
                                .put("name", planned.toolName)
                                .put("arguments", planned.toolArguments))))
                } else if (planned.role == "tool") {
                    message.put("tool_call_id", planned.toolCallId)
                        .put("name", planned.toolName)
                        .put("content", planned.content)
                } else {
                    message.put("content", planned.content.orEmpty())
                }
                messages.put(message)
            }

            val body = JSONObject()
                .put("model", model)
                .put("max_tokens", maxTokens)
                .put("messages", messages)
            if (nativeTools) {
                body.put("tools", JSONArray().also { tools ->
                    phoneToolDefinitions().forEach { definition -> tools.put(definition.toApiJson()) }
                })
                body.put("tool_choice", "auto")
                body.put("parallel_tool_calls", false)
            }
            return body.toString()
        }

        internal fun buildMessagePlan(
            model: String,
            instructions: String,
            userContext: String,
            toolHistory: List<ModelToolExchange>,
            nativeTools: Boolean,
        ): List<ModelMessagePlan> = buildList {
            if (model.contains("gemma", ignoreCase = true)) {
                add(ModelMessagePlan("user", "Instructions:\n$instructions\n$userContext"))
            } else {
                add(ModelMessagePlan("system", instructions))
                add(ModelMessagePlan("user", userContext))
            }
            toolHistory.takeLast(MAX_TOOL_HISTORY).forEach { exchange ->
                if (nativeTools) {
                    add(ModelMessagePlan(
                        role = "assistant",
                        toolCallId = exchange.id,
                        toolName = phoneToolName(exchange.action),
                        toolArguments = runCatching {
                            JSONObject(exchange.actionCallJson).optJSONObject("arguments")?.toString() ?: "{}"
                        }.getOrDefault("{}"),
                    ))
                    add(ModelMessagePlan(
                        role = "tool",
                        content = exchange.resultJson,
                        toolCallId = exchange.id,
                        toolName = phoneToolName(exchange.action),
                    ))
                } else {
                    add(ModelMessagePlan("assistant", exchange.actionCallJson))
                    add(ModelMessagePlan(
                        "user",
                        "Phone tool result for ${exchange.action} (call ${exchange.id}): ${exchange.resultJson}",
                    ))
                }
            }
        }

        private fun phoneToolName(action: String): String = phoneToolDefinitions()
            .firstOrNull { it.action == action }?.name ?: "phone_${action.replace('.', '_')}"

        private fun phoneToolAction(name: String): String? = phoneToolDefinitions()
            .firstOrNull { it.name == name }?.action

        private fun PhoneToolDefinition.toApiJson(): JSONObject = JSONObject()
            .put("type", "function")
            .put("function", JSONObject()
                .put("name", name)
                .put("description", description)
                .put("parameters", JSONObject()
                    .put("type", "object")
                    .put("properties", properties)
                    .put("required", JSONArray().also { values -> required.forEach(values::put) })
                    .put("additionalProperties", false)))

        private fun phoneToolDefinitions(): List<PhoneToolDefinition> {
            fun p(name: String, type: String = "string") = name to type
            fun tool(action: String, description: String, required: List<String> = emptyList(), vararg fields: Pair<String, String>) =
                PhoneToolDefinition(action, description, JSONObject().also { properties ->
                    fields.forEach { (name, type) ->
                        val value = JSONObject().put("type", type)
                        if (name == "direction") value.put("enum", JSONArray().put("forward").put("backward").put("up").put("down"))
                        properties.put(name, value)
                    }
                }, required)
            return listOf(
                tool("apps.list", "List installed launchable apps on this phone."),
                tool("apps.launch", "Open an installed app by its visible app name or package.", listOf("app"), p("app")),
                tool("ui.snapshot", "Read the current accessibility hierarchy: app/window, visible text, controls, IDs, bounds, and truncation flags."),
                tool("ui.screenshot", "Capture the current display and run local OCR; returns readable lines and screen-pixel bounds, never saves the image."),
                tool("ui.click", "Click one visible accessibility control by its exact unique label.", listOf("text"), p("text")),
                tool("ui.long_click", "Long-press one visible accessibility control by its exact unique label.", listOf("text"), p("text")),
                tool("ui.set_text", "Enter text into a named editable control.", listOf("text"), p("target"), p("text")),
                tool("ui.scroll", "Scroll a visible scrollable container.", listOf("direction"), p("direction")),
                tool("ui.tap", "Tap a grounded screen-pixel coordinate from the latest accessibility or OCR observation.", listOf("x", "y"), p("x", "number"), p("y", "number")),
                tool("ui.swipe", "Swipe between grounded screen-pixel coordinates.", listOf("x1", "y1", "x2", "y2"), p("x1", "number"), p("y1", "number"), p("x2", "number"), p("y2", "number"), p("duration_ms", "integer")),
                tool("ui.press_enter", "Press enter in the focused input field."),
                tool("system.back", "Navigate back on the phone."),
                tool("system.home", "Open the phone home screen."),
                tool("system.recents", "Show recent apps."),
                tool("system.notifications", "Open the notification shade."),
                tool("system.quick_settings", "Open quick settings."),
                tool("system.settings", "Open the requested Android settings page.", fields = arrayOf(p("action"))),
                tool("url.open", "Open a website URL in the user's default browser.", listOf("url"), p("url")),
                tool("web.search", "Search the web from the phone.", listOf("query"), p("query")),
                tool("timer.create", "Create a phone timer.", listOf("seconds"), p("seconds", "integer"), p("label")),
                tool("reminder.create", "Create a phone reminder.", listOf("hour", "minute"), p("hour", "integer"), p("minute", "integer"), p("label")),
                tool("media.play", "Resume the active phone media session."),
                tool("media.pause", "Pause the active phone media session."),
                tool("media.play_pause", "Toggle playback for the active phone media session."),
                tool("media.next", "Skip to the next item in the active phone media session."),
                tool("media.previous", "Go to the previous item in the active phone media session."),
                tool("media.status", "Read active media app, title, and playback state."),
                tool("volume.up", "Raise phone media volume."),
                tool("volume.down", "Lower phone media volume."),
                tool("device.status", "Read phone battery, charging, connectivity, and current display status."),
                tool("camera.open", "Open the phone camera."),
                tool("phone.dial", "Open the dialer with a phone number.", listOf("phone"), p("phone")),
                tool("sms.compose", "Open a new text message addressed to a phone number.", listOf("phone"), p("phone"), p("text")),
                tool("share.text", "Open Android's share sheet with text.", listOf("text"), p("text")),
                tool("clipboard.write", "Copy text to the phone clipboard.", listOf("text"), p("text")),
                tool("memory.remember", "Save a stable user fact or preference to encrypted on-device memory.", listOf("text"), p("text"), p("kind")),
                tool("memory.update", "Update an existing saved memory by ID.", listOf("id", "text"), p("id"), p("text"), p("kind")),
                tool("memory.forget", "Remove one saved memory by ID.", listOf("id"), p("id")),
            )
        }
    }

    private class EmptyModelResponse(message: String) : IOException(message)
}
