package io.github.hyusk.mobile.automation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LocalCommandRouterTest {
    @Test
    fun routesCommonMediaControlsWithoutAnLlmCall() {
        assertEquals("media.play", mediaCommandAction("Play some music"))
        assertEquals("media.pause", mediaCommandAction("pause the music"))
        assertEquals("media.next", mediaCommandAction("skip track"))
        assertEquals("media.previous", mediaCommandAction("previous song"))
        assertEquals("media.status", mediaCommandAction("is music playing"))
    }

    @Test
    fun leavesSpecificSongAndPlaylistRequestsForUiNavigation() {
        assertNull(mediaCommandAction("play a song from Quick Picks on YouTube Music"))
    }
}
