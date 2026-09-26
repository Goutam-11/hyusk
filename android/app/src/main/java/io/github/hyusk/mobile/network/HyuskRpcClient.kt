package io.github.hyusk.mobile.network

import android.util.Base64
import io.github.hyusk.mobile.security.ProviderSettings
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.UUID
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager

sealed interface PairingState {
    data object Disconnected : PairingState
    data object Connecting : PairingState
    data object Authenticating : PairingState
    data class Paired(val deviceId: String) : PairingState
    data class Failed(val message: String) : PairingState
}

data class RpcResponse(val id: Long, val result: JSONObject?, val error: JSONObject?)

/** JSON-RPC v1 client with certificate pinning, challenge auth, and replay-safe MACs. */
class HyuskRpcClient {
    private val ids = AtomicLong(1)
    private val sequence = AtomicLong(0)
    private val _state = MutableStateFlow<PairingState>(PairingState.Disconnected)
    val state: StateFlow<PairingState> = _state
    private val _notifications = MutableSharedFlow<JSONObject>(extraBufferCapacity = 32)
    val notifications: SharedFlow<JSONObject> = _notifications
    private var socket: WebSocket? = null
    private var settings: ProviderSettings? = null
    private var reconnectAttempt = 0
    private var closedByUser = false
    private val heartbeatHandler = android.os.Handler(android.os.Looper.getMainLooper())
    private val heartbeat = object : Runnable {
        override fun run() {
            if (_state.value is PairingState.Paired) runCatching { call("device.ping") }
            if (!closedByUser) heartbeatHandler.postDelayed(this, HEARTBEAT_MILLIS)
        }
    }

    fun connect(settings: ProviderSettings) {
        val uri = java.net.URI(settings.endpoint.trim())
        require(uri.scheme == "wss" || (uri.scheme == "ws" && (uri.host == "127.0.0.1" || uri.host == "localhost"))) {
            "Hyusk requires wss:// outside localhost"
        }
        require(settings.deviceId.isNotBlank() && settings.deviceSecret.isNotBlank()) { "Device identity is missing" }
        if (uri.scheme == "wss") require(settings.tlsFingerprint.isNotBlank()) { "The pairing payload has no TLS fingerprint" }
        closeSocket("reconnecting")
        this.settings = settings
        closedByUser = false
        _state.value = PairingState.Connecting
        socket = pinnedClient(settings.tlsFingerprint).newWebSocket(Request.Builder().url(settings.endpoint).build(), listener)
    }

    fun submit(text: String): Long = call("turn.submit", JSONObject()
        .put("turn_id", UUID.randomUUID().toString())
        .put("text", text)
        .put("source_device", settings?.deviceId)
        .put("target_device", "laptop")
        .put("timeout_ms", 600_000))

    fun call(method: String, params: JSONObject = JSONObject()): Long {
        check(_state.value is PairingState.Paired) { "Hyusk is not authenticated" }
        val id = ids.incrementAndGet()
        val seq = sequence.incrementAndGet()
        val secret = decode(settings!!.deviceSecret)
        val canonicalParams = canonicalObject(params)
        val signing = JSONArray().put("2.0").put(id).put(method).put(canonicalParams).put(seq).toString()
        val mac = hmac(secret, signing.toByteArray(StandardCharsets.UTF_8) + "request".toByteArray())
        val request = JSONObject()
            .put("jsonrpc", "2.0").put("id", id).put("method", method).put("params", canonicalParams)
            .put("auth", JSONObject().put("sequence", seq).put("mac", encode(mac)))
        check(socket?.send(request.toString()) == true) { "Hyusk connection closed" }
        return id
    }

    fun close() {
        closedByUser = true
        heartbeatHandler.removeCallbacks(heartbeat)
        closeSocket("client closed")
        _state.value = PairingState.Disconnected
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            reconnectAttempt = 0
            _state.value = PairingState.Authenticating
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            runCatching {
                val message = JSONObject(text)
                when {
                    message.optString("method") == "auth.challenge" -> sendHello(webSocket, message.getJSONObject("params"))
                    message.has("method") -> _notifications.tryEmit(message)
                    else -> {
                        val response = RpcResponse(message.optLong("id"), message.optJSONObject("result"), message.optJSONObject("error"))
                        if (response.id == 1L && response.error == null) {
                            _state.value = PairingState.Paired(settings!!.deviceId)
                            heartbeatHandler.removeCallbacks(heartbeat)
                            heartbeatHandler.postDelayed(heartbeat, HEARTBEAT_MILLIS)
                        }
                        if (response.error != null) _notifications.tryEmit(JSONObject().put("method", "rpc.error").put("params", response.error))
                    }
                }
            }.onFailure { _state.value = PairingState.Failed(it.message ?: "Invalid provider response") }
        }

        override fun onFailure(webSocket: WebSocket, failure: Throwable, response: Response?) {
            if (socket !== webSocket) return
            _state.value = PairingState.Failed(failure.message ?: "Connection failed")
            scheduleReconnect()
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            if (socket !== webSocket) return
            // Never leave the UI in Paired after the socket has gone away. This
            // also makes callers fall back to the local phone agent immediately
            // while the background reconnect is in progress.
            _state.value = if (closedByUser) PairingState.Disconnected
            else PairingState.Failed(if (reason.isBlank()) "Laptop connection closed" else reason)
            if (!closedByUser) scheduleReconnect()
        }
    }

    private fun sendHello(webSocket: WebSocket, params: JSONObject) {
        val config = settings ?: error("Pairing settings disappeared")
        require(params.getString("protocol") == "hyusk.link.v1") { "Unsupported Hyusk protocol" }
        val challengeText = params.getString("challenge")
        val proof = hmac(decode(config.deviceSecret), decode(challengeText) + config.deviceId.toByteArray())
        val hello = JSONObject()
            .put("protocol", "hyusk.link.v1")
            .put("device_id", config.deviceId)
            .put("device_name", android.os.Build.MODEL)
            .put("capabilities", JSONObject().put("audio_input", true).put("notifications", true)
                .put("approvals", true).put("memory_sync", true).put("workflow_sync", true))
            .put("challenge", challengeText).put("proof", encode(proof)).put("device_secret", config.deviceSecret)
        if (config.pairingSecret.isNotBlank()) hello.put("pairing_secret", config.pairingSecret)
        val request = JSONObject().put("jsonrpc", "2.0").put("id", 1).put("method", "device.hello").put("params", hello)
        check(webSocket.send(request.toString())) { "Could not send device hello" }
    }

    private fun scheduleReconnect() {
        val config = settings ?: return
        if (closedByUser) return
        val delay = (1L shl reconnectAttempt.coerceAtMost(5)).coerceAtMost(30)
        reconnectAttempt++
        Thread {
            try { TimeUnit.SECONDS.sleep(delay) } catch (_: InterruptedException) { return@Thread }
            if (!closedByUser) connect(config)
        }.start()
    }

    private fun closeSocket(reason: String) {
        socket?.close(1000, reason)
        socket = null
        sequence.set(0)
    }

    private fun pinnedClient(fingerprint: String): OkHttpClient {
        if (fingerprint.isBlank()) return OkHttpClient()
        val expected = fingerprint.replace(":", "").lowercase()
        val trust = object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
            override fun checkClientTrusted(chain: Array<X509Certificate>?, authType: String?) = Unit
            override fun checkServerTrusted(chain: Array<X509Certificate>?, authType: String?) {
                val cert = chain?.firstOrNull() ?: throw java.security.cert.CertificateException("No server certificate")
                val actual = MessageDigest.getInstance("SHA-256").digest(cert.encoded).joinToString("") { "%02x".format(it) }
                if (!MessageDigest.isEqual(actual.toByteArray(), expected.toByteArray())) {
                    throw java.security.cert.CertificateException("Hyusk certificate fingerprint changed")
                }
            }
        }
        val context = SSLContext.getInstance("TLS").apply { init(null, arrayOf<TrustManager>(trust), SecureRandom()) }
        return OkHttpClient.Builder().sslSocketFactory(context.socketFactory, trust)
            .hostnameVerifier { _, _ -> true }.build() // The exact full-certificate pin is the identity check.
    }

    private fun canonicalObject(value: JSONObject): JSONObject {
        val result = JSONObject()
        value.keys().asSequence().toList().sorted().forEach { key -> result.put(key, canonicalValue(value.get(key))) }
        return result
    }

    private fun canonicalValue(value: Any?): Any? = when (value) {
        is JSONObject -> canonicalObject(value)
        is JSONArray -> JSONArray().also { output -> for (index in 0 until value.length()) output.put(canonicalValue(value.get(index))) }
        else -> value
    }

    companion object {
        private const val HEARTBEAT_MILLIS = 30_000L
        fun newDeviceSecret(): String = ByteArray(32).also(SecureRandom()::nextBytes).let(::encode)
        private fun hmac(secret: ByteArray, value: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
            init(SecretKeySpec(secret, "HmacSHA256")); doFinal(value)
        }
        private fun encode(value: ByteArray): String = Base64.encodeToString(value, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        private fun decode(value: String): ByteArray = Base64.decode(value, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
    }
}
