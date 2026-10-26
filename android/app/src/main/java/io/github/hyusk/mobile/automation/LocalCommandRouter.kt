package io.github.hyusk.mobile.automation

import android.content.Context
import org.json.JSONObject

internal fun mediaCommandAction(text: String): String? = when (
    text.lowercase().trim().replace(Regex("\\s+"), " ")
) {
    "play", "resume", "resume music", "play music", "play some music", "play a song", "play song", "play songs" -> "media.play"
    "pause", "pause music", "pause the music", "stop music", "stop the music" -> "media.pause"
    "play pause", "toggle music" -> "media.play_pause"
    "next song", "next track", "skip song", "skip track" -> "media.next"
    "previous song", "previous track", "back a song" -> "media.previous"
    "is music playing", "is anything playing", "check music playback" -> "media.status"
    else -> null
}

/** Fast, LLM-free phone vocabulary for common commands. */
class LocalCommandRouter(context: Context) {
    private val executor = DeviceActionExecutor(context)

    fun execute(text: String): ActionResult? {
        val command = text.trim()
        mediaCommandAction(command)?.let { return executor.execute(it, JSONObject()) }
        if (command.equals("go back", true) || command.equals("back", true)) return executor.execute("system.back", JSONObject())
        if (command.equals("go home", true) || command.equals("home", true)) return executor.execute("system.home", JSONObject())
        if (command.equals("show notifications", true)) return executor.execute("system.notifications", JSONObject())
        if (command.equals("open quick settings", true)) return executor.execute("system.quick_settings", JSONObject())
        if (command.equals("show recent apps", true) || command.equals("open recents", true)) return executor.execute("system.recents", JSONObject())
        if (command.equals("volume up", true) || command.equals("increase volume", true)) return executor.execute("volume.up", JSONObject())
        if (command.equals("volume down", true) || command.equals("decrease volume", true)) return executor.execute("volume.down", JSONObject())
        Regex("^(?:search(?: the)? web for|google)\\s+(.+)$", RegexOption.IGNORE_CASE).find(command)?.let {
            return executor.execute("web.search", JSONObject().put("query", it.groupValues[1]))
        }
        Regex("^(?:dial|call)\\s+([+0-9][0-9 -]+)$", RegexOption.IGNORE_CASE).find(command)?.let {
            return executor.execute("phone.dial", JSONObject().put("phone", it.groupValues[1]))
        }
        Regex("^(?:open|go to)\\s+(https?://\\S+|[\\w.-]+\\.[a-z]{2,})$", RegexOption.IGNORE_CASE).find(command)?.let {
            return executor.execute("url.open", JSONObject().put("url", it.groupValues[1]))
        }
        Regex("^(?:open|launch|start)\\s+(?:the\\s+)?(.+?)(?:\\s+app)?$", RegexOption.IGNORE_CASE).find(command)?.let {
            return executor.execute("apps.launch", JSONObject().put("app", it.groupValues[1].trim()))
        }
        Regex("^(?:set )?timer for (\\d+)\\s*(seconds?|minutes?|hours?)$", RegexOption.IGNORE_CASE).find(command)?.let {
            val amount = it.groupValues[1].toInt()
            val seconds = when (it.groupValues[2].lowercase()) {
                "hour", "hours" -> amount * 3_600
                "minute", "minutes" -> amount * 60
                else -> amount
            }
            return executor.execute("timer.create", JSONObject().put("seconds", seconds).put("label", "Hyusk timer"))
        }
        if (command.equals("open camera", true) || command.equals("take a photo", true)) return executor.execute("camera.open", JSONObject())
        return null
    }
}
