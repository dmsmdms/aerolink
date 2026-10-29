package com.aerolink.robot.teleop

import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.link.RobotState
import com.aerolink.robot.link.SimulatedBase
import com.aerolink.robot.signal.Ice
import com.aerolink.robot.signal.IceServer
import com.aerolink.robot.signal.Sdp
import com.aerolink.robot.signal.SignalIn
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

private class FakePeer(val initialIceServers: List<IceServer>, val events: PeerEvents) : Peer {
    var servers = initialIceServers
    val added = mutableListOf<Ice>()
    val ctl = LinkedBlockingQueue<JSONObject>()
    val offers = mutableListOf<String>()
    var lightOn = false

    @Volatile
    var closed = false

    override suspend fun answer(offer: Sdp): Sdp {
        offers += offer.sdp
        return Sdp("answer", "answer to ${offer.sdp}")
    }
    override fun setIceServers(servers: List<IceServer>) {
        this.servers = servers
    }
    override fun addIce(ice: Ice) {
        added += ice
    }
    override fun sendCtl(text: String) = ctl.offer(JSONObject(text))
    override val hasLight = true
    override suspend fun setLight(on: Boolean): Boolean {
        lightOn = on
        return true
    }
    override suspend fun close() {
        closed = true
    }

    /** Next ctl message of type [t], skipping others. */
    fun next(t: String, timeoutMs: Long = 1000): JSONObject {
        val end = System.nanoTime() + timeoutMs * 1_000_000
        while (true) {
            val left = (end - System.nanoTime()) / 1_000_000
            val m = ctl.poll(left.coerceAtLeast(0), TimeUnit.MILLISECONDS) ?: error("no $t within $timeoutMs ms")
            if (m.getString("t") == t) return m
        }
    }
}

class TeleopTest {
    private val executor = Executors.newSingleThreadExecutor()
    private val dispatcher = executor.asCoroutineDispatcher()
    private val scope = CoroutineScope(SupervisorJob() + dispatcher)
    private val links = MutableStateFlow<RobotLink?>(null)
    private val peers = LinkedBlockingQueue<FakePeer>()
    private val signaled = LinkedBlockingQueue<JSONObject>()
    private val teleop = Teleop(
        scope = scope,
        links = links,
        peers = { servers, events -> FakePeer(servers, events).also(peers::put) },
        signal = { signaled.offer(JSONObject(it)) },
        phoneBattery = { 55 },
        telemetryPeriodMs = 50,
        driveTimeoutMs = 200,
        armGraceMs = 500,
    )

    @After
    fun tearDown() {
        scope.cancel()
        executor.shutdown()
    }

    private fun on(block: () -> Unit) = runBlocking(dispatcher) { block() }
    private fun signal(timeoutMs: Long = 1000) = signaled.poll(timeoutMs, TimeUnit.MILLISECONDS)
    private fun peer() = peers.poll(1, TimeUnit.SECONDS) ?: error("no peer created")

    private fun connectBase(): RobotLink {
        val link = RobotLink(SimulatedBase(), scope)
        link.start()
        runBlocking { withTimeout(2000) { link.status.first { it.phase == LinkPhase.READY } } }
        links.value = link
        return link
    }

    private fun awaitState(link: RobotLink, state: RobotState) =
        runBlocking { withTimeout(2000) { link.status.first { it.robotState == state } } }

    private fun startSession(id: Long = 1): FakePeer {
        val turn = IceServer(listOf("turn:x"), "u", "p")
        on {
            teleop.onSignal(SignalIn.Session(id, 42, "Alice", listOf(turn)))
            teleop.onSignal(SignalIn.Candidate(id, Ice("early", "0", 0))) // before the offer
            teleop.onSignal(SignalIn.Offer(id, Sdp("offer", "o$id")))
        }
        val answer = signal()!!
        assertEquals("answer", answer.getString("type"))
        assertEquals(id, answer.getLong("session"))
        assertEquals("answer to o$id", answer.getJSONObject("sdp").getString("sdp"))
        val peer = peer()
        assertEquals(listOf(turn), peer.servers)
        on { assertEquals(listOf(Ice("early", "0", 0)), peer.added) } // flushed after the answer
        return peer
    }

    @Test
    fun sessionDrivesTheBase() {
        val link = connectBase()
        val peer = startSession()
        assertTrue(teleop.status.value.active)
        assertEquals("Alice", teleop.status.value.operator)

        val idle = peer.next("telemetry")
        assertTrue(idle.getBoolean("base"))
        assertEquals("DISARMED", idle.getString("state"))
        assertEquals(55, idle.getInt("phone_battery"))

        on { peer.events.onCtl("""{"t":"arm"}""") }
        awaitState(link, RobotState.ARMED)
        // The console waits for ARMED: it goes out at once, not at the next 5 Hz tick.
        assertEquals("ARMED", peer.next("telemetry", timeoutMs = 40).getString("state"))

        on { peer.events.onDrive("""{"seq":1,"ts":0,"speed":400,"steer":0}""") }
        runBlocking { withTimeout(2000) { link.status.first { it.telemetry?.speedLeft == 400 } } }
        while (peer.next("telemetry").optInt("speed_l") != 400) Unit

        on { peer.events.onCtl("""{"t":"ping","ts":123.5}""") }
        assertEquals(123.5, peer.next("pong").getDouble("ts"), 0.0)

        on { peer.events.onCtl("""{"t":"disarm"}""") }
        awaitState(link, RobotState.DISARMED)
        // No stale wheel speeds once the base stops reporting.
        while (peer.next("telemetry").getString("state") != "DISARMED") Unit
        assertEquals(0, peer.next("telemetry").getInt("speed_l"))
    }

    @Test
    fun operatorSilenceDisarms() { // SAFE-010
        val link = connectBase()
        val peer = startSession()
        on {
            peer.events.onCtl("""{"t":"arm"}""")
            peer.events.onDrive("""{"seq":1,"ts":0,"speed":300,"steer":0}""")
        }
        awaitState(link, RobotState.ARMED)
        val start = System.nanoTime()
        awaitState(link, RobotState.DISARMED)
        val ms = (System.nanoTime() - start) / 1_000_000
        assertTrue("disarmed after $ms ms", ms < 600)
    }

    @Test
    fun endStopsAndReleases() {
        val link = connectBase()
        val peer = startSession()
        on {
            peer.events.onCtl("""{"t":"arm"}""")
            peer.events.onDrive("""{"seq":1,"ts":0,"speed":300,"steer":0}""")
        }
        awaitState(link, RobotState.ARMED)
        on { teleop.onSignal(SignalIn.End(1, "released")) }
        awaitState(link, RobotState.DISARMED)
        runBlocking { withTimeout(1000) { while (!peer.closed) kotlinx.coroutines.delay(10) } }
        assertFalse(teleop.status.value.active)

        // Late messages of the old session are ignored.
        on { peer.events.onCtl("""{"t":"arm"}""") }
        Thread.sleep(100)
        assertEquals(RobotState.DISARMED, link.status.value.robotState)
    }

    @Test
    fun newSessionRestartsSequence() { // SAFE-012 is per console session
        val link = connectBase()
        startSession(1)
        on { teleop.onSignal(SignalIn.End(1, "released")) }
        val peer = startSession(2)
        on {
            peer.events.onCtl("""{"t":"arm"}""")
            peer.events.onDrive("""{"seq":1,"ts":0,"speed":250,"steer":0}""")
        }
        runBlocking { withTimeout(2000) { link.status.first { it.telemetry?.speedLeft == 250 } } }
    }

    @Test
    fun withoutBaseTelemetrySaysSo() {
        val peer = startSession()
        val t = peer.next("telemetry")
        assertFalse(t.getBoolean("base"))
        assertEquals("DISARMED", t.getString("state"))
        on { peer.events.onCtl("""{"t":"arm"}""") } // nothing to arm, nothing breaks
        assertFalse(peer.next("telemetry").getBoolean("base"))
    }

    @Test
    fun failedPeerEndsTheSession() {
        val peer = startSession()
        on { peer.events.onState(PeerState.FAILED) }
        val end = signal()!!
        assertEquals("end", end.getString("type"))
        assertEquals(1, end.getLong("session"))
        assertFalse(teleop.status.value.active)
    }

    @Test
    fun foreignSessionsAreIgnored() {
        val peer = startSession(5)
        on {
            teleop.onSignal(SignalIn.Candidate(6, Ice("other", "0", 0)))
            teleop.onSignal(SignalIn.End(6, "released"))
        }
        assertTrue(teleop.status.value.active)
        assertNull(peers.poll(100, TimeUnit.MILLISECONDS))
        on { assertEquals(1, peer.added.size) }
    }

    @Test
    fun sessionSurvivesSignalingReconnect() { // SIG-007
        val link = connectBase()
        val peer = startSession(3)
        on {
            peer.events.onCtl("""{"t":"arm"}""")
            peer.events.onDrive("""{"seq":1,"ts":0,"speed":200,"steer":0}""")
        }
        awaitState(link, RobotState.ARMED)
        on { teleop.onSignalLost() }
        assertEquals(3L, teleop.currentSession)
        assertFalse(peer.closed)

        // The server resumes it with fresh TURN credentials, the console restarts ICE.
        val fresh = IceServer(listOf("turn:y"), "u2", "p2")
        on {
            teleop.onSignal(SignalIn.Session(3, 42, "Alice", listOf(fresh)))
            teleop.onSignal(SignalIn.Offer(3, Sdp("offer", "restart")))
            teleop.onSignal(SignalIn.Candidate(3, Ice("new", "0", 0))) // may overtake the answer
        }
        val answer = signal()!!
        assertEquals("answer to restart", answer.getJSONObject("sdp").getString("sdp"))
        assertNull(peers.poll(100, TimeUnit.MILLISECONDS)) // same peer, same camera
        on {
            assertEquals(listOf(fresh), peer.servers)
            assertEquals(listOf("o3", "restart"), peer.offers)
            assertEquals("new", peer.added.last().candidate)
        }
        // Same DriveGate: the sequence continues, the robot keeps driving.
        on { peer.events.onDrive("""{"seq":2,"ts":0,"speed":350,"steer":0}""") }
        runBlocking { withTimeout(2000) { link.status.first { it.telemetry?.speedLeft == 350 } } }
        assertTrue(teleop.status.value.active)
    }

    @Test
    fun lostSignalingBeforeTheOfferEndsTheSession() {
        on {
            teleop.onSignal(SignalIn.Session(1, 42, "Alice", emptyList()))
            teleop.onSignalLost() // nothing to keep yet; the server answers `end` to a resume anyway
        }
        assertFalse(teleop.status.value.active)
        assertNull(teleop.currentSession)
        assertNull(signal(100))
    }

    @Test
    fun lightFollowsTheConsole() { // CTL-004
        val peer = startSession()
        assertFalse(peer.next("telemetry").getBoolean("light"))
        on { peer.events.onCtl("""{"t":"light","on":true}""") }
        while (!peer.next("telemetry").getBoolean("light")) Unit
        on { assertTrue(peer.lightOn) }
        on { peer.events.onCtl("""{"t":"light","on":false}""") }
        while (peer.next("telemetry").getBoolean("light")) Unit
    }
}
