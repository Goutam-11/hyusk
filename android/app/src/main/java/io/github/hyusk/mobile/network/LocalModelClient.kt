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

class LocalModelClient(
    private val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .writeTimeout(30, TimeUnit.SECONDS)
        .readTimeout(120, TimeUnit.SECONDS)
        .callTimeout(150, TimeUnit.SECONDS)
        .build(),
) {
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
    ): String = withContext(Dispatchers.IO) {
        val apiKey = ProviderIdentity.normalizeApiKey(settings.apiKey)
        require(apiKey.isNotBlank()) { "Add a phone model API key in Settings" }
        require(settings.model.isNotBlank()) { "Choose a phone model in Settings" }

        val endpoint = completionEndpoint(settings.baseUrl)
        val system = """You are Hyusk, an autonomous Android assistant. Continue the user's task one step at a time using native actions and the accessibility tree. Return exactly one complete JSON object, with no markdown and no text outside the object. When another action is required, use {"action":"name","arguments":{...},"progress_text":"optional short spoken update"}. Use {"reply":"...","needs_reply":false,"reply_type":"none"} only when the task is complete; set needs_reply true and reply_type to question, choice, or confirmation when the user must answer. progress_text is optional, must be brief and natural to say aloud, and is not a completion claim. Never claim an action succeeded until an observation says it succeeded. Do not repeat an action that already succeeded. Prefer apps.launch before UI navigation. Allowed actions: apps.list, apps.launch, url.open, web.search, timer.create, reminder.create, memory.remember, system.back, system.home, system.recents, system.notifications, system.quick_settings, system.settings, ui.snapshot, ui.click, ui.long_click, ui.set_text, ui.scroll, ui.tap, ui.swipe, ui.press_enter, media.play_pause, media.next, media.previous, volume.up, volume.down, device.status, camera.open, phone.dial, sms.compose, share.text, clipboard.write. For memory.remember use {"text":"concise durable fact","kind":"fact or preference"}; save only stable personal facts or preferences the user explicitly states or clearly asks you to remember, never transient requests, guesses, secrets, credentials, or screen contents. Arguments must use the names implied by the action, such as app, text, query, url, phone, x, y, x1, y1, x2, y2. Sensitive operations must be proposed normally; Hyusk will enforce confirmation."""
        val loopGuidance = "You have a persistent checkpoint and up to sixty-four verified actions for multi-step tasks. After launching an app, opening a URL, submitting text, or changing screens, use the latest accessibility tree before choosing the next target. A result that says an action was dispatched but no accessibility change was verified is not proof of success: do not repeat the identical action on the same screen; inspect the tree, try a different route, or report the concrete blocker. For messaging tasks, identify the intended contact and enter the message, but stop before sending unless the user clearly authorized sending. Never return a completion reply after a failed action; recover or state the concrete blocker. Keep JSON compact so it remains complete within the response limit."
        val fullInstructions = "$system\n$loopGuidance"
        val relevantContext = memoryContext.take(MAX_CONTEXT_CHARS)
        val task = prompt.take(MAX_PROMPT_CHARS)
        val observedState = compactObservations(observations)
        val messages = if (settings.model.contains("gemma", ignoreCase = true)) {
            // Gemma chat templates require alternating user/assistant turns and
            // may reject system messages. This request has no prior assistant
            // turn, so provide all context in one user message.
            val content = buildString {
                appendLine("Instructions:\n$fullInstructions")
                if (relevantContext.isNotBlank())
                    appendLine("Relevant private on-device context:\n$relevantContext")
                appendLine("User task:\n$task")
                observedState.forEach { appendLine("Observed phone state:\n$it") }
            }
            JSONArray().put(JSONObject().put("role", "user").put("content", content))
        } else {
            JSONArray().put(JSONObject().put("role", "system").put("content", fullInstructions)).apply {
                if (relevantContext.isNotBlank()) {
                    put(JSONObject().put("role", "system").put("content", "Relevant private on-device context:\n$relevantContext"))
                }
                put(JSONObject().put("role", "user").put("content", task))
                observedState.forEach {
                    put(JSONObject().put("role", "system").put("content", "Observed phone state:\n$it"))
                }
            }
        }

        var lastFailure: Throwable? = null
        repeat(MAX_COMPLETION_ATTEMPTS) { attempt ->
            val body = JSONObject()
                .put("model", settings.model.trim())
                .put("max_tokens", outputTokensForAttempt(attempt))
                .put("messages", messages)
                .toString()
            val builder = authorizedRequest(endpoint, apiKey)
            try {
                http.newCall(builder.post(body.toRequestBody(JSON_MEDIA_TYPE)).build()).execute().use { response ->
                    val responseText = response.body?.string().orEmpty()
                    if (response.isSuccessful) return@withContext parseContent(responseText)
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

    private fun parseContent(raw: String): String {
        val choice = JSONObject(raw).optJSONArray("choices")?.optJSONObject(0)
            ?: throw EmptyModelResponse("No completion choice was included.")
        val finishReason = choice.optString("finish_reason")
            .takeUnless { it.isBlank() || it.equals("null", ignoreCase = true) }
        if (finishReason.equals("length", ignoreCase = true)) {
            throw EmptyModelResponse("The model response was truncated by the output limit (finish_reason=length).")
        }
        val message = choice.optJSONObject("message")
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
            val compact = observation.take(limit).trim()
            remaining -= compact.length
            compact.takeIf { it.isNotBlank() }
        }
        return selected.asReversed()
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

    private companion object {
        val JSON_MEDIA_TYPE = "application/json".toMediaType()
        val RETRYABLE_CODES = setOf(408, 429, 500, 502, 503, 504)
        const val MAX_COMPLETION_ATTEMPTS = 3
        const val MAX_CONTEXT_CHARS = 12_000
        const val MAX_PROMPT_CHARS = 8_000
        const val MAX_OBSERVATIONS = 6
        const val MAX_OBSERVATION_CHARS = 3_500
        const val MAX_OBSERVATION_TOTAL_CHARS = 12_000
        const val INITIAL_OUTPUT_TOKENS = 900
        const val RETRY_OUTPUT_TOKENS = 1_200
        const val MAX_OUTPUT_TOKENS = 1_600
    }

    private class EmptyModelResponse(message: String) : IOException(message)
}
