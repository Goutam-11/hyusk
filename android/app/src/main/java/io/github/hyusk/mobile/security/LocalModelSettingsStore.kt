package io.github.hyusk.mobile.security

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import io.github.hyusk.mobile.data.ModelCatalogItem
import io.github.hyusk.mobile.data.ProviderIdentity
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import org.json.JSONArray
import org.json.JSONObject

private val Context.localModelDataStore by preferencesDataStore("local_model_settings")

data class LocalModelSettings(
    val baseUrl: String = "https://openrouter.ai/api/v1",
    val model: String = "openai/gpt-4o-mini",
    val apiKey: String = "",
)

data class CachedModelCatalog(
    val providerKey: String,
    val endpoint: String,
    val fetchedAtMillis: Long,
    val models: List<ModelCatalogItem>,
)

class LocalModelSettingsStore(private val context: Context) {
    private val envelope = EncryptedPayload(context)
    private val baseUrl = stringPreferencesKey("base_url")
    private val model = stringPreferencesKey("model")
    private val apiKey = stringPreferencesKey("api_key")
    private val modelCatalog = stringPreferencesKey("model_catalog")
    private val providerProfiles = stringPreferencesKey("provider_profiles")
    val settings: Flow<LocalModelSettings> = context.localModelDataStore.data.map { prefs ->
        LocalModelSettings(
            prefs[baseUrl]?.let { runCatching { envelope.decrypt(it) }.getOrNull() } ?: "https://openrouter.ai/api/v1",
            prefs[model]?.let { runCatching { envelope.decrypt(it) }.getOrNull() } ?: "openai/gpt-4o-mini",
            prefs[apiKey]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
        )
    }
    suspend fun save(value: LocalModelSettings) { context.localModelDataStore.edit { prefs ->
        prefs[baseUrl] = envelope.encrypt(value.baseUrl.trim().trimEnd('/'))
        prefs[model] = envelope.encrypt(value.model.trim())
        prefs[apiKey] = envelope.encrypt(value.apiKey.trim())
    } }

    /** Loads one provider's encrypted settings, falling back to the supplied provider defaults. */
    suspend fun loadProfile(profileId: String, defaults: LocalModelSettings): LocalModelSettings {
        val key = ProviderIdentity.profileKey(profileId)
        val prefs = context.localModelDataStore.data.first()
        val profiles = prefs[providerProfiles]
            ?.let { runCatching { parseProfiles(envelope.decrypt(it)) }.getOrDefault(emptyMap()) }
            .orEmpty()
        profiles[key]?.let { saved ->
            saveProfile(key, saved)
            return saved
        }

        // Migrate the existing single-profile values only when they belong to this provider.
        val legacy = LocalModelSettings(
            prefs[baseUrl]?.let { runCatching { envelope.decrypt(it) }.getOrNull() } ?: "https://openrouter.ai/api/v1",
            prefs[model]?.let { runCatching { envelope.decrypt(it) }.getOrNull() } ?: "openai/gpt-4o-mini",
            prefs[apiKey]?.let { runCatching { envelope.decrypt(it) }.getOrNull() }.orEmpty(),
        )
        if (sameEndpoint(legacy.baseUrl, defaults.baseUrl)) {
            saveProfile(key, legacy)
            return legacy
        }
        saveProfile(key, defaults)
        return defaults
    }

    /** Saves provider-specific settings encrypted at rest and updates the active legacy values. */
    suspend fun saveProfile(profileId: String, value: LocalModelSettings) {
        val key = ProviderIdentity.profileKey(profileId)
        context.localModelDataStore.edit { prefs ->
            val profiles = prefs[providerProfiles]
                ?.let { runCatching { parseProfiles(envelope.decrypt(it)) }.getOrDefault(emptyMap()) }
                .orEmpty()
            val updated = (profiles + (key to value)).toList().takeLast(MAX_SAVED_PROFILES).toMap()
            prefs[providerProfiles] = envelope.encrypt(serializeProfiles(updated))
            prefs[baseUrl] = envelope.encrypt(value.baseUrl.trim().trimEnd('/'))
            prefs[model] = envelope.encrypt(value.model.trim())
            prefs[apiKey] = envelope.encrypt(value.apiKey.trim())
        }
    }

    private fun sameEndpoint(first: String, second: String): Boolean =
        runCatching { ProviderIdentity.normalizeBaseUrl(first).equals(ProviderIdentity.normalizeBaseUrl(second), ignoreCase = true) }
            .getOrDefault(false)

    private fun parseProfiles(raw: String): Map<String, LocalModelSettings> = runCatching {
        val entries = JSONObject(raw).optJSONArray("entries") ?: JSONArray()
        buildMap {
            for (index in 0 until entries.length()) {
                val entry = entries.optJSONObject(index) ?: continue
                val id = runCatching { ProviderIdentity.profileKey(entry.optString("id")) }.getOrNull() ?: continue
                put(id, LocalModelSettings(
                    entry.optString("baseUrl"), entry.optString("model"), entry.optString("apiKey"),
                ))
            }
        }
    }.getOrDefault(emptyMap())

    private fun serializeProfiles(profiles: Map<String, LocalModelSettings>): String = JSONObject().apply {
        put("version", 1)
        put("entries", JSONArray().apply {
            profiles.forEach { (id, settings) -> put(JSONObject().apply {
                put("id", id)
                put("baseUrl", settings.baseUrl)
                put("model", settings.model)
                put("apiKey", settings.apiKey)
            }) }
        })
    }.toString()

    /** Returns only the catalog for this endpoint and API-key profile. */
    suspend fun readModelCatalog(settings: LocalModelSettings): CachedModelCatalog? {
        val key = runCatching { ProviderIdentity.cacheKey(settings) }.getOrNull() ?: return null
        val encrypted = context.localModelDataStore.data.first()[modelCatalog] ?: return null
        val raw = runCatching { envelope.decrypt(encrypted) }.getOrNull() ?: return null
        return parseCatalogs(raw).firstOrNull { it.providerKey == key }
    }

    /** Stores model IDs and metadata encrypted at rest, without persisting the API key. */
    suspend fun saveModelCatalog(
        settings: LocalModelSettings,
        models: List<ModelCatalogItem>,
        fetchedAtMillis: Long = System.currentTimeMillis(),
    ) {
        val key = ProviderIdentity.cacheKey(settings)
        val value = CachedModelCatalog(key, ProviderIdentity.normalizeBaseUrl(settings.baseUrl), fetchedAtMillis, models)
        context.localModelDataStore.edit { prefs ->
            val current = prefs[modelCatalog]
                ?.let { runCatching { parseCatalogs(envelope.decrypt(it)) }.getOrDefault(emptyList()) }
                .orEmpty()
            val entries = (current.filterNot { it.providerKey == key } + value).takeLast(MAX_CACHED_PROVIDERS)
            prefs[modelCatalog] = envelope.encrypt(serializeCatalogs(entries))
        }
    }

    private fun parseCatalogs(raw: String): List<CachedModelCatalog> = runCatching {
        val entries = JSONObject(raw).optJSONArray("entries") ?: JSONArray()
        buildList {
            for (index in 0 until entries.length()) {
                val entry = entries.optJSONObject(index) ?: continue
                val providerKey = entry.optString("providerKey").trim()
                val endpoint = entry.optString("endpoint").trim()
                val fetchedAt = entry.optLong("fetchedAtMillis", 0L)
                if (providerKey.isBlank() || endpoint.isBlank() || fetchedAt <= 0L) continue
                val models = entry.optJSONArray("models") ?: JSONArray()
                add(CachedModelCatalog(providerKey, endpoint, fetchedAt, buildList {
                    for (modelIndex in 0 until models.length()) {
                        val model = models.optJSONObject(modelIndex) ?: continue
                        val id = model.optString("id").trim()
                        if (id.isNotBlank()) {
                            add(ModelCatalogItem(id, model.optString("name").trim(), model.optString("ownedBy").trim()))
                        }
                    }
                }))
            }
        }
    }.getOrDefault(emptyList())

    private fun serializeCatalogs(entries: List<CachedModelCatalog>): String = JSONObject().apply {
        put("version", 1)
        put("entries", JSONArray().apply {
            entries.forEach { entry ->
                put(JSONObject().apply {
                    put("providerKey", entry.providerKey)
                    put("endpoint", entry.endpoint)
                    put("fetchedAtMillis", entry.fetchedAtMillis)
                    put("models", JSONArray().apply {
                        entry.models.forEach { model ->
                            put(JSONObject().apply {
                                put("id", model.id)
                                if (model.name.isNotBlank()) put("name", model.name)
                                if (model.ownedBy.isNotBlank()) put("ownedBy", model.ownedBy)
                            })
                        }
                    })
                })
            }
        })
    }.toString()

    private companion object {
        const val MAX_CACHED_PROVIDERS = 8
        const val MAX_SAVED_PROFILES = 8
    }
}
