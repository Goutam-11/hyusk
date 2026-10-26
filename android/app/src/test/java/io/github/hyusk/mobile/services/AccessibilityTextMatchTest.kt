package io.github.hyusk.mobile.services

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
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

    @Test
    fun controlLabelFallsBackToVisibleDescendantText() {
        assertEquals("Quick Pick", accessibilityControlLabel(null, null, listOf("Quick Pick", "Artist")))
    }

    @Test
    fun controlLabelPrefersItsOwnAccessibleName() {
        assertEquals("Play", accessibilityControlLabel(null, "Play", listOf("Song title")))
    }

    @Test
    fun swipeEndpointsMustBeFiniteAndInsideDisplayBounds() {
        assertTrue(swipeCoordinatesWithinDisplay(10f, 20f, 300f, 700f, 400, 800))
        assertFalse(swipeCoordinatesWithinDisplay(-1f, 20f, 300f, 700f, 400, 800))
        assertFalse(swipeCoordinatesWithinDisplay(10f, 20f, 400f, 700f, 400, 800))
        assertFalse(swipeCoordinatesWithinDisplay(10f, Float.NaN, 300f, 700f, 400, 800))
        assertFalse(swipeCoordinatesWithinDisplay(10f, 20f, 300f, 700f, 0, 800))
    }

    @Test
    fun tapCoordinatesMustBeInsideHalfOpenDisplayBounds() {
        assertTrue(pointWithinDisplay(0f, 0f, 400, 800))
        assertFalse(pointWithinDisplay(400f, 100f, 400, 800))
        assertFalse(pointWithinDisplay(100f, 800f, 400, 800))
        assertFalse(pointWithinDisplay(Float.POSITIVE_INFINITY, 100f, 400, 800))
    }

    @Test
    fun scrollDirectionAcceptsOnlyKnownAliases() {
        assertEquals("forward", normalizedScrollDirection("forward"))
        assertEquals("forward", normalizedScrollDirection(" DOWN "))
        assertEquals("backward", normalizedScrollDirection("up"))
        assertEquals("backward", normalizedScrollDirection("BACKWARD"))
        assertEquals(null, normalizedScrollDirection("sideways"))
        assertEquals(null, normalizedScrollDirection(""))
    }

    @Test
    fun scrollFallbackSwipesWithinTheScrollableBoundsInTheRequestedDirection() {
        val forward = scrollSwipeCoordinates(100, 200, 300, 600, 400, 800, "forward")!!
        assertEquals(200f, forward.startX)
        assertEquals(520f, forward.startY)
        assertEquals(200f, forward.endX)
        assertEquals(280f, forward.endY)

        val backward = scrollSwipeCoordinates(100, 200, 300, 600, 400, 800, "up")!!
        assertEquals(200f, backward.startX)
        assertEquals(280f, backward.startY)
        assertEquals(200f, backward.endX)
        assertEquals(520f, backward.endY)
    }

    @Test
    fun scrollFallbackClipsToDisplayAndRejectsEmptyOrInvalidBounds() {
        val clipped = scrollSwipeCoordinates(-20, 700, 200, 900, 400, 800, "down")!!
        assertTrue(pointWithinDisplay(clipped.startX, clipped.startY, 400, 800))
        assertTrue(pointWithinDisplay(clipped.endX, clipped.endY, 400, 800))
        assertEquals(null, scrollSwipeCoordinates(10, 20, 10, 80, 400, 800, "forward"))
        assertEquals(null, scrollSwipeCoordinates(10, 20, 100, 21, 400, 800, "forward"))
        assertEquals(null, scrollSwipeCoordinates(10, 20, 100, 80, 0, 800, "forward"))
        assertEquals(null, scrollSwipeCoordinates(10, 20, 100, 80, 400, 800, "sideways"))
    }

    @Test
    fun focusedOrActiveExternalApplicationBeatsOtherWindows() {
        val focusedApp = accessibilityWindowPriority(
            isIme = false, isExternal = true, isApplication = true, isFocused = true, isActive = true,
        )
        val focusedOverlay = accessibilityWindowPriority(
            isIme = false, isExternal = true, isApplication = false, isFocused = true, isActive = true,
        )
        assertTrue(focusedApp!! < focusedOverlay!!)
        assertTrue(
            accessibilityWindowPriority(
                isIme = false, isExternal = true, isApplication = true, isFocused = false, isActive = true,
            )!! < focusedOverlay,
        )
    }

    @Test
    fun inputMethodIsExcludedAndExternalSystemWindowCanBeFallback() {
        assertEquals(
            null,
            accessibilityWindowPriority(
                isIme = true, isExternal = true, isApplication = false, isFocused = true, isActive = true,
            ),
        )
        val systemWindow = accessibilityWindowPriority(
            isIme = false, isExternal = true, isApplication = false, isFocused = false, isActive = false,
        )
        assertTrue(systemWindow != null)
    }

    @Test
    fun longClickTargetResolutionRejectsDuplicateLabelsAtDifferentBounds() {
        val duplicateTargets = listOf(
            AccessibilityTextTarget("Play", null, "[0,0][100,100]"),
            AccessibilityTextTarget("Play", null, "[0,200][100,300]"),
        )
        assertFalse(hasUniqueBestAccessibilityTarget(duplicateTargets, "Play"))
        assertTrue(hasUniqueBestAccessibilityTarget(duplicateTargets.take(1), "Play"))
    }
}
