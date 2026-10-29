package com.aerolink.robot.teleop

import android.util.Log
import com.aerolink.robot.link.DriveSource
import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.signal.Ice
import com.aerolink.robot.signal.IceServer
import com.aerolink.robot.signal.SignalIn
import com.aerolink.robot.signal.SignalOut
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/** What the diagnostics screen and the service (wake lock, camera) need to know about the operator session. */
data class TeleopStatus(
    val session: Long? = null,
    val operator: String? = null,
    val peer: PeerState? = null,
) {
    val active get() = session != null
}

/**
 * The robot side of an operator session (docs/08-backend.md "Signaling" + "Control channel"):
 * `session` → wait for the offer → [Peer] answers with camera and microphone → `ctl`/`drive` commands go
 * through [DriveGate] to the current [RobotLink]; telemetry goes back at ~5 Hz and at once on every state
 * change. Camera, encoder and microphone exist only between `session` and `end` (PWR-012).
 *
 * One session at a time (SIG-001); a new `session` replaces an old one. The session survives a signaling
 * reconnect (SIG-007): the server re-announces it and the console restarts ICE with a new offer.
 * All calls on [scope]'s thread.
 */
class Teleop(
    private val scope: CoroutineScope,
    private val links: StateFlow<RobotLink?>,
    private val peers: PeerFactory,
    private val signal: (String) -> Boolean,
    private val phoneBattery: () -> Int? = { null },
    private val telemetryPeriodMs: Long = 200,
    private val driveTimeoutMs: Long = 500,
    private val armGraceMs: Long = 1500,
) {
    private val _status = MutableStateFlow(TeleopStatus())
    val status: StateFlow<TeleopStatus> = _status.asStateFlow()

    private val sink = object : DriveSink {
        override fun arm() {
            links.value?.arm()
        }

        override fun drive(speed: Int, steer: Int) {
            links.value?.setDrive(speed, steer, DriveSource.OPERATOR)
        }

        override fun stop() {
            links.value?.run {
                setDrive(0, 0, DriveSource.OPERATOR)
                disarm()
            }
        }
    }
    /** New per session: DRIVE `seq` restarts with every console session. */
    private var gate = DriveGate(scope, sink, driveTimeoutMs, armGraceMs)

    private var session: Long? = null
    private var iceServers: List<IceServer> = emptyList()
    private var peer: Peer? = null
    private var answered = false
    private val earlyIce = mutableListOf<Ice>()
    private var telemetry: Job? = null
    /** The previous peer still releasing the camera. */
    private var closing: Job? = null
    /** CTL-004, per session (the camera and its flashlight close with it). */
    private val light = MutableStateFlow(false)

    /** The session to ask the server to resume when signaling reconnects (SIG-007). */
    val currentSession: Long? get() = session

    fun onSignal(msg: SignalIn) {
        when (msg) {
            is SignalIn.Session -> {
                if (msg.session == session) {
                    // Resumed after a reconnect: the console follows with an ICE restart offer.
                    Log.i(TAG, "session ${msg.session} resumed")
                    iceServers = msg.iceServers
                    peer?.setIceServers(msg.iceServers)
                    return
                }
                if (session != null) endSession()
                session = msg.session
                gate = DriveGate(scope, sink, driveTimeoutMs, armGraceMs)
                iceServers = msg.iceServers
                _status.value = TeleopStatus(msg.session, msg.operatorName)
                Log.i(TAG, "session ${msg.session} with ${msg.operatorName}")
            }
            is SignalIn.Offer -> if (msg.session == session) {
                val p = peer
                if (p == null) startPeer(msg) else negotiate(p, msg) // ICE restart
            }
            is SignalIn.Candidate -> if (msg.session == session) {
                if (answered) peer?.addIce(msg.ice) else earlyIce += msg.ice
            }
            is SignalIn.End -> if (msg.session == session) {
                Log.i(TAG, "session ${msg.session} ended: ${msg.reason}")
                endSession()
            }
            is SignalIn.Welcome, is SignalIn.Update -> Unit
        }
    }

    /**
     * The signaling connection is gone. A running peer stays: media and control are peer-to-peer and may
     * still work, and the reconnect asks the server to resume the session (SIG-007), which answers `end` if
     * it is over. Commands that stop meanwhile stop the robot (SAFE-010). Without a peer there is nothing
     * to keep.
     */
    fun onSignalLost() {
        if (session != null && peer == null) endSession()
    }

    /** Ends the session from the robot side (e.g. the service stops). */
    fun close() {
        session?.let { signal(SignalOut.end(it)) }
        endSession()
    }

    private fun startPeer(offer: SignalIn.Offer) {
        val id = offer.session
        val p = peers.create(iceServers, Events(id))
        peer = p
        _status.value = _status.value.copy(peer = PeerState.NEW)
        negotiate(p, offer, after = closing) // the camera of the previous session must be free
        telemetry = scope.launch { sendTelemetry(p) }
    }

    /** Answers [offer]; ICE that arrives meanwhile belongs to it and waits for the answer. */
    private fun negotiate(p: Peer, offer: SignalIn.Offer, after: Job? = null) {
        val id = offer.session
        answered = false
        scope.launch {
            after?.join()
            val answer = try {
                p.answer(offer.sdp)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "answer failed", e)
                if (session == id && peer === p) close()
                return@launch
            }
            if (session != id || peer !== p) return@launch // ended meanwhile; endSession closed the peer
            signal(SignalOut.answer(id, answer))
            answered = true
            earlyIce.forEach(p::addIce)
            earlyIce.clear()
        }
    }

    private fun endSession() {
        gate.stop()
        telemetry?.cancel()
        telemetry = null
        peer?.let { p ->
            val previous = closing
            // Not a child of the scope: the camera must be released even when the service is going away.
            closing = scope.launch(NonCancellable) {
                previous?.join()
                p.close()
            }
        }
        peer = null
        session = null
        answered = false
        earlyIce.clear()
        light.value = false
        _status.value = TeleopStatus()
    }

    /**
     * ~5 Hz, and immediately when state, faults, base availability or the light change (the console waits
     * for ARMED).
     */
    @OptIn(ExperimentalCoroutinesApi::class)
    private suspend fun sendTelemetry(p: Peer) {
        links.flatMapLatest { link -> link?.status ?: flowOf(null) }
            .map { Triple(it?.phase == LinkPhase.READY, it?.robotState, it?.faults) }
            .distinctUntilChanged()
            .combine(light) { _, on -> on }
            .collectLatest { on ->
                while (true) {
                    p.sendCtl(CtlOut.telemetry(links.value?.status?.value, phoneBattery(), on.takeIf { p.hasLight }))
                    delay(telemetryPeriodMs)
                }
            }
    }

    private inner class Events(private val id: Long) : PeerEvents {
        private val current get() = session == id

        override fun onIce(ice: Ice) {
            if (current) signal(SignalOut.ice(id, ice))
        }

        override fun onState(state: PeerState) {
            if (!current) return
            _status.value = _status.value.copy(peer = state)
            if (state == PeerState.FAILED) {
                Log.w(TAG, "session $id: peer connection failed")
                close()
            }
        }

        override fun onCtl(text: String) {
            if (!current) return
            when (val m = CtlIn.parse(text)) {
                CtlIn.Arm -> gate.arm()
                CtlIn.Disarm -> gate.disarm()
                is CtlIn.Ping -> peer?.sendCtl(CtlOut.pong(m.ts))
                is CtlIn.Light -> peer?.let { p ->
                    scope.launch { if (p.setLight(m.on) && peer === p) light.value = m.on }
                }
                null -> Unit
            }
        }

        override fun onDrive(text: String) {
            if (!current) return
            DriveMsg.parse(text)?.let(gate::drive)
        }
    }

    private companion object {
        const val TAG = "AeroLinkTeleop"
    }
}
