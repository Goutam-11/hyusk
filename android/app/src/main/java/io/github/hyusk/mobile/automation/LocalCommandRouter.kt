package io.github.hyusk.mobile.automation

import android.content.Context
import org.json.JSONObject

/** Fast, LLM-free phone vocabulary for common commands. */
class LocalCommandRouter(context: Context) {
    private val executor = DeviceActionExecutor(context)

    fun execute(text: String): ActionResult? {
        val command = text.trim()
        if (command.equals("go back", true) || command.equals("back", true)) return executor.execute("system.back", JSONObject())
        if (command.equals("go home", true) || command.equals("home", true)) return executor.execute("system.home", JSONObject())
        if (command.equals("show notifications", true)) return executor.execute("system.notifications", JSONObject())
        if (command.equals("open quick settings", true)) return executor.execute("system.quick_settings", JSONObject())
        if (command.equals("show recent apps", true) || command.equals("open recents", true)) return executor.execute("system.recents", JSONObject())
        if (command.equals("volume up", true) || command.equals("increase volume", true)) return executor.execute("volume.up", JSONObject())
        if (command.equals("volume down", true) || command.equals("decrease volume", true)) return executor.execute("volume.down", JSONObject())
        if (command.equals("play", true) || command.equals("pause", true) || command.equals("play pause", true)) return executor.execute("media.play_pause", JSONObject())
        if (command.equals("next song", true) || command.equals("next track", true)) return executor.execute("media.next", JSONObject())
        if (command.equals("previous song", true) || command.equals("previous track", true)) return executor.execute("media.previous", JSONObject())
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
