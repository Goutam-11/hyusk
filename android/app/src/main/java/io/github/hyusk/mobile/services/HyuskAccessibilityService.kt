package io.github.hyusk.mobile.services

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Rect
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import android.util.Log
import com.google.android.gms.tasks.Task
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import java.util.ArrayDeque
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONArray
import org.json.JSONObject

data class UiNodeSnapshot(val className: String?, val text: String?, val contentDescription: String?, val clickable: Boolean, val children: List<UiNodeSnapshot>)
data class UiTextActionResult(val success: Boolean, val message: String, val candidates: List<String> = emptyList())

internal fun accessibilityTextMatchRank(text: String?, description: String?, query: String): Int {
    val needle = query.trim()
    if (needle.isEmpty()) return Int.MAX_VALUE
    val labels = listOfNotNull(text, description).map(String::trim)
    return when {
        labels.any { it.equals(needle, ignoreCase = true) } -> 0
        labels.any { it.contains(needle, ignoreCase = true) } -> 1
        else -> Int.MAX_VALUE
    }
}

internal fun accessibilityControlLabel(text: String?, description: String?, descendantLabels: Iterable<String>): String? =
    listOfNotNull(text, description).firstOrNull { it.isNotBlank() }
        ?: descendantLabels.firstOrNull { it.isNotBlank() }

internal fun swipeCoordinatesWithinDisplay(x1: Float, y1: Float, x2: Float, y2: Float, width: Int, height: Int): Boolean {
    return pointWithinDisplay(x1, y1, width, height) && pointWithinDisplay(x2, y2, width, height)
}

internal fun pointWithinDisplay(x: Float, y: Float, width: Int, height: Int): Boolean =
    width > 0 && height > 0 && x.isFinite() && y.isFinite() && x >= 0f && y >= 0f && x < width && y < height

internal fun normalizedScrollDirection(direction: String): String? = when (direction.trim().lowercase()) {
    "forward", "down" -> "forward"
    "backward", "up" -> "backward"
    else -> null
}

internal fun accessibilityWindowPriority(
    isIme: Boolean,
    isExternal: Boolean,
    isApplication: Boolean,
    isFocused: Boolean,
    isActive: Boolean,
): Int? = when {
    isIme -> null
    isExternal && isApplication && isFocused -> 0
    isExternal && isApplication && isActive -> 1
    isExternal && isFocused -> 2
    isExternal && isActive -> 3
    isExternal && isApplication -> 4
    isExternal -> 5
    isFocused -> 6
    isActive -> 7
    else -> 8
}

internal data class ScrollSwipeCoordinates(val startX: Float, val startY: Float, val endX: Float, val endY: Float)

internal fun scrollSwipeCoordinates(
    left: Int,
    top: Int,
    right: Int,
    bottom: Int,
    width: Int,
    height: Int,
    direction: String,
): ScrollSwipeCoordinates? {
    if (width <= 0 || height <= 0) return null
    val normalized = normalizedScrollDirection(direction) ?: return null
    val clippedLeft = left.coerceIn(0, width)
    val clippedTop = top.coerceIn(0, height)
    val clippedRight = right.coerceIn(0, width)
    val clippedBottom = bottom.coerceIn(0, height)
    val clippedWidth = clippedRight - clippedLeft
    val clippedHeight = clippedBottom - clippedTop
    if (clippedWidth <= 0 || clippedHeight < 2) return null
    val centerX = clippedLeft + clippedWidth / 2f
    val nearY = clippedTop + clippedHeight * 0.2f
    val farY = clippedTop + clippedHeight * 0.8f
    return if (normalized == "forward") ScrollSwipeCoordinates(centerX, farY, centerX, nearY)
    else ScrollSwipeCoordinates(centerX, nearY, centerX, farY)
}

internal data class AccessibilityTextTarget(val text: String?, val description: String?, val bounds: String)

/** Duplicate accessible labels are safe only when they resolve to the same physical target. */
internal fun hasUniqueBestAccessibilityTarget(candidates: List<AccessibilityTextTarget>, query: String): Boolean {
    if (candidates.isEmpty()) return false
    val bestRank = candidates.minOf { accessibilityTextMatchRank(it.text, it.description, query) }
    return candidates.asSequence()
        .filter { accessibilityTextMatchRank(it.text, it.description, query) == bestRank }
        .distinctBy(AccessibilityTextTarget::bounds)
        .count() == 1
}

class HyuskAccessibilityService : AccessibilityService() {
    companion object {
        private const val TAG = "HyuskAccessibility"
        private val _connected = MutableStateFlow(false)
        val connected: StateFlow<Boolean> = _connected
        @Volatile var instance: HyuskAccessibilityService? = null
            private set
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        _connected.value = true
        Log.i(TAG, "Accessibility service connected")
    }

    override fun onUnbind(intent: Intent?): Boolean {
        markDisconnected("unbound")
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        markDisconnected("destroyed")
        super.onDestroy()
    }

    private fun markDisconnected(reason: String) {
        if (instance === this) {
            instance = null
            _connected.value = false
            Log.w(TAG, "Accessibility service $reason")
        }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) { /* events are consumed by workflow runs on demand */ }
    override fun onInterrupt() = Unit

    fun snapshot(): UiNodeSnapshot? = targetWindowRoot()?.let(::readNode)

    fun snapshotJson(): JSONObject? = targetWindowRoot()?.let { root ->
        val tree = nodeJson(root)
        val controls = interactiveControls(root)
        val controlsTruncated = controls.optJSONObject(controls.length() - 1)?.optBoolean("truncated") == true
        tree.put("interactive_controls", controls)
            .put("window", JSONObject()
                .put("package", root.packageName?.toString().orEmpty())
                .put("class", root.className?.toString().orEmpty())
                .put("focused", root.isFocused)
                .put("active", root.isAccessibilityFocused))
            .put("truncation", JSONObject()
                .put("tree", containsTruncatedNode(tree))
                .put("interactive_controls", controlsTruncated)
                .put("visible_text", false))
    }

    private fun containsTruncatedNode(value: JSONObject): Boolean {
        if (value.optBoolean("truncated")) return true
        val children = value.optJSONArray("children") ?: return false
        return (0 until children.length()).any { children.optJSONObject(it)?.let(::containsTruncatedNode) == true }
    }

    /** Captures only on request and returns local OCR text/rectangles; the bitmap is never persisted. */
    suspend fun screenshotText(): JSONObject = withContext(Dispatchers.Default) {
        val bitmap = captureScreenshotBitmap()
        val recognizer = TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS)
        try {
            val recognized = recognizer.process(InputImage.fromBitmap(bitmap, 0)).awaitValue()
            val lines = JSONArray()
            val text = StringBuilder()
            recognized.textBlocks.asSequence().flatMap { it.lines.asSequence() }.take(100).forEach { line ->
                val value = line.text.trim()
                if (value.isNotBlank()) {
                    if (text.isNotEmpty()) text.append('\n')
                    text.append(value)
                    val bounds = line.boundingBox
                    val rect = JSONArray()
                    if (bounds != null) listOf(bounds.left, bounds.top, bounds.right, bounds.bottom).forEach(rect::put)
                    lines.put(JSONObject().put("text", value).put("bounds", rect))
                }
            }
            JSONObject()
                .put("source", "on_device_screenshot_ocr")
                .put("width", bitmap.width)
                .put("height", bitmap.height)
                .put("text", text.toString().take(8_000))
                .put("lines", lines)
        } finally {
            recognizer.close()
            bitmap.recycle()
        }
    }

    private suspend fun captureScreenshotBitmap(): Bitmap = suspendCancellableCoroutine { continuation ->
        takeScreenshot(Display.DEFAULT_DISPLAY, mainExecutor, object : TakeScreenshotCallback {
            override fun onSuccess(screenshot: ScreenshotResult) {
                val hardwareBuffer = screenshot.hardwareBuffer
                var hardwareBitmap: Bitmap? = null
                try {
                    val wrapped = Bitmap.wrapHardwareBuffer(hardwareBuffer, screenshot.colorSpace)
                        ?: error("Android returned an empty screenshot buffer")
                    hardwareBitmap = wrapped
                    val bitmap = wrapped.copy(Bitmap.Config.ARGB_8888, false)
                        ?: error("Could not copy the screenshot for OCR")
                    if (continuation.isActive) continuation.resume(bitmap) else bitmap.recycle()
                } catch (failure: Exception) {
                    if (continuation.isActive) continuation.resumeWithException(failure)
                } finally {
                    hardwareBitmap?.recycle()
                    hardwareBuffer.close()
                }
            }

            override fun onFailure(errorCode: Int) {
                if (continuation.isActive) continuation.resumeWithException(
                    IllegalStateException("Android screenshot capture failed (code $errorCode). Check that Hyusk Accessibility is enabled and screenshot access is allowed."),
                )
            }
        })
    }

    private suspend fun <T> Task<T>.awaitValue(): T = suspendCancellableCoroutine { continuation ->
        addOnSuccessListener { value -> if (continuation.isActive) continuation.resume(value) }
        addOnFailureListener { failure -> if (continuation.isActive) continuation.resumeWithException(failure) }
        addOnCanceledListener { continuation.cancel() }
    }

    fun click(node: AccessibilityNodeInfo): Boolean = EmergencyStop.allowAction() && node.refresh() &&
        node.isEnabled && node.isVisibleToUser && node.isClickable &&
        node.performAction(AccessibilityNodeInfo.ACTION_CLICK)

    fun setText(node: AccessibilityNodeInfo, text: String): Boolean {
        if (!EmergencyStop.allowAction()) return false
        return node.refresh() && node.isEnabled && node.isVisibleToUser && node.isEditable &&
            node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, android.os.Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        })
    }

    fun tap(x: Float, y: Float): Boolean {
        if (!EmergencyStop.allowAction()) return false
        val metrics = resources.displayMetrics
        if (!pointWithinDisplay(x, y, metrics.widthPixels, metrics.heightPixels)) return false
        val path = Path().apply { moveTo(x, y) }
        return dispatchGesture(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, 50)).build(), null, null)
    }

    fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long): Boolean {
        if (!EmergencyStop.allowAction()) return false
        val metrics = resources.displayMetrics
        if (!swipeCoordinatesWithinDisplay(x1, y1, x2, y2, metrics.widthPixels, metrics.heightPixels)) return false
        val path = Path().apply { moveTo(x1, y1); lineTo(x2, y2) }
        return dispatchGesture(
            GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, durationMs.coerceIn(100, 2_000))).build(),
            null,
            null,
        )
    }

    fun pressEnter(): Boolean {
        if (!EmergencyStop.allowAction()) return false
        val root = targetWindowRoot() ?: return false
        val focused = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: findFirstEditable(root) ?: return false
        return focused.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
    }

    fun clickText(text: String): Boolean {
        return clickTextDetailed(text).success
    }

    /** Resolve text to one unambiguous visible control and report why it cannot be activated. */
    fun clickTextDetailed(text: String): UiTextActionResult {
        if (!EmergencyStop.allowAction()) return UiTextActionResult(false, "Emergency stop is engaged")
        val root = targetWindowRoot() ?: return UiTextActionResult(false, "No active accessibility window")
        val matches = findNodes(root, text)
        if (matches.isEmpty()) return UiTextActionResult(false, "No visible, enabled control matched \"${text.trim()}\"")
        val bestRank = matches.minOf { node ->
            accessibilityTextMatchRank(node.text?.toString(), node.contentDescription?.toString(), text)
        }
        val candidates = matches.asSequence()
            .filter { node -> accessibilityTextMatchRank(node.text?.toString(), node.contentDescription?.toString(), text) == bestRank }
            .mapNotNull { match ->
                var current: AccessibilityNodeInfo? = match
                var clickable: AccessibilityNodeInfo? = null
                while (current != null) {
                    if (current.isClickable && current.isEnabled && current.isVisibleToUser) {
                        clickable = current
                        break
                    }
                    current = current.parent
                }
                val target = clickable ?: match.takeIf {
                    !Rect().also(match::getBoundsInScreen).isEmpty
                }
                target?.let { node ->
                    val bounds = Rect().also(node::getBoundsInScreen)
                    ClickCandidate(node, bounds, match.text?.toString()
                        ?: match.contentDescription?.toString().orEmpty())
                }
            }
            .distinctBy { it.bounds.toShortString() }
            .toList()
        if (candidates.isEmpty()) return UiTextActionResult(false, "Found \"${text.trim()}\", but it has no usable click target")
        if (candidates.size > 1) {
            return UiTextActionResult(
                false,
                "\"${text.trim()}\" matches more than one control; choose a more specific label or tap a listed bound",
                candidates.take(6).map { "${it.label} ${it.bounds.toShortString()}" },
            )
        }
        val candidate = candidates.single()
        if (candidate.node.refresh() && candidate.node.isEnabled && candidate.node.isVisibleToUser &&
            candidate.node.isClickable && candidate.node.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
            return UiTextActionResult(true, "Activated \"${candidate.label}\"")
        }
        // Some apps expose a labelled text/icon without a usable ACTION_CLICK.
        // Gesture dispatch is only considered success when the caller verifies a UI change.
        if (tap(candidate.bounds.exactCenterX(), candidate.bounds.exactCenterY())) {
            return UiTextActionResult(true, "Tapped \"${candidate.label}\" at ${candidate.bounds.toShortString()}")
        }
        return UiTextActionResult(false, "Android rejected the click for \"${candidate.label}\"")
    }

    private data class ClickCandidate(
        val node: AccessibilityNodeInfo,
        val bounds: Rect,
        val label: String,
    )

    fun longClickText(text: String): Boolean {
        val root = targetWindowRoot() ?: return false
        val matches = findNodes(root, text)
        if (matches.isEmpty()) return false
        val bestRank = matches.minOf { accessibilityTextMatchRank(it.text?.toString(), it.contentDescription?.toString(), text) }
        val targets = matches.asSequence()
            .filter { accessibilityTextMatchRank(it.text?.toString(), it.contentDescription?.toString(), text) == bestRank }
            .mapNotNull { match ->
                var current: AccessibilityNodeInfo? = match
                while (current != null) {
                    if (current.isClickable && current.isEnabled && current.isVisibleToUser) {
                        val clickable = current
                        val bounds = Rect().also(clickable::getBoundsInScreen)
                        return@mapNotNull ClickCandidate(
                            clickable,
                            bounds,
                            match.text?.toString() ?: match.contentDescription?.toString().orEmpty(),
                        )
                    }
                    current = current.parent
                }
                null
            }
            .distinctBy { it.bounds.toShortString() }
            .toList()
        val targetKeys = targets.map { candidate ->
            AccessibilityTextTarget(candidate.label, null, candidate.bounds.toShortString())
        }
        if (!hasUniqueBestAccessibilityTarget(targetKeys, text)) return false
        val target = targets.singleOrNull() ?: return false
        return EmergencyStop.allowAction() && target.node.refresh() && target.node.isEnabled &&
            target.node.isVisibleToUser && target.node.isClickable &&
            target.node.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK)
    }

    fun setTextInFocusedOrMatching(target: String, text: String): Boolean {
        return setTextInFocusedOrMatchingDetailed(target, text).success
    }

    fun setTextInFocusedOrMatchingDetailed(target: String, text: String): UiTextActionResult {
        val root = targetWindowRoot() ?: return UiTextActionResult(false, "No active accessibility window")
        val node = if (target.isBlank()) {
            root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.takeIf { it.isEditable }
                ?: return UiTextActionResult(false, "No focused input field; name the target field from the accessibility controls")
        } else {
            val matches = findNodes(root, target)
            if (matches.isEmpty()) return UiTextActionResult(false, "No visible field matched \"${target.trim()}\"")
            val rank = matches.minOf { accessibilityTextMatchRank(it.text?.toString(), it.contentDescription?.toString(), target) }
            val fields = matches.filter {
                accessibilityTextMatchRank(it.text?.toString(), it.contentDescription?.toString(), target) == rank
            }.mapNotNull { match -> if (match.isEditable) match else findFirstEditable(match) }
                .distinctBy { node -> Rect().also(node::getBoundsInScreen).toShortString() }
            if (fields.isEmpty()) return UiTextActionResult(false, "\"${target.trim()}\" matched, but no editable field was found")
            if (fields.size > 1) return UiTextActionResult(
                false,
                "\"${target.trim()}\" matches more than one editable field",
                fields.take(6).map { node ->
                    "${node.hintText ?: node.contentDescription ?: node.text ?: node.className} ${Rect().also(node::getBoundsInScreen).toShortString()}"
                },
            )
            fields.single()
        }
        if (setText(node, text)) return UiTextActionResult(true, "Entered text in \"${target.ifBlank { "focused input" }}\"")
        return UiTextActionResult(false, "Android rejected text entry for \"${target.ifBlank { "focused input" }}\"")
    }

    fun scroll(direction: String): Boolean {
        if (!EmergencyStop.allowAction()) return false
        val normalizedDirection = normalizedScrollDirection(direction) ?: return false
        val action = if (normalizedDirection == "backward")
            AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD else AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
        val root = targetWindowRoot() ?: return false
        val pending = ArrayDeque<AccessibilityNodeInfo>()
        var largestScrollableBounds: Rect? = null
        var largestScrollableArea = 0L
        pending.add(root)
        while (pending.isNotEmpty()) {
            val node = pending.removeFirst()
            if (node.isScrollable && node.refresh() && node.isEnabled && node.isVisibleToUser) {
                if (node.performAction(action)) return true
                val bounds = Rect().also(node::getBoundsInScreen)
                if (bounds.intersect(0, 0, resources.displayMetrics.widthPixels, resources.displayMetrics.heightPixels)) {
                    val area = bounds.width().toLong() * bounds.height()
                    if (area > largestScrollableArea) {
                        largestScrollableArea = area
                        largestScrollableBounds = bounds
                    }
                }
            }
            for (index in 0 until node.childCount) node.getChild(index)?.let(pending::addLast)
        }
        val bounds = largestScrollableBounds ?: return false
        val metrics = resources.displayMetrics
        val coordinates = scrollSwipeCoordinates(
            bounds.left, bounds.top, bounds.right, bounds.bottom,
            metrics.widthPixels, metrics.heightPixels, normalizedDirection,
        ) ?: return false
        return swipe(coordinates.startX, coordinates.startY, coordinates.endX, coordinates.endY, 500)
    }

    private fun findNodes(root: AccessibilityNodeInfo, query: String): List<AccessibilityNodeInfo> {
        val needle = query.trim()
        if (needle.isEmpty()) return emptyList()
        val candidates = (runCatching { root.findAccessibilityNodeInfosByText(needle) }.getOrDefault(emptyList()) +
            namedMatches(root, needle))
            .distinctBy { node ->
                val bounds = Rect().also(node::getBoundsInScreen)
                "${node.viewIdResourceName}:${node.text}:${node.contentDescription}:$bounds"
            }
            .filter { it.isVisibleToUser && it.isEnabled }
        return candidates.sortedBy { node -> accessibilityTextMatchRank(node.text?.toString(), node.contentDescription?.toString(), needle) }
    }

    private fun namedMatches(node: AccessibilityNodeInfo, query: String): List<AccessibilityNodeInfo> {
        val matches = mutableListOf<AccessibilityNodeInfo>()
        val description = node.contentDescription?.toString()
        val viewId = node.viewIdResourceName
        if (node.isVisibleToUser &&
            (description?.contains(query, ignoreCase = true) == true ||
                viewId?.equals(query, ignoreCase = true) == true ||
                viewId?.substringAfterLast("/")?.equals(query, ignoreCase = true) == true)
        ) {
            matches += node
        }
        for (index in 0 until node.childCount) {
            node.getChild(index)?.let { matches += namedMatches(it, query) }
        }
        return matches
    }

    /** Prefer the app being controlled over Hyusk's own voice-interaction window. */
    private fun targetWindowRoot(): AccessibilityNodeInfo? {
        val candidates = runCatching { windows }.getOrDefault(emptyList()).mapNotNull { window ->
            val root = runCatching { window.root }.getOrNull() ?: return@mapNotNull null
            val packageName = root.packageName?.toString()
            val priority = accessibilityWindowPriority(
                isIme = window.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD,
                isExternal = packageName != this.packageName,
                isApplication = window.type == AccessibilityWindowInfo.TYPE_APPLICATION,
                isFocused = window.isFocused,
                isActive = window.isActive,
            ) ?: return@mapNotNull null
            priority to root
        }
        return candidates.minByOrNull { it.first }?.second
            ?: rootInActiveWindow
    }

    private fun findFirstEditable(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        if (node.isEditable) return node
        for (index in 0 until node.childCount) {
            node.getChild(index)?.let { child -> findFirstEditable(child)?.let { return it } }
        }
        return null
    }

    @Suppress("DEPRECATION")
    private fun nodeJson(node: AccessibilityNodeInfo, depth: Int = 0, budget: IntArray = intArrayOf(180)): JSONObject {
        if (budget[0]-- <= 0) return JSONObject().put("truncated", true)
        val children = JSONArray()
        if (depth < 8) for (index in 0 until node.childCount) node.getChild(index)?.let { child ->
            if (child.isVisibleToUser) children.put(nodeJson(child, depth + 1, budget))
        } else if (node.childCount > 0) children.put(JSONObject().put("truncated", true))
        val bounds = Rect().also(node::getBoundsInScreen)
        val result = JSONObject()
            .put("class", node.className?.toString())
        if (depth == 0) result.put("package", node.packageName?.toString())
        node.viewIdResourceName?.let { result.put("id", it) }
        node.text?.toString()?.takeIf(String::isNotBlank)?.let { result.put("text", it) }
        node.contentDescription?.toString()?.takeIf(String::isNotBlank)?.let { result.put("description", it) }
        if (node.isClickable) result.put("clickable", true)
        if (node.isEditable) result.put("editable", true)
        if (node.isScrollable) result.put("scrollable", true)
        if (!node.isEnabled) result.put("enabled", false)
        if (node.isFocused) result.put("focused", true)
        if (node.isFocusable) result.put("focusable", true)
        if (node.isChecked) result.put("checked", true)
        if (node.isSelected) result.put("selected", true)
        if (node.isClickable || node.isEditable || node.text != null || node.contentDescription != null) {
            result.put("bounds", "[${bounds.left},${bounds.top}][${bounds.right},${bounds.bottom}]")
        }
        if (children.length() > 0) result.put("children", children)
        return result
    }

    private fun interactiveControls(root: AccessibilityNodeInfo): JSONArray {
        val pending = ArrayDeque<Pair<AccessibilityNodeInfo, Int>>()
        val controls = mutableListOf<Pair<Int, JSONObject>>()
        pending.add(root to 0)
        var inspected = 0
        while (pending.isNotEmpty() && inspected++ < 1_200) {
            val (node, depth) = pending.removeFirst()
            if (!node.isVisibleToUser || depth > 12) continue
            val label = accessibilityControlLabel(
                node.text?.toString(),
                node.contentDescription?.toString(),
                if (node.isClickable) descendantLabels(node) else emptyList(),
            )
            if (node.isClickable || node.isEditable || node.isScrollable || node.isFocused) {
                val bounds = Rect().also(node::getBoundsInScreen)
                controls += controlPriority(node) to JSONObject()
                    .put("label", label.orEmpty())
                    .put("id", node.viewIdResourceName.orEmpty())
                    .put("class", node.className?.toString().orEmpty())
                    .put("clickable", node.isClickable)
                    .put("enabled", node.isEnabled)
                    .put("editable", node.isEditable)
                    .put("scrollable", node.isScrollable)
                    .put("focused", node.isFocused)
                    .put("bounds", "[${bounds.left},${bounds.top}][${bounds.right},${bounds.bottom}]")
            }
            for (index in 0 until node.childCount) {
                node.getChild(index)?.let { pending.add(it to depth + 1) }
            }
        }
        return JSONArray().apply {
            controls.sortedBy { it.first }.take(40).forEach { put(it.second) }
            if (inspected >= 1_200) put(JSONObject().put("truncated", true))
        }
    }

    private fun descendantLabels(node: AccessibilityNodeInfo): List<String> {
        val pending = ArrayDeque<AccessibilityNodeInfo>()
        for (index in 0 until node.childCount) node.getChild(index)?.let(pending::addLast)
        while (pending.isNotEmpty()) {
            val child = pending.removeFirst()
            if (!child.isVisibleToUser) continue
            val label = child.text?.toString()?.takeIf(String::isNotBlank)
                ?: child.contentDescription?.toString()?.takeIf(String::isNotBlank)
            if (label != null) return listOf(label)
            for (index in 0 until child.childCount) child.getChild(index)?.let(pending::addLast)
        }
        return emptyList()
    }

    private fun controlPriority(node: AccessibilityNodeInfo): Int = when {
        node.isFocused -> 0
        node.isEditable -> 1
        node.isClickable -> 2
        node.isScrollable -> 3
        else -> 4
    }

    private fun readNode(node: AccessibilityNodeInfo): UiNodeSnapshot {
        val children = (0 until node.childCount).mapNotNull { node.getChild(it)?.let(::readNode) }
        return UiNodeSnapshot(node.className?.toString(), node.text?.toString(), node.contentDescription?.toString(), node.isClickable, children)
    }
}
