package com.aerolink.robot.signal

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit
import kotlin.random.Random

enum class SignalPhase { CONNECTING, ONLINE, OFFLINE, UNAUTHORIZED }

data class SignalState(val phase: SignalPhase = SignalPhase.CONNECTING, val error: String? = null)

/** Reconnect delays: exponential with jitter, so a server restart is not hit by every robot at once. */
data class Backoff(val firstMs: Long = 1_000, val maxMs: Long = 60_000, val unauthorizedMs: Long = 10 * 60_000) {
    fun delayMs(attempt: Int): Long {
        val base = (firstMs shl attempt.coerceAtMost(16)).coerceAtMost(maxMs)
        return base - Random.nextLong(base / 4 + 1)
    }
}

/**
 * The robot's signaling WebSocket `/ws/robot` (docs/08-backend.md), authenticated with the robot token.
 * It is the only thing running while the robot is idle (PWR-012): keepalive is a WebSocket ping every
 * 3 minutes, which the server counts as activity; OkHttp also drops the connection when a pong is missing.
 * Reconnects forever with [Backoff]; [reconnectNow] skips the wait (network came back).
 *
 * [onMessage] and [onLost] run in [scope]; [onLost] means a welcomed connection is gone. Each connection
 * attempt asks to resume the session [resume] returns (`?session=<id>`, SIG-007).
 */
class SignalingClient(
    private val url: String,
    private val token: String,
    private val scope: CoroutineScope,
    private val onMessage: (SignalIn) -> Unit,
    private val onLost: () -> Unit,
    private val resume: () -> Long? = { null },
    private val http: OkHttpClient = defaultHttp,
    private val backoff: Backoff = Backoff(),
) {
    private sealed class Event {
        data class Text(val text: String) : Event()
        data class Closed(val code: Int?, val reason: String?) : Event()
    }

    private val _state = MutableStateFlow(SignalState())
    val state: StateFlow<SignalState> = _state.asStateFlow()

    private val kick = Channel<Unit>(Channel.CONFLATED)
    private var socket: WebSocket? = null
    private var job: Job? = null

    fun start() {
        check(job == null) { "already started" }
        job = scope.launch { run() }
    }

    fun stop() {
        job?.cancel()
        socket?.cancel()
        socket = null
    }

    /** Queues a text message; false when not connected (the message is lost, callers resend state on welcome). */
    fun send(text: String): Boolean = socket?.send(text) ?: false

    fun reconnectNow() {
        kick.trySend(Unit)
    }

    private suspend fun run() {
        var attempt = 0
        while (true) {
            _state.value = SignalState(SignalPhase.CONNECTING)
            val events = Channel<Event>(Channel.UNLIMITED)
            val target = resume()?.let { "$url?session=$it" } ?: url
            val request = Request.Builder().url(target).header("Authorization", "Bearer $token").build()
            val ws = http.newWebSocket(request, Listener(events))
            socket = ws
            var welcomed = false
            val closed = try {
                receive(events) {
                    welcomed = true
                    attempt = 0
                }
            } finally {
                ws.cancel() // no-op when already closed; frees the socket when the job is cancelled
                socket = null
            }
            if (welcomed) onLost()
            val unauthorized = closed.code == HTTP_UNAUTHORIZED
            _state.value = SignalState(if (unauthorized) SignalPhase.UNAUTHORIZED else SignalPhase.OFFLINE, closed.reason)
            val wait = if (unauthorized) backoff.unauthorizedMs else backoff.delayMs(attempt++)
            withTimeoutOrNull(wait) { kick.receive() }
        }
    }

    /** Dispatches messages until the connection closes. */
    private suspend fun receive(events: Channel<Event>, onWelcome: () -> Unit): Event.Closed {
        while (true) {
            when (val e = events.receive()) {
                is Event.Text -> {
                    val msg = SignalIn.parse(e.text) ?: continue
                    if (msg is SignalIn.Welcome) {
                        onWelcome()
                        _state.value = SignalState(SignalPhase.ONLINE)
                    }
                    onMessage(msg)
                }
                is Event.Closed -> return e
            }
        }
    }

    /** OkHttp calls this on its own threads; everything is handed to [run] through the channel. */
    private class Listener(private val events: Channel<Event>) : WebSocketListener() {
        override fun onMessage(webSocket: WebSocket, text: String) {
            events.trySend(Event.Text(text))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, null)
            events.trySend(Event.Closed(code, reason.ifEmpty { null }))
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            events.trySend(Event.Closed(response?.code, response?.let { "HTTP ${it.code}" } ?: t.toString()))
        }
    }

    companion object {
        private const val HTTP_UNAUTHORIZED = 401

        val defaultHttp: OkHttpClient by lazy {
            OkHttpClient.Builder()
                .pingInterval(3, TimeUnit.MINUTES) // PWR-012: 2–4 min, server closes after 10 min of silence
                .connectTimeout(15, TimeUnit.SECONDS)
                .build()
        }

        /** `https://host[:port]` → `wss://host[:port]/ws/robot`; null if [server] is not an http(s) URL. */
        fun robotSocketUrl(server: String): String? {
            val s = server.trim().trimEnd('/')
            return when {
                s.startsWith("https://") -> "wss://" + s.removePrefix("https://") + "/ws/robot"
                s.startsWith("http://") -> "ws://" + s.removePrefix("http://") + "/ws/robot"
                else -> null
            }.takeIf { s.substringAfter("://").isNotEmpty() }
        }
    }
}
