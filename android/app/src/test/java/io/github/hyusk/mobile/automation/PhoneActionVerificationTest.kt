package io.github.hyusk.mobile.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PhoneActionVerificationTest {
    @Test
    fun unchangedAccessibilityTreeDoesNotTurnAcceptedActionIntoFailure() {
        // Screen-tree evidence is tracked separately from the executor's result.
        // A successful tap can launch media without changing exposed semantics.
        assertNull(phoneActionIssue(ActionResult(true, "Tap dispatched"), "ui.click"))
    }

    @Test
    fun executorFailuresRemainActionIssues() {
        assertEquals(
            "ui.click failed: No matching control was found",
            phoneActionIssue(ActionResult(false, "No matching control was found"), "ui.click"),
        )
    }
}
