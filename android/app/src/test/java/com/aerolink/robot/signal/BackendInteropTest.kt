package com.aerolink.robot.signal

import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.link.RobotState
import com.aerolink.robot.link.SimulatedBase
import com.aerolink.robot.teleop.Peer
import com.aerolink.robot.teleop.PeerEvents
import com.aerolink.robot.teleop.Teleop
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The robot side against the real backend (run by `make e2e`, skipped otherwise): SignalingClient + Teleop +
 * RobotLink on a SimulatedBase, with a scripted operator speaking HTTP/WebSocket like the console. Only the
 * WebRTC peer is fake (SDP and ICE are opaque to the server), so this pins the JSON contract of
 * docs/08-backend.md between the phone and `backend/src/hub.rs`.
 */
class BackendInteropTest {
    private val backend = System.getProperty("aerolink.backend").orEmpty()
    private val token = System.getProperty("aerolink.robotToken").orEmpty()
    private val origin = System.getProperty("aerolink.origin").orEmpty()

    private val executor = Executors.newSingleThreadExecutor()
    private val dispatcher = executor.asCoroutineDispatcher()
    private val scope = CoroutineScope(SupervisorJob() + dispatcher)
    private val http = OkHttpClient()

    @After
    fun tearDown() {
        scope.cancel()
        executor.shutdown()
    }

    private class Operator : WebSocketListener() {
        val inbox = LinkedBlockingQueue<JSONObject>()
        override fun onMessage(webSocket: WebSocket, text: String) {
            inbox.put(JSONObject(text))
        }
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            inbox.put(JSONObject().put("type", "failure").put("error", t.toString()))
        }

        fun next(match: (JSONObject) -> Boolean): JSONObject {
            val end = System.nanoTime() + 5_000_000_000
            while (true) {
                val m = inbox.poll((end - System.nanoTime()) / 1_000_000, TimeUnit.MILLISECONDS) ?: error("operator: timeout")
                check(m.getString("type") != "failure") { m.toString() }
                if (match(m)) return m
            }
        }
    }

    private class RecordingPeer(val events: PeerEvents) : Peer {
        val ice = LinkedBlockingQueue<Ice>()
        @Volatile var closed = false
        override suspend fun answer(offer: Sdp) = Sdp("answer", "answer to ${offer.sdp}")
        override fun addIce(ice: Ice) {
            this.ice.put(ice)
        }
        override fun setIceServers(servers: List<IceServer>) = Unit
        override fun sendCtl(text: String) = true
        override val hasLight = false
        override suspend fun setLight(on: Boolean) = false
        override suspend fun close() {
            closed = true
        }
    }

    @Test
    fun robotAndOperatorThroughTheRealBackend() {
        assumeTrue("set by tools/e2e_console.sh", backend.isNotEmpty())

        // Robot: a base, the signaling client, the session controller.
        val link = RobotLink(SimulatedBase(), scope).also { it.start() }
        runBlocking { withTimeout(2000) { link.status.first { it.phase == LinkPhase.READY } } }
        val peers = LinkedBlockingQueue<RecordingPeer>()
        lateinit var client: SignalingClient
        val teleop = Teleop(
            scope, MutableStateFlow(link), { _, events -> RecordingPeer(events).also(peers::put) },
            signal = { client.send(it) }, phoneBattery = { 55 },
        )
        client = SignalingClient(
            SignalingClient.robotSocketUrl(backend)!!, token, scope, teleop::onSignal, teleop::onSignalLost,
            { teleop.currentSession },
        )
        client.start()
        runBlocking { withTimeout(5000) { client.state.first { it.phase == SignalPhase.ONLINE } } }
        client.send(SignalOut.status(RobotStatus(RobotMode.IDLE, phoneBattery = 55, app = "test")))

        // Operator: login, operator WebSocket, robot list with the status the robot sent.
        val login = http.newCall(
            Request.Builder().url("$backend/api/v1/login")
                .post("""{"email":"op@x.org","password":"password1234"}""".toRequestBody("application/json".toMediaType()))
                .build(),
        ).execute()
        assertEquals(200, login.code)
        val cookie = login.headers("Set-Cookie").first().substringBefore(';')
        login.close()
        val op = Operator()
        val ws = http.newWebSocket(
            Request.Builder().url(backend.replaceFirst("http", "ws") + "/ws/operator")
                .header("Cookie", cookie).header("Origin", origin).build(),
            op,
        )
        val robots = op.next { it.getString("type") == "robots" }.getJSONArray("robots")
        val phone = List(robots.length()) { robots.getJSONObject(it) }.first { it.getString("name") == "phone" }
        val robotId = phone.getLong("id")
        if (phone.getJSONObject("status").optInt("phone_battery") != 55) { // the status may still be on its way
            op.next { m -> m.getString("type") == "robot" && m.getJSONObject("robot").getJSONObject("status").optInt("phone_battery") == 55 }
        }

        // Take → session on both ends → offer/answer and ICE both ways → arm through the session.
        ws.send(JSONObject().put("type", "take").put("robot", robotId).toString())
        val session = op.next { it.getString("type") == "session" }.getLong("session")
        ws.send(JSONObject().put("type", "offer").put("session", session).put("sdp", Sdp("offer", "v=0 op").toJson()).toString())
        val answer = op.next { it.getString("type") == "answer" }
        assertEquals("answer to v=0 op", answer.getJSONObject("sdp").getString("sdp"))

        val peer = peers.poll(2, TimeUnit.SECONDS)!!
        val opIce = Ice("candidate:1 1 udp 2122260223 192.0.2.1 50000 typ host", "0", 0)
        ws.send(JSONObject().put("type", "ice").put("session", session).put("candidate", opIce.toJson()).toString())
        assertEquals(opIce, peer.ice.poll(2, TimeUnit.SECONDS))
        val robotIce = Ice("candidate:2 1 udp 2122260223 192.0.2.2 50001 typ host", "0", 0)
        runBlocking(dispatcher) { peer.events.onIce(robotIce) }
        assertEquals(robotIce, Ice.parse(op.next { it.getString("type") == "ice" }.getJSONObject("candidate")))

        runBlocking(dispatcher) { peer.events.onCtl("""{"t":"arm"}""") }
        runBlocking { withTimeout(2000) { link.status.first { it.robotState == RobotState.ARMED } } }

        // Release → the robot hears `end`, stops and frees the peer.
        ws.send(JSONObject().put("type", "release").put("session", session).toString())
        runBlocking { withTimeout(2000) { link.status.first { it.robotState == RobotState.DISARMED } } }
        runBlocking { withTimeout(2000) { teleop.status.first { !it.active } } }
        runBlocking { withTimeout(2000) { while (!peer.closed) delay(10) } } // released after the session ends

        ws.close(1000, null)
        client.stop()
    }
}
