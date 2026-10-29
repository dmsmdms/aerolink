package com.aerolink.robot.teleop

import com.aerolink.robot.signal.Ice
import com.aerolink.robot.signal.IceServer
import com.aerolink.robot.signal.Sdp

enum class PeerState { NEW, CONNECTING, CONNECTED, DISCONNECTED, FAILED, CLOSED }

/** Callbacks from a [Peer]; implementations deliver them on the [Teleop] scope's thread. */
interface PeerEvents {
    fun onIce(ice: Ice)
    fun onState(state: PeerState)
    fun onCtl(text: String)
    fun onDrive(text: String)
}

/**
 * The robot's end of one WebRTC session (`rtc/WebRtcPeer` on the phone, a fake in tests). The console
 * offers; the peer answers with camera + microphone, plays the operator's voice and receives the `ctl` and
 * `drive` DataChannels.
 */
interface Peer {
    /**
     * Applies the offer and returns the answer. The first call starts capture; later ones are the console's
     * ICE restarts after a network change (SIG-007) and keep everything running.
     */
    suspend fun answer(offer: Sdp): Sdp

    /** Fresh TURN credentials for the next ICE restart. */
    fun setIceServers(servers: List<IceServer>)

    fun addIce(ice: Ice)

    /** False when the `ctl` channel is not open (the message is dropped). */
    fun sendCtl(text: String): Boolean

    /** Whether the camera in use has a flashlight; known once [answer] has started capture. */
    val hasLight: Boolean

    /** Flashlight on/off (CTL-004); false when it could not be switched. */
    suspend fun setLight(on: Boolean): Boolean

    /** Stops capture and frees everything; the camera is free when this returns. */
    suspend fun close()
}

fun interface PeerFactory {
    fun create(iceServers: List<IceServer>, events: PeerEvents): Peer
}
