package io.github.hyusk.mobile.automation

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PhoneAgentUiVerificationTest {
    @Test
    fun failedScreenReadInvalidatesOldTreeUntilFreshSnapshotArrives() {
        val observations = listOf(
            "Current accessibility tree: {\"package\":\"com.example.old\"}",
            "Current screen read failed: no active app window",
        )

        assertTrue(latestFreshScreenObservation(
            observations,
            PhoneAgent.SCREEN_OBSERVATION_PREFIX,
            PhoneAgent.SCREEN_READ_FAILURE_PREFIX,
        ) == null)
        assertTrue(latestFreshScreenObservation(
            observations + "Current accessibility tree: {\"package\":\"com.example.new\"}",
            PhoneAgent.SCREEN_OBSERVATION_PREFIX,
            PhoneAgent.SCREEN_READ_FAILURE_PREFIX,
        ) == "{\"package\":\"com.example.new\"}")
    }

    @Test
    fun setTextVerificationReadsTheNamedEditableFieldNotOtherScreenText() {
        val before = summary(
            control(label = "Search", id = "app:id/search", bounds = "[0,0][300,80]"),
            control(label = "Message", id = "app:id/message", bounds = "[0,100][300,180]"),
        )
        val after = JSONObject().put("children", JSONArray()
            .put(editable("app:id/search", "[0,0][300,80]", ""))
            .put(editable("app:id/message", "[0,100][300,180]", "pasta recipe")))

        assertFalse(setTextPostconditionVerified(before.toString(), after, "Search", "pasta recipe"))
    }

    @Test
    fun setTextVerificationAcceptsExactReadbackFromIdentifiedField() {
        val before = summary(control("Search", "app:id/search", "[0,0][300,80]"))
        val after = JSONObject().put("children", JSONArray()
            .put(editable("app:id/search", "[0,0][300,80]", "pasta recipe")))

        assertTrue(setTextPostconditionVerified(before.toString(), after, "Search", "pasta recipe"))
    }

    @Test
    fun missingOrAmbiguousFieldReadbackIsNotVerified() {
        val ambiguous = summary(
            control("Search", "app:id/search_one", "[0,0][300,80]"),
            control("Search", "app:id/search_two", "[0,100][300,180]"),
        )
        val after = JSONObject().put("children", JSONArray()
            .put(editable("app:id/search_one", "[0,0][300,80]", "pasta recipe")))

        assertFalse(setTextPostconditionVerified(ambiguous.toString(), after, "Search", "pasta recipe"))
        assertFalse(setTextPostconditionVerified(null, after, "Search", "pasta recipe"))
    }

    @Test
    fun snapshotTruncationReportsTreeControlAndVisibleTextOmissions() {
        val controls = JSONArray()
        repeat(25) { index -> controls.put(control("Control $index", "id/$index", "[$index,0][${index + 1},1]")) }
        val tree = JSONObject()
            .put("children", JSONArray().put(JSONObject().put("truncated", true)))
            .put("interactive_controls", controls)
            .put("visible_text", JSONArray().apply { repeat(25) { put("text $it") } })

        val flags = snapshotTruncationMetadata(tree, controlLimit = 24, textLimit = 24)
        assertTrue(flags.optBoolean("tree"))
        assertTrue(flags.optBoolean("interactive_controls"))
        assertTrue(flags.optBoolean("visible_text"))
    }

    private fun summary(vararg controls: JSONObject) = JSONObject()
        .put("interactive_controls", JSONArray().apply { controls.forEach(::put) })

    private fun control(label: String, id: String, bounds: String) = JSONObject()
        .put("label", label)
        .put("id", id)
        .put("editable", true)
        .put("bounds", bounds)

    private fun editable(id: String, bounds: String, value: String) = JSONObject()
        .put("id", id)
        .put("bounds", bounds)
        .put("editable", true)
        .put("text", value)
}
