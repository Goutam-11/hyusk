package io.github.hyusk.mobile.automation

import android.accessibilityservice.AccessibilityService
import android.app.SearchManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.media.AudioManager
import android.net.Uri
import android.os.BatteryManager
import android.provider.AlarmClock
import android.provider.MediaStore
import android.provider.Settings
import android.view.KeyEvent
import io.github.hyusk.mobile.services.EmergencyStop
import io.github.hyusk.mobile.services.HyuskAccessibilityService
import org.json.JSONObject

private fun normalizeAppName(value: String): String =
    value.lowercase().replace(Regex("[^a-z0-9]+"), "")

enum class ActionRisk { Safe, Confirm, Dangerous }

data class ActionResult(
    val success: Boolean,
    val message: String,
    val data: JSONObject = JSONObject(),
)

data class InstalledApp(val packageName: String, val label: String)

/**
 * Executes the small deterministic Android vocabulary shared by workflows and
 * the remote link. Model output never becomes a shell command here.
 */
class DeviceActionExecutor(private val context: Context) {
    fun installedApps(): List<InstalledApp> = launchableApps()
        .sortedBy { it.label.lowercase() }
        .map { InstalledApp(it.packageName, it.label) }

    fun risk(action: String, args: JSONObject = JSONObject()): ActionRisk = when {
        action in setOf("ui.click", "ui.long_click") && args.optString("text").lowercase().let { label ->
            SENSITIVE_CLICK_WORDS.any(label::contains)
        } -> ActionRisk.Confirm
        action in setOf(
            "notifications.reply", "call.place", "clipboard.read", "settings.change",
            "app.install", "app.uninstall", "device.lock", "device.reboot", "shell.execute",
        ) -> ActionRisk.Confirm
        else -> ActionRisk.Safe
    }

    fun execute(action: String, args: JSONObject): ActionResult {
        if (!EmergencyStop.allowAction()) return ActionResult(false, "Emergency stop is engaged")
        return runCatching {
            when (action) {
                "apps.list" -> listApps()
                "apps.launch" -> launchApp(
                    firstArgument(args, "package", "app", "app_name", "appName", "name", "label")
                )
                "url.open" -> openUrl(args.getString("url"))
                "web.search" -> webSearch(args.getString("query"))
                "timer.create" -> setTimer(args.optInt("seconds"), args.optString("label", "Hyusk timer"))
                "reminder.create" -> setAlarm(args.optInt("hour"), args.optInt("minute"), args.optString("label", "Hyusk reminder"))
                "system.back" -> global(AccessibilityService.GLOBAL_ACTION_BACK, "Back")
                "system.home" -> global(AccessibilityService.GLOBAL_ACTION_HOME, "Home")
                "system.recents" -> global(AccessibilityService.GLOBAL_ACTION_RECENTS, "Recents")
                "system.notifications" -> global(AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS, "Notifications")
                "system.quick_settings" -> global(AccessibilityService.GLOBAL_ACTION_QUICK_SETTINGS, "Quick settings")
                "system.settings" -> openSettings(args.optString("action", Settings.ACTION_SETTINGS))
                "ui.snapshot" -> snapshot()
                "ui.click" -> accessibility().clickText(args.getString("text")).asResult("Clicked ${args.getString("text")}")
                "ui.long_click" -> accessibility().longClickText(args.getString("text")).asResult("Long-pressed ${args.getString("text")}")
                "ui.set_text" -> accessibility().setTextInFocusedOrMatching(args.optString("target"), args.getString("text")).asResult("Entered text")
                "ui.scroll" -> accessibility().scroll(args.optString("direction", "forward")).asResult("Scrolled")
                "ui.tap" -> accessibility().tap(args.getDouble("x").toFloat(), args.getDouble("y").toFloat()).asResult("Tapped")
                "ui.swipe" -> accessibility().swipe(
                    args.getDouble("x1").toFloat(), args.getDouble("y1").toFloat(),
                    args.getDouble("x2").toFloat(), args.getDouble("y2").toFloat(),
                    args.optLong("duration_ms", 350),
                ).asResult("Swiped")
                "ui.press_enter" -> accessibility().pressEnter().asResult("Pressed enter")
                "media.play_pause" -> mediaKey(KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE)
                "media.next" -> mediaKey(KeyEvent.KEYCODE_MEDIA_NEXT)
                "media.previous" -> mediaKey(KeyEvent.KEYCODE_MEDIA_PREVIOUS)
                "volume.up" -> volume(AudioManager.ADJUST_RAISE)
                "volume.down" -> volume(AudioManager.ADJUST_LOWER)
                "device.status" -> deviceStatus()
                "camera.open" -> launchIntent(Intent(MediaStore.ACTION_IMAGE_CAPTURE), "Opened camera")
                "phone.dial" -> dial(args.getString("phone"))
                "sms.compose" -> composeSms(args.getString("phone"), args.optString("text"))
                "share.text" -> shareText(args.getString("text"))
                "clipboard.write" -> writeClipboard(args.getString("text"))
                else -> ActionResult(false, "Unsupported phone action: $action")
            }
        }.getOrElse { ActionResult(false, it.message ?: "Action failed") }
    }

    private fun launchApp(requestedName: String): ActionResult {
        val requested = requestedName.trim().replace(Regex("(?i)\\s+app$"), "").trim()
        require(requested.isNotEmpty()) { "Tell me which app to open" }

        val apps = launchableApps()
        var matches = resolveApp(requested, apps)
        if (matches.isEmpty()) {
            val query = normalizeAppName(requested)
            matches = APP_ALIASES[query].orEmpty().mapNotNull { packageName ->
                context.packageManager.getLaunchIntentForPackage(packageName)?.let { launchIntent ->
                    LaunchableApp(packageName, requested.replaceFirstChar { it.uppercase() }, launchIntent)
                }
            }
        }
        if (matches.isEmpty()) {
            val suggestions = apps
                .sortedBy { app -> app.score(requested) }
                .take(5)
                .map { it.label }
            val data = JSONObject().put("requested", requested)
            val array = org.json.JSONArray()
            suggestions.forEach { array.put(it) }
            data.put("suggestions", array)
            return ActionResult(
                false,
                if (suggestions.isEmpty()) "I could not find an app named $requested"
                else "I could not find $requested. Try: ${suggestions.joinToString(", ")}",
                data,
            )
        }
        if (matches.size > 1 && matches.first().score(requested) == matches[1].score(requested)) {
            val choices = matches.take(5)
            val array = org.json.JSONArray()
            choices.forEach { app ->
                array.put(JSONObject().put("package", app.packageName).put("label", app.label))
            }
            return ActionResult(
                false,
                "More than one app matches $requested: ${choices.joinToString(", ") { it.label }}",
                JSONObject().put("matches", array),
            )
        }

        val selected = matches.first()
        val intent = selected.launchIntent
            ?: return ActionResult(false, "${selected.label} is installed but has no launchable activity")
        context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED))
        return ActionResult(
            true,
            "Opened ${selected.label}",
            JSONObject().put("package", selected.packageName).put("label", selected.label),
        )
    }

    private fun listApps(): ActionResult {
        val apps = installedApps()
            .map { app -> JSONObject().put("package", app.packageName).put("label", app.label) }
        val array = org.json.JSONArray()
        apps.forEach { array.put(it) }
        return ActionResult(true, "Found ${apps.size} launchable apps", JSONObject().put("apps", array))
    }

    private data class LaunchableApp(
        val packageName: String,
        val label: String,
        val launchIntent: Intent?,
    ) {
        fun score(raw: String): Int {
            val query = normalizeAppName(raw)
            val packageValue = normalizeAppName(packageName)
            val labelValue = normalizeAppName(label)
            return when {
                query == packageValue || query == labelValue -> 0
                packageName.equals(raw.trim(), ignoreCase = true) -> 0
                labelValue.startsWith(query) || packageValue.endsWith(query) -> 1
                labelValue.contains(query) || packageValue.contains(query) -> 2
                else -> 3
            }
        }
    }

    private fun launchableApps(): List<LaunchableApp> {
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        // Launcher activities such as WhatsApp and Instagram are not always
        // marked with CATEGORY_DEFAULT. MATCH_DEFAULT_ONLY silently excluded
        // them even though Android's launcher can start them.
        return context.packageManager.queryIntentActivities(intent, 0)
            .map { info ->
                val packageName = info.activityInfo.packageName
                LaunchableApp(
                    packageName = packageName,
                    label = info.loadLabel(context.packageManager).toString(),
                    launchIntent = context.packageManager.getLaunchIntentForPackage(packageName)
                        ?: Intent(intent).setPackage(packageName).setClassName(packageName, info.activityInfo.name),
                )
            }
            .distinctBy { it.packageName }
    }

    private fun resolveApp(requested: String, apps: List<LaunchableApp>): List<LaunchableApp> {
        val query = normalizeAppName(requested)
        val aliasPackages = APP_ALIASES[query].orEmpty()
        val aliases = apps.filter { it.packageName in aliasPackages }
        if (aliases.isNotEmpty()) return aliases.sortedBy { it.score(requested) }

        val matching = apps.filter { app ->
            val label = normalizeAppName(app.label)
            val packageName = normalizeAppName(app.packageName)
            query == label || query == packageName ||
                label.startsWith(query) || label.contains(query) ||
                packageName.endsWith(query) || packageName.contains(query)
        }
        return matching.sortedWith(compareBy<LaunchableApp> { it.score(requested) }.thenBy { it.label.lowercase() })
    }

    private fun firstArgument(args: JSONObject, vararg names: String): String =
        names.asSequence().map { args.optString(it).trim() }.firstOrNull { it.isNotEmpty() }.orEmpty()

    private companion object {
        // Common spoken names. Packages are filtered against the actual installed launcher list.
        val APP_ALIASES = mapOf(
            "youtube" to setOf("com.google.android.youtube"),
            "chrome" to setOf("com.android.chrome", "com.google.android.apps.chrome"),
            "brave" to setOf("com.brave.browser"),
            "whatsapp" to setOf("com.whatsapp"),
            "telegram" to setOf("org.telegram.messenger"),
            "instagram" to setOf("com.instagram.android"),
            "facebook" to setOf("com.facebook.katana"),
            "gmail" to setOf("com.google.android.gm"),
            "maps" to setOf("com.google.android.apps.maps"),
            "googlemaps" to setOf("com.google.android.apps.maps"),
            "spotify" to setOf("com.spotify.music"),
            "clock" to setOf("com.google.android.deskclock"),
            "calculator" to setOf("com.google.android.calculator"),
        )
        val SENSITIVE_CLICK_WORDS = setOf("send", "pay", "buy", "purchase", "delete", "uninstall", "transfer", "place order", "post", "publish")
    }

    private fun openUrl(raw: String): ActionResult {
        val uri = Uri.parse(raw).let { if (it.scheme == null) Uri.parse("https://$raw") else it }
        require(uri.scheme == "https" || uri.scheme == "http") { "Only http and https links are supported" }
        context.startActivity(Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Opened link")
    }

    private fun webSearch(query: String): ActionResult {
        context.startActivity(Intent(Intent.ACTION_WEB_SEARCH).putExtra(SearchManager.QUERY, query).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Searching for $query")
    }

    private fun setTimer(seconds: Int, label: String): ActionResult {
        require(seconds in 1..86_400) { "Timer must be between one second and one day" }
        context.startActivity(Intent(AlarmClock.ACTION_SET_TIMER)
            .putExtra(AlarmClock.EXTRA_LENGTH, seconds)
            .putExtra(AlarmClock.EXTRA_MESSAGE, label)
            .putExtra(AlarmClock.EXTRA_SKIP_UI, true)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Timer set for $seconds seconds")
    }

    private fun setAlarm(hour: Int, minute: Int, label: String): ActionResult {
        require(hour in 0..23 && minute in 0..59) { "Invalid alarm time" }
        context.startActivity(Intent(AlarmClock.ACTION_SET_ALARM)
            .putExtra(AlarmClock.EXTRA_HOUR, hour)
            .putExtra(AlarmClock.EXTRA_MINUTES, minute)
            .putExtra(AlarmClock.EXTRA_MESSAGE, label)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Alarm created")
    }

    private fun global(action: Int, label: String) = accessibility().performGlobalAction(action).asResult("Opened $label")

    private fun openSettings(action: String): ActionResult {
        val resolved = when (action.lowercase()) {
            "", "main", "settings" -> Settings.ACTION_SETTINGS
            "wifi", "wi-fi" -> Settings.ACTION_WIFI_SETTINGS
            "bluetooth" -> Settings.ACTION_BLUETOOTH_SETTINGS
            "sound", "volume" -> Settings.ACTION_SOUND_SETTINGS
            "display", "screen" -> Settings.ACTION_DISPLAY_SETTINGS
            else -> action
        }
        val allowed = setOf(
            Settings.ACTION_SETTINGS,
            Settings.ACTION_WIFI_SETTINGS,
            Settings.ACTION_BLUETOOTH_SETTINGS,
            Settings.ACTION_SOUND_SETTINGS,
            Settings.ACTION_DISPLAY_SETTINGS,
        )
        require(resolved in allowed) { "That settings page is not allowed" }
        context.startActivity(Intent(resolved).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Opened settings")
    }

    private fun snapshot(): ActionResult {
        val value = accessibility().snapshotJson()
            ?: return ActionResult(false, "No active accessibility window")
        return ActionResult(true, "Captured semantic UI", JSONObject().put("tree", value))
    }

    private fun mediaKey(code: Int): ActionResult {
        val audio = context.getSystemService(AudioManager::class.java)
        audio.dispatchMediaKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, code))
        audio.dispatchMediaKeyEvent(KeyEvent(KeyEvent.ACTION_UP, code))
        return ActionResult(true, "Media control sent")
    }

    private fun volume(direction: Int): ActionResult {
        context.getSystemService(AudioManager::class.java)
            .adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, AudioManager.FLAG_SHOW_UI)
        return ActionResult(true, "Volume changed")
    }

    private fun deviceStatus(): ActionResult {
        val battery = context.getSystemService(BatteryManager::class.java)
            .getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        return ActionResult(true, "Device status", JSONObject()
            .put("battery_percent", battery)
            .put("accessibility", HyuskAccessibilityService.connected.value)
            .put("airplane_mode", Settings.Global.getInt(context.contentResolver, Settings.Global.AIRPLANE_MODE_ON, 0) != 0))
    }

    private fun launchIntent(intent: Intent, message: String): ActionResult {
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (intent.resolveActivity(context.packageManager) == null) return ActionResult(false, "No app can handle that action")
        context.startActivity(intent)
        return ActionResult(true, message)
    }

    private fun dial(phone: String): ActionResult {
        require(phone.isNotBlank()) { "A phone number is required" }
        return launchIntent(Intent(Intent.ACTION_DIAL, Uri.parse("tel:${Uri.encode(phone)}")), "Opened dialer for $phone")
    }

    private fun composeSms(phone: String, text: String): ActionResult {
        require(phone.isNotBlank()) { "A phone number is required" }
        return launchIntent(Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:${Uri.encode(phone)}")).putExtra("sms_body", text), "Opened message composer")
    }

    private fun shareText(text: String): ActionResult {
        require(text.isNotBlank()) { "Text to share is required" }
        val share = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
        context.startActivity(Intent.createChooser(share, "Share with").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        return ActionResult(true, "Opened share sheet")
    }

    private fun writeClipboard(text: String): ActionResult {
        require(text.isNotBlank()) { "Clipboard text is required" }
        context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Hyusk", text))
        return ActionResult(true, "Copied to clipboard")
    }

    private fun accessibility(): HyuskAccessibilityService =
        HyuskAccessibilityService.instance ?: error("Hyusk Accessibility is not enabled")

    private fun Boolean.asResult(message: String) =
        if (this) ActionResult(true, message) else ActionResult(false, "$message failed")
}
