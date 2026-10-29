package com.aerolink.robot.signal

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** Server side of one mocked WebSocket. */
private class ServerSocket : WebSocketListener() {
    val received = LinkedBlockingQueue<String>()
    val opened = LinkedBlockingQueue<WebSocket>()

    override fun onOpen(webSocket: WebSocket, response: Response) {
        opened.put(webSocket)
    }

    override fun onMessage(webSocket: WebSocket, text: String) {
        received.put(text)
    }

    fun socket(): WebSocket = opened.poll(2, TimeUnit.SECONDS) ?: error("client did not connect")
}

class SignalingClientTest {
    private val server = MockWebServer()
    private val executor = Executors.newSingleThreadExecutor()
    private val scope = CoroutineScope(SupervisorJob() + executor.asCoroutineDispatcher())
    private val messages = LinkedBlockingQueue<SignalIn>()
    private val lost = LinkedBlockingQueue<Unit>()
    @Volatile
    private var session: Long? = null

    @After
    fun tearDown() {
        scope.cancel()
        executor.shutdown()
        server.shutdown()
    }

    private fun client() = SignalingClient(
        url = server.url("/ws/robot").toString().replaceFirst("http", "ws"),
        token = "7.secret",
        scope = scope,
        onMessage = messages::put,
        onLost = { lost.put(Unit) },
        resume = { session },
        backoff = Backoff(firstMs = 50, maxMs = 100, unauthorizedMs = 60_000),
    )

    private fun SignalingClient.await(phase: SignalPhase) =
        runBlocking { withTimeout(3000) { state.first { it.phase == phase } } }

    @Test
    fun connectsAuthenticatesAndReconnects() {
        val first = ServerSocket()
        val second = ServerSocket()
        server.enqueue(MockResponse().withWebSocketUpgrade(first))
        server.enqueue(MockResponse().withWebSocketUpgrade(second))
        val client = client()
        client.start()

        val ws = first.socket()
        assertEquals("Bearer 7.secret", server.takeRequest().getHeader("Authorization"))
        ws.send("""{"type":"welcome","you":{"id":7,"name":"r1"}}""")
        client.await(SignalPhase.ONLINE)
        assertEquals(SignalIn.Welcome(7, "r1"), messages.poll(1, TimeUnit.SECONDS))

        assertTrue(client.send(SignalOut.end(1)))
        assertEquals(SignalOut.end(1), first.received.poll(1, TimeUnit.SECONDS))

        ws.send("""{"type":"something_new"}""") // ignored
        ws.send("""{"type":"end","session":1,"reason":"released"}""")
        assertEquals(SignalIn.End(1, "released"), messages.poll(1, TimeUnit.SECONDS))

        session = 4
        ws.close(1008, "rate")
        assertEquals(Unit, lost.poll(2, TimeUnit.SECONDS))
        second.socket() // backoff, then a new connection that asks to resume the session (SIG-007)
        assertEquals("/ws/robot?session=4", server.takeRequest().path)
        client.stop()
    }

    @Test
    fun rejectedTokenWaitsLong() {
        server.enqueue(MockResponse().setResponseCode(401))
        val client = client()
        client.start()
        client.await(SignalPhase.UNAUTHORIZED)
        Thread.sleep(300)
        assertEquals(1, server.requestCount) // not hammering the server with a bad token
        client.stop()
    }

    @Test
    fun unreachableServerRetries() {
        server.enqueue(MockResponse().setResponseCode(502))
        server.enqueue(MockResponse().setResponseCode(502))
        val ok = ServerSocket()
        server.enqueue(MockResponse().withWebSocketUpgrade(ok))
        val client = client()
        client.start()
        ok.socket().send("""{"type":"welcome","you":{"id":7,"name":"r1"}}""")
        client.await(SignalPhase.ONLINE)
        assertEquals(3, server.requestCount)
        assertTrue(lost.isEmpty()) // never welcomed before, nothing was lost
        client.stop()
    }
}
