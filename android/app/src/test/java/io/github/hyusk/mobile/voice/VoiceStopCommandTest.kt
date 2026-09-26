package io.github.hyusk.mobile.voice

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceStopCommandTest {
    @Test fun recognizesExplicitStopCommands() {
        assertTrue(isVoiceStopCommand("Go away, Hyusk."))
        assertTrue(isVoiceStopCommand("stop listening"))
        assertFalse(isVoiceStopCommand("Search for the song Go Away"))
    }
}
