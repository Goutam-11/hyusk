package io.github.hyusk.mobile.services

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.graphics.Rect
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import java.util.ArrayDeque
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

class HyuskAccessibilityService : AccessibilityService() {
    companion object {
        private val _connected = MutableStateFlow(false)
        val connected: StateFlow<Boolean> = _connected
        var instance: HyuskAccessibilityService? = null
            private set
    }

    override fun onServiceConnected() { super.onServiceConnected(); instance = this; _connected.value = true }
    override fun onDestroy() { instance = null; _connected.value = false; super.onDestroy() }
    override fun onAccessibilityEvent(event: AccessibilityEvent?) { /* events are consumed by workflow runs on demand */ }
    override fun onInterrupt() = Unit

    fun snapshot(): UiNodeSnapshot? = targetWindowRoot()?.let(::readNode)

    fun snapshotJson(): JSONObject? = targetWindowRoot()?.let { root ->
        nodeJson(root).put("interactive_controls", interactiveControls(root))
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
        if (x !in 0f..metrics.widthPixels.toFloat() || y !in 0f..metrics.heightPixels.toFloat()) return false
        val path = Path().apply { moveTo(x, y) }
        return dispatchGesture(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, 50)).build(), null, null)
    }

    fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long): Boolean {
        if (!EmergencyStop.allowAction()) return false
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
        for (match in findNodes(root, text)) {
            var current: AccessibilityNodeInfo? = match
            while (current != null) {
                if (current.isClickable && EmergencyStop.allowAction() && current.refresh() && current.isEnabled &&
                    current.isVisibleToUser && current.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK)) return true
                current = current.parent
            }
        }
        return false
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
        val action = if (direction.equals("backward", true) || direction.equals("up", true))
            AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD else AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
        var node: AccessibilityNodeInfo? = targetWindowRoot()
        while (node != null) {
            if (node.isScrollable && node.refresh() && node.isEnabled && node.isVisibleToUser && node.performAction(action)) return true
            node = firstScrollableChild(node)
        }
        return false
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
            Triple(window, root, root.packageName?.toString())
        }
        val external = candidates.filter { (_, _, packageName) -> packageName != this.packageName }
        return external.firstOrNull { (window, _, _) -> window.isFocused && window.type != AccessibilityWindowInfo.TYPE_INPUT_METHOD }?.second
            ?: external.firstOrNull { (window, _, _) -> window.isActive && window.type != AccessibilityWindowInfo.TYPE_INPUT_METHOD }?.second
            ?: external.firstOrNull { (window, _, _) -> window.type == AccessibilityWindowInfo.TYPE_APPLICATION }?.second
            ?: candidates.firstOrNull { (window, _, _) -> window.isFocused }?.second
            ?: rootInActiveWindow
    }

    private fun findFirstEditable(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        if (node.isEditable) return node
        for (index in 0 until node.childCount) {
            node.getChild(index)?.let { child -> findFirstEditable(child)?.let { return it } }
        }
        return null
    }

    private fun firstScrollableChild(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        for (index in 0 until node.childCount) {
            val child = node.getChild(index) ?: continue
            if (child.isScrollable) return child
            firstScrollableChild(child)?.let { return it }
        }
        return null
    }

    @Suppress("DEPRECATION")
    private fun nodeJson(node: AccessibilityNodeInfo, depth: Int = 0, budget: IntArray = intArrayOf(180)): JSONObject {
        if (budget[0]-- <= 0) return JSONObject().put("truncated", true)
        val children = JSONArray()
        if (depth < 8) for (index in 0 until node.childCount) node.getChild(index)?.let { child ->
            if (child.isVisibleToUser) children.put(nodeJson(child, depth + 1, budget))
        }
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
            val label = node.text?.toString()?.takeIf(String::isNotBlank)
                ?: node.contentDescription?.toString()?.takeIf(String::isNotBlank)
            if (node.isClickable || node.isEditable || node.isScrollable || node.isFocused) {
                val bounds = Rect().also(node::getBoundsInScreen)
                controls += controlPriority(node) to JSONObject()
                    .put("label", label.orEmpty())
                    .put("id", node.viewIdResourceName.orEmpty())
                    .put("class", node.className?.toString().orEmpty())
                    .put("clickable", node.isClickable)
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
