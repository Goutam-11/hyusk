package io.github.hyusk.mobile.voice

import android.speech.tts.TextToSpeech
import java.net.URI
import java.util.Locale

object HyuskSpeech {
    private val markdownLink = Regex("\\[([^]]+)]\\((https?://[^)]+)\\)")
    private val url = Regex("https?://[^\\s)]+", RegexOption.IGNORE_CASE)
    private val clockTime = Regex("\\b([01]?\\d|2[0-3]):([0-5]\\d)\\b")
    private val rupees = Regex("₹\\s*([0-9][0-9,]*(?:\\.[0-9]+)?)")
    private val dollars = Regex("\\$\\s*([0-9][0-9,]*(?:\\.[0-9]+)?)")
    private val percent = Regex("([0-9]+(?:\\.[0-9]+)?)%")

    fun configure(engine: TextToSpeech) {
        val locale = Locale.forLanguageTag("en-IN")
        engine.language = locale
        engine.voices.orEmpty()
            .filter { it.locale.language == "en" && it.locale.country == "IN" }
            .sortedWith(
                compareByDescending<android.speech.tts.Voice> { !it.isNetworkConnectionRequired }
                    .thenByDescending { it.name.contains("male", true) }
                    .thenByDescending { it.quality },
            )
            .firstOrNull()
            ?.let { engine.voice = it }
        engine.setPitch(0.94f)
        engine.setSpeechRate(0.96f)
    }

    fun normalize(raw: CharSequence): String {
        var text = raw.toString()
            .replace(Regex("```[\\s\\S]*?```"), " Code block omitted. ")
            .replace(markdownLink) { it.groupValues[1] }
            .replace(url) { match -> readableLink(match.value) }
            .replace(rupees) { "${it.groupValues[1]} rupees" }
            .replace(dollars) { "${it.groupValues[1]} dollars" }
            .replace(percent) { "${it.groupValues[1]} percent" }
            .replace(clockTime) { match ->
                val hour = match.groupValues[1].toInt()
                val minute = match.groupValues[2].toInt()
                when {
                    minute == 0 -> "$hour o'clock"
                    minute < 10 -> "$hour oh $minute"
                    else -> "$hour $minute"
                }
            }
            .replace("&", " and ")
            .replace(Regex("(?m)^\\s*[-*•]+\\s*"), ". ")
            .replace(Regex("[*_#`~]"), "")
            .replace(Regex("\\s+"), " ")
            .trim()
        if (text.length > 1_500) text = text.take(1_497).trimEnd() + "..."
        return text
    }

    private fun readableLink(raw: String): String = runCatching {
        val uri = URI(raw)
        val host = uri.host?.removePrefix("www.") ?: return@runCatching "the provided link"
        "the link on ${host.replace('.', ' ')}"
    }.getOrDefault("the provided link")
}
