package io.github.hyusk.mobile.network

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.json.JSONArray
import org.json.JSONObject

class LocalModelClientProtocolTest {
    @Test
    fun nativeActionToolCallRetainsCallIdOutsideActionArguments() {
        val response = JSONObject()
            .put("choices", JSONArray().put(JSONObject()
                .put("finish_reason", "tool_calls")
                .put("message", JSONObject().put("tool_calls", JSONArray().put(JSONObject()
                    .put("id", "call_open")
                    .put("function", JSONObject()
                        .put("name", "phone_apps_launch")
                        .put("arguments", "{\"app\":\"WhatsApp\"}")))))))

        val parsed = JSONObject(LocalModelClient().parseContent(response.toString()))

        assertEquals("apps.launch", parsed.optString("action"))
        assertEquals("call_open", parsed.optString("_tool_call_id"))
        assertEquals("WhatsApp", parsed.getJSONObject("arguments").optString("app"))
        assertFalse(parsed.getJSONObject("arguments").has("_tool_call_id"))
    }

    @Test
    fun nativeHistoryPairsAssistantToolCallWithMatchingToolResult() {
        val exchange = ModelToolExchange(
            id = "call_123",
            action = "ui.click",
            actionCallJson = "{\"action\":\"ui.click\",\"arguments\":{\"text\":\"Recipes\"}}",
            resultJson = "{\"success\":true,\"verification\":\"screen_changed\"}",
        )

        val messages = LocalModelClient.buildMessagePlan(
            "openrouter/test-model", "system instructions", "task context", listOf(exchange), nativeTools = true,
        )

        assertEquals(listOf("system", "user", "assistant", "tool"), messages.map { it.role })
        assertEquals("call_123", messages[2].toolCallId)
        assertEquals("phone_ui_click", messages[2].toolName)
        assertEquals("{\"text\":\"Recipes\"}", messages[2].toolArguments)
        assertEquals("call_123", messages[3].toolCallId)
        assertEquals(exchange.resultJson, messages[3].content)
    }

    @Test
    fun eachPhoneActionReplaysAsItsOwnNamedToolWithOnlyActionArguments() {
        val exchange = ModelToolExchange(
            id = "call_launch",
            action = "apps.launch",
            actionCallJson = "{\"action\":\"apps.launch\",\"arguments\":{\"app\":\"WhatsApp\"}}",
            resultJson = "{\"success\":true,\"package\":\"com.whatsapp\"}",
        )
        val messages = LocalModelClient.buildMessagePlan("model", "system", "task", listOf(exchange), true)

        assertEquals("phone_apps_launch", messages[2].toolName)
        assertEquals("{\"app\":\"WhatsApp\"}", messages[2].toolArguments)
        assertEquals("phone_apps_launch", messages[3].toolName)
        assertEquals(exchange.resultJson, messages[3].content)
    }

    @Test
    fun gemmaUsesUserInstructionsAndAlternatingFallbackTurns() {
        val exchange = ModelToolExchange("call_456", "apps.launch", "{}", "{\"success\":true}")
        val messages = LocalModelClient.buildMessagePlan(
            "gemma-test", "instructions", "task", listOf(exchange), nativeTools = false,
        )

        assertEquals(listOf("user", "assistant", "user"), messages.map { it.role })
        assertTrue(messages.first().content.orEmpty().contains("Instructions:"))
        assertTrue(messages[2].content.orEmpty().contains("Phone tool result"))
    }

    @Test
    fun unsupportedProviderFallbackUsesValidAlternatingChatTurns() {
        val exchange = ModelToolExchange(
            id = "call_789",
            action = "apps.launch",
            actionCallJson = "{\"action\":\"apps.launch\",\"arguments\":{\"app\":\"Chrome\"}}",
            resultJson = "{\"success\":true}",
        )
        val messages = LocalModelClient.buildMessagePlan(
            "gemma-test", "instructions", "task", listOf(exchange), nativeTools = false,
        )

        assertEquals(listOf("user", "assistant", "user"), messages.map { it.role })
        assertEquals(exchange.actionCallJson, messages[1].content)
        assertTrue(messages[2].content.orEmpty().contains("Phone tool result for apps.launch"))
        assertFalse(messages.any { it.role == "tool" })
    }

    @Test
    fun replayIsBoundedToTheMostRecentEightActionResults() {
        val history = (0..9).map { index ->
            ModelToolExchange("call_$index", "ui.click", "{}", "result_$index")
        }
        val messages = LocalModelClient.buildMessagePlan("model", "system", "task", history, nativeTools = true)

        assertEquals(18, messages.size)
        assertEquals("call_2", messages[2].toolCallId)
        assertEquals("call_9", messages.last().toolCallId)
    }

    @Test
    fun uiObservationCompactionPreservesAccessibilityTruncationFlags() {
        val input = JSONObject()
            .put("package", "com.example")
            .put("truncation", JSONObject()
                .put("tree", true)
                .put("interactive_controls", true)
                .put("visible_text", false))
            .put("interactive_controls", JSONArray().put(JSONObject()
                .put("label", "Search")
                .put("id", "app:id/search")
                .put("editable", true)
                .put("enabled", true)))
            .put("visible_text", JSONArray().put("Search"))

        val compacted = LocalModelClient().compactUiObservation("Current accessibility tree: $input")
        val output = JSONObject(compacted.removePrefix("Current accessibility tree: "))

        assertTrue(output.getJSONObject("truncation").optBoolean("tree"))
        assertTrue(output.getJSONObject("truncation").optBoolean("interactive_controls"))
        assertFalse(output.getJSONObject("truncation").optBoolean("visible_text"))
        assertTrue(output.getJSONArray("interactive_controls").getJSONObject(0).optBoolean("enabled"))
    }

    @Test
    fun oversizedUiObservationStaysValidJsonAndSignalsTruncation() {
        val hugeLabel = "Recipe page content ".repeat(600)
        val input = JSONObject()
            .put("package", "com.example.reader")
            .put("interactive_controls", JSONArray().put(JSONObject()
                .put("label", hugeLabel)
                .put("id", "com.example:id/search")
                .put("clickable", true)))
            .put("visible_text", JSONArray().put(hugeLabel))
        val compacted = LocalModelClient().compactUiObservation("Current accessibility tree: $input")
        val bounded = LocalModelClient().fitUiObservation(compacted, 3_500)
        val output = JSONObject(bounded.removePrefix("Current accessibility tree: "))

        assertTrue(bounded.length <= 3_500)
        assertTrue(output.getJSONObject("truncation").optBoolean("tree"))
        assertTrue(output.getJSONObject("truncation").optBoolean("interactive_controls"))
        assertTrue(output.getJSONObject("truncation").optBoolean("visible_text"))
    }
}
