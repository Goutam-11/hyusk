package io.github.hyusk.mobile.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PhoneAgentResumeTest {
    @Test fun newRequestDoesNotResumeWaitingTask() {
        assertFalse(PhoneAgent.resumesAwaitingRun("open Instagram instead", false))
        assertFalse(PhoneAgent.resumesAwaitingRun("search for another song", true))
        assertTrue(PhoneAgent.resumesAwaitingRun("continue", false))
        assertTrue(PhoneAgent.resumesAwaitingRun("confirm", true))
        assertFalse(PhoneAgent.resumesAwaitingRun("continue", true))
    }

    @Test fun nonIdempotentMediaAndVolumeActionsHaveOneStableAttemptAcrossScreens() {
        for (action in PhoneAgent.SINGLE_ATTEMPT_ACTIONS) {
            val first = PhoneAgent.attemptSignature(action, "{}", "screen-a")
            val later = PhoneAgent.attemptSignature(action, "{ignored:argument}", "screen-b")
            val attempts = mapOf("$action:{}|screen=old-fingerprint" to 1)
            assertEquals("$action must not get a fresh retry key when the UI changes", first, later)
            assertEquals("$action must have only one automatic attempt", 1, PhoneAgent.attemptLimitFor(action))
            assertEquals("Legacy checkpoint attempts must still block a duplicate", 1,
                PhoneAgent.attemptsFor(action, first, attempts))
        }
    }

    @Test fun repeatableUiActionsKeepScreenScopedRetries() {
        val args = "{direction:forward}"
        val first = PhoneAgent.attemptSignature("ui.scroll", args, "screen-a")
        val later = PhoneAgent.attemptSignature("ui.scroll", args, "screen-b")

        assertNotEquals(first, later)
        assertEquals(4, PhoneAgent.attemptLimitFor("ui.scroll"))
        assertEquals(4, PhoneAgent.attemptLimitFor("ui.tap"))
    }
}
