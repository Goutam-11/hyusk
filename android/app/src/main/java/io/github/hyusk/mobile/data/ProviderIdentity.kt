package io.github.hyusk.mobile.data

import io.github.hyusk.mobile.security.LocalModelSettings
import java.net.URI
import java.security.MessageDigest

/** Stable, non-secret identity used to scope provider-owned cached data. */
object ProviderIdentity {
    /** Stable key for a provider's saved credentials and model selection. */
    fun profileKey(raw: String): String {
        val key = raw.trim().lowercase()
        require(key.matches(Regex("[a-z0-9][a-z0-9._-]{0,63}"))) { "Invalid provider profile ID" }
        return key
    }

    fun normalizeBaseUrl(raw: String): String {
        val base = raw.trim().trimEnd('/')
        val uri = runCatching { URI(base) }.getOrNull()
            ?: throw IllegalArgumentException("Provider URL must be a valid absolute URL")
        val scheme = uri.scheme?.lowercase()
        val host = uri.host?.lowercase()?.removePrefix("[")?.removeSuffix("]")
        require(uri.isAbsolute && host != null && uri.rawUserInfo == null && uri.rawQuery == null && uri.rawFragment == null) {
            "Provider URL must be an absolute URL without credentials, query, or fragment"
        }
        require(scheme == "https" || (scheme == "http" && host in LOOPBACK_HOSTS)) {
            "Provider URLs must use https:// (http:// is allowed only for localhost development)"
        }
        require(uri.port == -1 || uri.port in 1..65535) { "Provider URL has an invalid port" }
        return base
    }

    fun normalizeApiKey(raw: String): String =
        raw.trim().replace(Regex("^Bearer\\s+", RegexOption.IGNORE_CASE), "").trim()

    /** A one-way provider profile key; the API key itself is never written to the catalog cache. */
    fun cacheKey(settings: LocalModelSettings): String {
        val identity = "${normalizeBaseUrl(settings.baseUrl)}\u0000${normalizeApiKey(settings.apiKey)}"
        val digest = MessageDigest.getInstance("SHA-256").digest(identity.toByteArray(Charsets.UTF_8))
        return digest.joinToString("") { "%02x".format(it) }
    }

    private val LOOPBACK_HOSTS = setOf("localhost", "127.0.0.1", "::1")
}
