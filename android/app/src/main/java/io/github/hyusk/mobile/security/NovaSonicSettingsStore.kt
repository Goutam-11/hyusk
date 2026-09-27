package io.github.hyusk.mobile.security

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.novaSonicDataStore by preferencesDataStore("nova_sonic_settings")

data class NovaSonicSettings(
    val enabled: Boolean = false,
    val region: String = "us-east-1",
    val modelId: String = "amazon.nova-2-sonic-v1:0",
    val accessKeyId: String = "",
    val secretAccessKey: String = "",
)

/** IAM credentials never enter logs, the model catalog, or the laptop pairing channel. */
class NovaSonicSettingsStore(context: Context) {
    private val appContext = context.applicationContext
    private val envelope = EncryptedPayload(appContext)
    private val region = stringPreferencesKey("region")
    private val enabled = booleanPreferencesKey("enabled")
    private val model = stringPreferencesKey("model")
    private val accessKey = stringPreferencesKey("access_key")
    private val secretKey = stringPreferencesKey("secret_key")

    val settings: Flow<NovaSonicSettings> = appContext.novaSonicDataStore.data.map { prefs ->
        NovaSonicSettings(
            enabled = prefs[enabled] ?: false,
            region = prefs[region] ?: "us-east-1",
            modelId = prefs[model] ?: "amazon.nova-2-sonic-v1:0",
            accessKeyId = prefs[accessKey]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
            secretAccessKey = prefs[secretKey]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
        )
    }

    suspend fun save(value: NovaSonicSettings) {
        require(value.region.matches(Regex("[a-z]{2}-[a-z]+-\\d+"))) { "Enter an AWS region, such as us-east-1" }
        require(value.modelId.startsWith("amazon.nova") && value.modelId.contains("sonic")) { "Choose a Nova Sonic model ID" }
        appContext.novaSonicDataStore.edit { prefs ->
            prefs[enabled] = value.enabled
            prefs[region] = value.region.trim()
            prefs[model] = value.modelId.trim()
            prefs[accessKey] = envelope.encrypt(value.accessKeyId.trim())
            prefs[secretKey] = envelope.encrypt(value.secretAccessKey.trim())
        }
    }
}
