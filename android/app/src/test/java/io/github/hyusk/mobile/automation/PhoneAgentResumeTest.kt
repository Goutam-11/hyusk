package io.github.hyusk.mobile.automation

import org.junit.Assert.assertFalse
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
}
