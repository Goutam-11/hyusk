package io.github.hyusk.mobile.services

import org.junit.Assert.assertEquals
import org.junit.Test

class AccessibilityTextMatchTest {
    @Test
    fun exactVisibleLabelIsPreferredOverPartialMatch() {
        assertEquals(0, accessibilityTextMatchRank("Settings", null, "settings"))
        assertEquals(1, accessibilityTextMatchRank("Settings and privacy", null, "settings"))
    }

    @Test
    fun descriptionsAreSearchableAndBlankQueriesAreRejected() {
        assertEquals(0, accessibilityTextMatchRank(null, "Search", "Search"))
        assertEquals(Int.MAX_VALUE, accessibilityTextMatchRank("Search", null, "  "))
    }
}
