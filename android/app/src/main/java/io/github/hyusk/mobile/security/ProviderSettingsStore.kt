package io.github.hyusk.mobile.security

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import io.github.hyusk.mobile.network.HyuskRpcClient
import org.json.JSONObject
import java.util.UUID

private val Context.providerDataStore by preferencesDataStore("provider_settings")

data class ProviderSettings(
    val endpoint: String = "",
    val pairingSecret: String = "",
    val tlsFingerprint: String = "",
    val deviceId: String = "",
    val deviceSecret: String = "",
    val pairingExpiresAt: Long = 0,
) {
    companion object {
        fun fromPairingPayload(payload: String, previous: ProviderSettings = ProviderSettings(), replaceIdentity: Boolean = false): ProviderSettings {
            // Accept either the raw payload or a journal/terminal line that
            // contains it, which is the most common copy/paste path on Fedora.
            val trimmed = payload.trim()
            val start = trimmed.indexOf('{')
            val end = trimmed.lastIndexOf('}')
            require(start >= 0 && end > start) { "Paste the complete pairing JSON printed by Hyusk on the laptop." }
            val json = JSONObject(trimmed.substring(start, end + 1))
            require(json.getString("protocol") == "hyusk.link.v1") { "Unsupported pairing payload" }
            val expiresAt = json.optLong("expires_at")
            require(expiresAt == 0L || expiresAt > System.currentTimeMillis() / 1_000) { "This pairing code expired. Generate a new one on the laptop." }
            return previous.copy(
                endpoint = json.getString("url"),
                pairingSecret = json.getString("secret"),
                tlsFingerprint = json.getString("tls_fingerprint"),
                deviceId = if (replaceIdentity || previous.deviceId.isBlank()) UUID.randomUUID().toString() else previous.deviceId,
                deviceSecret = if (replaceIdentity || previous.deviceSecret.isBlank()) HyuskRpcClient.newDeviceSecret() else previous.deviceSecret,
                pairingExpiresAt = expiresAt,
            )
        }
    }
}

class ProviderSettingsStore(private val context: Context) {
    private val envelope = EncryptedPayload(context)
    private val endpoint = stringPreferencesKey("endpoint")
    private val pairingSecret = stringPreferencesKey("pairing_secret")
    private val tlsFingerprint = stringPreferencesKey("tls_fingerprint")
    private val deviceId = stringPreferencesKey("device_id")
    private val deviceSecret = stringPreferencesKey("device_secret")
    private val pairingExpiresAt = stringPreferencesKey("pairing_expires_at")

    val settings: Flow<ProviderSettings> = context.providerDataStore.data.map { prefs ->
        ProviderSettings(
            endpoint = prefs[endpoint]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            pairingSecret = prefs[pairingSecret]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            tlsFingerprint = prefs[tlsFingerprint]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            deviceId = prefs[deviceId]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            deviceSecret = prefs[deviceSecret]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            pairingExpiresAt = prefs[pairingExpiresAt]?.let { runCatching { envelope.decrypt(it).toLong() }.getOrNull() } ?: 0,
        )
    }

    suspend fun save(settings: ProviderSettings) {
        context.providerDataStore.edit { prefs ->
            prefs[endpoint] = envelope.encrypt(settings.endpoint)
            prefs[pairingSecret] = envelope.encrypt(settings.pairingSecret)
            prefs[tlsFingerprint] = envelope.encrypt(settings.tlsFingerprint)
            prefs[deviceId] = envelope.encrypt(settings.deviceId)
            prefs[deviceSecret] = envelope.encrypt(settings.deviceSecret)
            prefs[pairingExpiresAt] = envelope.encrypt(settings.pairingExpiresAt.toString())
        }
    }

    suspend fun clear() {
        context.providerDataStore.edit { prefs ->
            prefs.remove(endpoint)
            prefs.remove(pairingSecret)
            prefs.remove(tlsFingerprint)
            prefs.remove(deviceId)
            prefs.remove(deviceSecret)
            prefs.remove(pairingExpiresAt)
        }
    }
}
