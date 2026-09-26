package io.github.hyusk.mobile.network

import android.content.Context
import io.github.hyusk.mobile.data.ModelCatalogItem
import io.github.hyusk.mobile.security.CachedModelCatalog
import io.github.hyusk.mobile.security.LocalModelSettings
import io.github.hyusk.mobile.security.LocalModelSettingsStore
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

sealed interface ModelCatalogState {
    data object Idle : ModelCatalogState
    data class Loading(val previous: ModelCatalogSnapshot? = null) : ModelCatalogState
    data class Ready(val snapshot: ModelCatalogSnapshot) : ModelCatalogState
    data class Error(val message: String, val staleSnapshot: ModelCatalogSnapshot? = null) : ModelCatalogState
}

enum class ModelCatalogSource { NETWORK, CACHE, STALE_CACHE }

data class ModelCatalogSnapshot(
    val models: List<ModelCatalogItem>,
    val fetchedAtMillis: Long,
    val isStale: Boolean,
    val source: ModelCatalogSource,
)

/** Loads provider models with encrypted, provider-scoped caching and stale-cache fallback. */
class ModelCatalogRepository(
    context: Context,
    private val client: LocalModelClient = LocalModelClient(),
    private val now: () -> Long = System::currentTimeMillis,
) {
    private val store = LocalModelSettingsStore(context.applicationContext)
    private val mutableState = MutableStateFlow<ModelCatalogState>(ModelCatalogState.Idle)
    private var requestVersion = 0L
    val state: StateFlow<ModelCatalogState> = mutableState.asStateFlow()

    /** Removes any previous provider's models while another profile is selected. */
    fun clear() {
        requestVersion += 1
        mutableState.value = ModelCatalogState.Idle
    }

    /** Loads the cached catalog, refreshing automatically when absent or older than six hours. */
    suspend fun load(settings: LocalModelSettings): ModelCatalogSnapshot? {
        val request = ++requestVersion
        val cached = readCache(settings)
        val cachedSnapshot = cached?.snapshot(now())
        if (cachedSnapshot != null && !cachedSnapshot.isStale) {
            if (request == requestVersion) mutableState.value = ModelCatalogState.Ready(cachedSnapshot.copy(source = ModelCatalogSource.CACHE))
            return cachedSnapshot.copy(source = ModelCatalogSource.CACHE)
        }
        return refresh(settings, cachedSnapshot, request)
    }

    /** Explicitly bypasses a fresh cache and fetches the provider catalog. */
    suspend fun refresh(settings: LocalModelSettings): ModelCatalogSnapshot? {
        val request = ++requestVersion
        return refresh(settings, readCache(settings)?.snapshot(now()), request)
    }

    private suspend fun refresh(
        settings: LocalModelSettings,
        cachedSnapshot: ModelCatalogSnapshot?,
        request: Long,
    ): ModelCatalogSnapshot? {
        if (request == requestVersion) mutableState.value = ModelCatalogState.Loading(cachedSnapshot)
        return try {
            val models = client.listModels(settings)
            val fetchedAt = now()
            store.saveModelCatalog(settings, models, fetchedAt)
            ModelCatalogSnapshot(models, fetchedAt, isStale = false, source = ModelCatalogSource.NETWORK).also {
                if (request == requestVersion) mutableState.value = ModelCatalogState.Ready(it)
            }
        } catch (failure: CancellationException) {
            throw failure
        } catch (failure: Exception) {
            val stale = cachedSnapshot?.copy(isStale = true, source = ModelCatalogSource.STALE_CACHE)
            if (request == requestVersion) mutableState.value = ModelCatalogState.Error(
                failure.message ?: "Could not load models",
                stale,
            )
            stale
        }
    }

    private suspend fun readCache(settings: LocalModelSettings): CachedModelCatalog? = try {
        store.readModelCatalog(settings)
    } catch (_: Exception) {
        null
    }

    internal fun isFresh(fetchedAtMillis: Long, currentTimeMillis: Long = now()): Boolean =
        currentTimeMillis >= fetchedAtMillis && currentTimeMillis - fetchedAtMillis < CACHE_TTL_MILLIS

    private fun CachedModelCatalog.snapshot(currentTimeMillis: Long): ModelCatalogSnapshot = ModelCatalogSnapshot(
        models = models,
        fetchedAtMillis = fetchedAtMillis,
        isStale = !isFresh(fetchedAtMillis, currentTimeMillis),
        source = ModelCatalogSource.CACHE,
    )

    private companion object {
        val CACHE_TTL_MILLIS = TimeUnit.HOURS.toMillis(6)
    }
}
