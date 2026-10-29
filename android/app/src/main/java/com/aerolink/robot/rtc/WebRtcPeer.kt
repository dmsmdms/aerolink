package com.aerolink.robot.rtc

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.util.Log
import androidx.core.content.ContextCompat
import com.aerolink.robot.signal.Ice
import com.aerolink.robot.signal.IceServer
import com.aerolink.robot.signal.Sdp
import com.aerolink.robot.teleop.Peer
import com.aerolink.robot.teleop.PeerEvents
import com.aerolink.robot.teleop.PeerFactory
import com.aerolink.robot.teleop.PeerState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.Camera1Enumerator
import org.webrtc.Camera2Enumerator
import org.webrtc.CameraVideoCapturer
import org.webrtc.DataChannel
import org.webrtc.DefaultVideoDecoderFactory
import org.webrtc.DefaultVideoEncoderFactory
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.MediaStreamTrack
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RTCStatsReport
import org.webrtc.RtpParameters
import org.webrtc.RtpReceiver
import org.webrtc.RtpSender
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.SurfaceTextureHelper
import org.webrtc.VideoFrame
import org.webrtc.VideoProcessor
import org.webrtc.VideoSink
import org.webrtc.VideoSource
import org.webrtc.VideoTrack
import org.webrtc.audio.JavaAudioDeviceModule
import java.nio.ByteBuffer
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Capture and send settings. WebRTC scales the resolution down on its own when bandwidth or CPU run short
 * (PWR-013), keeping the frame rate: for driving, smooth motion matters more than detail. [maxBitrateBps]
 * caps cellular data (1.5 Mbit/s ≈ 0.7 GB per hour of driving); [startBitrateBps] skips most of the slow
 * ramp-up from 300 kbit/s. [rotation] (clockwise degrees): 0 = picture upright with the phone upright, whatever
 * the screen shows; otherwise as the phone is mounted on the robot.
 */
data class VideoSettings(
    val width: Int = 1280,
    val height: Int = 720,
    val fps: Int = 30,
    val rotation: Int = 0,
    val startBitrateBps: Int = 800_000,
    val maxBitrateBps: Int = 1_500_000,
)

/**
 * WebRTC peers for operator sessions. Everything (factory, EGL, encoder, camera, microphone) is created per
 * session and released when it ends, so an idle robot holds none of it (PWR-012). The native library is
 * loaded once per process.
 */
class WebRtcPeerFactory(
    private val context: Context,
    private val scope: CoroutineScope,
    private val video: () -> VideoSettings = { VideoSettings() },
) : PeerFactory {
    override fun create(iceServers: List<IceServer>, events: PeerEvents): Peer {
        initialize(context)
        return WebRtcPeer(context, scope, iceServers, events, video())
    }

    private companion object {
        @Volatile
        var initialized = false

        fun initialize(context: Context) {
            if (initialized) return
            synchronized(this) {
                if (initialized) return
                PeerConnectionFactory.initialize(
                    PeerConnectionFactory.InitializationOptions.builder(context.applicationContext)
                        .createInitializationOptions(),
                )
                initialized = true
            }
        }
    }
}

/**
 * One session (docs/08-backend.md "Control channel"): the console's offer has receive-only video, two-way
 * audio and the `ctl` and `drive` DataChannels; we answer with the back camera and the microphone and play
 * the operator's voice on the loudspeaker. Hardware H.264 is preferred when the phone has an encoder for it
 * (PWR-013), otherwise VP8. Later offers are ICE restarts (SIG-007).
 *
 * Nothing heavy happens before [answer]. WebRTC calls back on its own threads; events are handed to [scope].
 * [answer] and [close] are serialized, so closing during the answer waits for it and then tears everything down.
 */
private class WebRtcPeer(
    private val context: Context,
    private val scope: CoroutineScope,
    iceServers: List<IceServer>,
    private val events: PeerEvents,
    private val video: VideoSettings,
) : Peer {
    private val lock = Mutex()

    @Volatile
    private var iceServers = iceServers.map(::toNative)

    @Volatile
    private var closed = false

    // Created in answer(), on Dispatchers.IO: the constructor runs on the caller's (main) thread.
    private var egl: EglBase? = null
    private var audioModule: JavaAudioDeviceModule? = null
    private var factory: PeerConnectionFactory? = null
    private var pc: PeerConnection? = null
    private val channels = mutableListOf<DataChannel>()

    @Volatile
    private var ctl: DataChannel? = null

    private var capturer: CameraVideoCapturer? = null
    private var textures: SurfaceTextureHelper? = null
    private var videoSource: VideoSource? = null
    private var audioSource: AudioSource? = null
    private var speaker: Speaker? = null

    @Volatile
    override var hasLight = false
        private set

    // The lock is taken on Dispatchers.IO so a main thread blocked in close() can never hold it up.
    override suspend fun answer(offer: Sdp): Sdp = withContext(Dispatchers.IO) {
        lock.withLock {
            check(!closed) { "closed" }
            pc?.let { return@withLock restartIce(it, offer) }
            val egl = EglBase.create().also { this@WebRtcPeer.egl = it }
            val audio = JavaAudioDeviceModule.builder(context)
                .setUseHardwareAcousticEchoCanceler(true)
                .setUseHardwareNoiseSuppressor(true)
                .createAudioDeviceModule()
                .also { this@WebRtcPeer.audioModule = it }
            val factory = PeerConnectionFactory.builder()
                .setAudioDeviceModule(audio)
                .setVideoEncoderFactory(DefaultVideoEncoderFactory(egl.eglBaseContext, true, true))
                .setVideoDecoderFactory(DefaultVideoDecoderFactory(egl.eglBaseContext))
                .createPeerConnectionFactory()
                .also { this@WebRtcPeer.factory = it }
            val pc = checkNotNull(factory.createPeerConnection(config(), Observer())) { "createPeerConnection failed" }
            this@WebRtcPeer.pc = pc
            pc.setBitrate(null, video.startBitrateBps, null)
            speaker = Speaker(context)

            pc.awaitSet { pc.setRemoteDescription(it, SessionDescription(SessionDescription.Type.OFFER, offer.sdp)) }
            // addTrack reuses the transceivers of the offer: video becomes send-only, audio two-way.
            val videoSender = startCamera(factory, egl)?.let { pc.addTrack(it, listOf(STREAM_ID)) }
            startMicrophone(factory)?.let { pc.addTrack(it, listOf(STREAM_ID)) }
            preferH264(factory, pc)
            val answer = pc.awaitCreate { pc.createAnswer(it, MediaConstraints()) }
            pc.awaitSet { pc.setLocalDescription(it, answer) }
            videoSender?.let(::limitVideo)
            Sdp("answer", answer.description)
        }
    }

    /** New network on either end: same tracks and channels, new ICE credentials and candidates. */
    private suspend fun restartIce(pc: PeerConnection, offer: Sdp): Sdp {
        pc.setConfiguration(config())
        pc.awaitSet { pc.setRemoteDescription(it, SessionDescription(SessionDescription.Type.OFFER, offer.sdp)) }
        val answer = pc.awaitCreate { pc.createAnswer(it, MediaConstraints()) }
        pc.awaitSet { pc.setLocalDescription(it, answer) }
        Log.i(TAG, "ICE restart answered")
        return Sdp("answer", answer.description)
    }

    private fun config() = PeerConnection.RTCConfiguration(iceServers).apply {
        sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
        bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
        rtcpMuxPolicy = PeerConnection.RtcpMuxPolicy.REQUIRE
        // Watch new networks (Wi-Fi ↔ cellular) so an ICE restart finds candidates on them.
        continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
    }

    override fun setIceServers(servers: List<IceServer>) {
        iceServers = servers.map(::toNative)
    }

    override suspend fun setLight(on: Boolean): Boolean = withContext(Dispatchers.IO) {
        lock.withLock {
            val cam = capturer
            if (closed || cam == null || !hasLight) false else Torch.set(cam, on)
        }
    }

    override fun addIce(ice: Ice) {
        if (closed) return
        pc?.addIceCandidate(IceCandidate(ice.sdpMid ?: "", ice.sdpMLineIndex ?: 0, ice.candidate))
    }

    override fun sendCtl(text: String): Boolean {
        val channel = ctl ?: return false
        if (channel.state() != DataChannel.State.OPEN) return false
        return channel.send(DataChannel.Buffer(ByteBuffer.wrap(text.toByteArray()), false))
    }

    override suspend fun close() = withContext(Dispatchers.IO) {
        lock.withLock {
            if (closed) return@withLock
            closed = true
            pc?.let { logSummary(it) }
            // Order as in the WebRTC sample app: channels and connection, then sources and capture, factory last.
            ctl = null
            synchronized(channels) {
                channels.forEach {
                    it.unregisterObserver()
                    it.dispose()
                }
                channels.clear()
            }
            pc?.dispose()
            speaker?.release()
            audioSource?.dispose()
            try {
                capturer?.stopCapture()
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
            }
            capturer?.dispose()
            videoSource?.dispose()
            textures?.dispose()
            factory?.dispose()
            audioModule?.release()
            egl?.release()
        }
    }

    /** One line per session for field diagnostics: what went out, what came in, which path. */
    private suspend fun logSummary(pc: PeerConnection) {
        val report = withTimeoutOrNull(500) {
            suspendCancellableCoroutine<RTCStatsReport> { cont -> pc.getStats { cont.resume(it) } }
        } ?: return
        val stats = report.statsMap.values
        fun bytes(type: String, kind: String, field: String) = stats
            .filter { it.type == type && it.members["kind"] == kind }
            .sumOf { (it.members[field] as? Number)?.toLong() ?: 0L } / 1024
        val pair = stats.firstOrNull { it.type == "transport" }?.members?.get("selectedCandidatePairId")
            ?.let { report.statsMap[it] }
        val local = pair?.members?.get("localCandidateId")?.let { report.statsMap[it]?.members?.get("candidateType") }
        val remote = pair?.members?.get("remoteCandidateId")?.let { report.statsMap[it]?.members?.get("candidateType") }
        Log.i(
            TAG,
            "session summary: video sent ${bytes("outbound-rtp", "video", "bytesSent")} KiB, " +
                "audio sent ${bytes("outbound-rtp", "audio", "bytesSent")} KiB, " +
                "audio received ${bytes("inbound-rtp", "audio", "bytesReceived")} KiB, path $local ↔ $remote",
        )
    }

    private fun granted(permission: String) =
        ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

    /** Back camera (the robot looks forward); a session without camera permission has no video. */
    private fun startCamera(factory: PeerConnectionFactory, egl: EglBase): VideoTrack? {
        if (!granted(Manifest.permission.CAMERA)) {
            Log.w(TAG, "no camera permission: session without video")
            return null
        }
        // Camera2 only where no camera is LEGACY (the SM-J710F's are): Camera1 is the reliable path there.
        val enumerator = if (Camera2Enumerator.isSupported(context)) Camera2Enumerator(context) else Camera1Enumerator(true)
        val names = enumerator.deviceNames
        val name = names.firstOrNull(enumerator::isBackFacing) ?: names.firstOrNull() ?: run {
            Log.w(TAG, "no camera")
            return null
        }
        val cam = enumerator.createCapturer(name, null) ?: return null
        val helper = SurfaceTextureHelper.create("capture", egl.eglBaseContext)
        val source = factory.createVideoSource(false)
        // WebRTC derives the frame rotation from the display rotation, which on a robot depends on whatever
        // is on the screen. Pinned instead: the sensor orientation (phone upright) plus the mount rotation.
        sensorOrientation(enumerator is Camera2Enumerator, name)?.let { source.setVideoProcessor(Rotate((it + video.rotation) % 360)) }
        cam.initialize(helper, context, source.capturerObserver)
        cam.startCapture(video.width, video.height, video.fps)
        capturer = cam
        textures = helper
        videoSource = source
        hasLight = Torch.available(context, enumerator is Camera2Enumerator, name)
        Log.i(TAG, "camera $name, mount rotation ${video.rotation}°, light $hasLight")
        return factory.createVideoTrack("video", source)
    }

    private fun sensorOrientation(camera2: Boolean, name: String): Int? = try {
        if (camera2) {
            val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
            manager.getCameraCharacteristics(name).get(CameraCharacteristics.SENSOR_ORIENTATION)
        } else {
            CAMERA1_ORIENTATION.find(name)?.groupValues?.get(1)?.toInt() // "Camera 0, Facing back, Orientation 90"
        }
    } catch (e: Exception) {
        Log.w(TAG, "sensor orientation of camera $name", e)
        null
    }

    /** PWR-013: keep the frame rate, cap the bitrate (cellular data). */
    private fun limitVideo(sender: RtpSender) {
        val params = sender.parameters
        params.degradationPreference = RtpParameters.DegradationPreference.MAINTAIN_FRAMERATE
        params.encodings.forEach { it.maxBitrateBps = video.maxBitrateBps }
        if (!sender.setParameters(params)) Log.w(TAG, "video sender parameters rejected")
    }

    private fun startMicrophone(factory: PeerConnectionFactory): AudioTrack? {
        if (!granted(Manifest.permission.RECORD_AUDIO)) {
            Log.w(TAG, "no microphone permission: session without audio")
            return null
        }
        val source = factory.createAudioSource(MediaConstraints())
        audioSource = source
        return factory.createAudioTrack("audio", source)
    }

    /** Hardware H.264 first when the encoder factory offers it (it lists H.264 only with a hardware encoder). */
    private fun preferH264(factory: PeerConnectionFactory, pc: PeerConnection) {
        val transceiver = pc.transceivers.firstOrNull { it.mediaType == MediaStreamTrack.MediaType.MEDIA_TYPE_VIDEO } ?: return
        val codecs = factory.getRtpSenderCapabilities(MediaStreamTrack.MediaType.MEDIA_TYPE_VIDEO).codecs
        Log.i(TAG, "video codecs: ${codecs.map { it.name }.distinct()}")
        if (codecs.none { it.name.equals("H264", ignoreCase = true) }) return
        val error = transceiver.setCodecPreferences(codecs.sortedBy { if (it.name.equals("H264", ignoreCase = true)) 0 else 1 })
        if (error.isError) Log.w(TAG, "codec preferences: ${error.error()?.message}")
    }

    private fun deliver(block: () -> Unit) {
        scope.launch { if (!closed) block() }
    }

    private inner class Observer : PeerConnection.Observer {
        override fun onIceCandidate(c: IceCandidate) = deliver { events.onIce(Ice(c.sdp, c.sdpMid, c.sdpMLineIndex)) }

        override fun onConnectionChange(state: PeerConnection.PeerConnectionState) = deliver {
            events.onState(PeerState.valueOf(state.name))
        }

        override fun onDataChannel(channel: DataChannel) {
            val label = channel.label()
            synchronized(channels) {
                if (closed) {
                    channel.dispose()
                    return
                }
                channels += channel
            }
            if (label == CTL) ctl = channel
            channel.registerObserver(object : DataChannel.Observer {
                override fun onBufferedAmountChange(previous: Long) = Unit
                override fun onStateChange() = Unit
                override fun onMessage(buffer: DataChannel.Buffer) {
                    if (buffer.binary) return
                    val bytes = ByteArray(buffer.data.remaining()).also(buffer.data::get)
                    val text = String(bytes)
                    when (label) {
                        CTL -> deliver { events.onCtl(text) }
                        DRIVE -> deliver { events.onDrive(text) }
                    }
                }
            })
        }

        override fun onSignalingChange(state: PeerConnection.SignalingState) = Unit
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState) = Unit
        override fun onIceConnectionReceivingChange(receiving: Boolean) = Unit
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState) = Unit
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) = Unit
        override fun onAddStream(stream: MediaStream) = Unit
        override fun onRemoveStream(stream: MediaStream) = Unit
        override fun onRenegotiationNeeded() = Unit
        override fun onAddTrack(receiver: RtpReceiver, streams: Array<out MediaStream>) = Unit
    }

    private companion object {
        const val TAG = "AeroLinkRtc"
        const val CTL = "ctl"
        const val DRIVE = "drive"
        const val STREAM_ID = "robot"
        val CAMERA1_ORIENTATION = Regex("Orientation (\\d+)")

        fun toNative(s: IceServer): PeerConnection.IceServer = PeerConnection.IceServer.builder(s.urls)
            .setUsername(s.username ?: "")
            .setPassword(s.credential ?: "")
            .createIceServer()
    }
}

/**
 * Sets a fixed clockwise rotation on every frame. Only the rotation metadata changes (RTP video orientation):
 * the console's video element turns the picture, the encoder does no extra work.
 */
private class Rotate(private val degrees: Int) : VideoProcessor {
    @Volatile
    private var sink: VideoSink? = null

    override fun setSink(sink: VideoSink?) {
        this.sink = sink
    }

    override fun onCapturerStarted(success: Boolean) = Unit
    override fun onCapturerStopped() = Unit

    override fun onFrameCaptured(frame: VideoFrame) {
        sink?.onFrame(VideoFrame(frame.buffer, degrees, frame.timestampNs))
    }
}

private suspend fun PeerConnection.awaitSet(start: (SdpObserver) -> Unit) = suspendCancellableCoroutine { cont ->
    start(object : SdpObserver {
        override fun onSetSuccess() = cont.resume(Unit)
        override fun onSetFailure(error: String?) = cont.resumeWithException(IllegalStateException(error))
        override fun onCreateSuccess(sdp: SessionDescription?) = Unit
        override fun onCreateFailure(error: String?) = Unit
    })
}

private suspend fun PeerConnection.awaitCreate(start: (SdpObserver) -> Unit) = suspendCancellableCoroutine { cont ->
    start(object : SdpObserver {
        override fun onCreateSuccess(sdp: SessionDescription) = cont.resume(sdp)
        override fun onCreateFailure(error: String?) = cont.resumeWithException(IllegalStateException(error))
        override fun onSetSuccess() = Unit
        override fun onSetFailure(error: String?) = Unit
    })
}
