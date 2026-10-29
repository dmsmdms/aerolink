package com.aerolink.robot.link

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean

enum class LinkPhase {
    /** HELLO sent, waiting for HELLO_ACK. */
    HANDSHAKE,
    READY,
    /** Firmware speaks another protocol version: never ARM (LINK-030). */
    INCOMPATIBLE,
    /** No HELLO_ACK after all attempts (old firmware, wrong device). */
    NO_RESPONSE,
    CLOSED,
}

data class LinkStatus(
    val phase: LinkPhase = LinkPhase.HANDSHAKE,
    val firmware: Report.HelloAck? = null,
    val robotState: RobotState = RobotState.DISARMED,
    val faults: Int = 0,
    val telemetry: Report.Telemetry? = null,
    val lastEvent: Report.Event? = null,
    val lastNack: Report.Nack? = null,
    val decodeErrors: Long = 0,
)

data class LinkTiming(
    val helloTimeoutMs: Long = 500,
    val helloAttempts: Int = 5,
    /** LINK-021: DRIVE at 20 Hz while ARMED. */
    val drivePeriodMs: Long = 50,
    /** A DRIVE survives ~4 lost frames, then the firmware stops the wheels (SAFE-002). */
    val driveTtlMs: Int = 200,
    /** FW_ACK after a sector (erase up to 400 ms) or FW_END (CRC over the image). */
    val firmwareAckTimeoutMs: Long = 3000,
)

/** A firmware transfer failed (LINK-040..042); the firmware keeps running the image it has. */
class FirmwareUpdateException(message: String) : IOException(message)

/**
 * Phone side of the USB link (docs/03-usb-link.md "Behaviour"): handshake, state tracking from
 * EVENT/TELEMETRY, commands, and the 20 Hz DRIVE stream that exists only while the robot is ARMED
 * (LINK-020: zero periodic traffic otherwise).
 *
 * One coroutine blocks on [LinkTransport.read] (LINK-022); writes are serialized by a mutex.
 */
class RobotLink(
    private val transport: LinkTransport,
    private val scope: CoroutineScope,
    private val timing: LinkTiming = LinkTiming(),
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    private val _status = MutableStateFlow(LinkStatus())
    val status: StateFlow<LinkStatus> = _status.asStateFlow()

    private val driveTarget = MutableStateFlow(zeroDrive(DriveSource.LOCAL))
    private val decoder = FrameDecoder()
    private val writeLock = Mutex()
    private var seq = 0
    private val closed = AtomicBoolean(false)
    private var mainJob: Job? = null
    private val fwAcks = Channel<Report.FwAck>(Channel.UNLIMITED)

    /** Between FW_BEGIN and FW_END the robot must not ARM (SAFE-007); the firmware refuses too. */
    @Volatile
    private var updating = false

    fun start() {
        check(mainJob == null) { "already started" }
        mainJob = scope.launch {
            if (handshake()) driveLoop()
        }
        scope.launch(io) {
            readLoop()
            shutdown()
        }
    }

    /** ARM; the drive target is reset first so a held joystick cannot jump the robot. */
    fun arm() {
        driveTarget.value = zeroDrive(driveTarget.value.source)
        scope.launch { if (status.value.phase == LinkPhase.READY && !updating) send(Command.Arm) }
    }

    fun disarm() {
        scope.launch { send(Command.Disarm) }
    }

    /** Latest wish of [source] in ‰; streamed while ARMED. */
    fun setDrive(speed: Int, steer: Int, source: DriveSource) {
        driveTarget.value = Command.Drive(speed, steer, source, timing.driveTtlMs)
    }

    /**
     * Sends [image] into the firmware's staging slot (LINK-040..042): FW_BEGIN, then one sector of FW_DATA at a
     * time, each acknowledged once it is in flash, then FW_END (the firmware checks the CRC and stages it).
     * Works in [LinkPhase.READY] and [LinkPhase.INCOMPATIBLE] (the update messages do not change between protocol
     * versions). The image is installed by the loader on the next [reboot]. Throws [FirmwareUpdateException].
     */
    suspend fun flashFirmware(image: ByteArray, onProgress: (percent: Int) -> Unit = {}) {
        if (image.isEmpty() || image.size > FirmwareFlash.SLOT_SIZE) throw FirmwareUpdateException("image of ${image.size} bytes")
        val crc = java.util.zip.CRC32().apply { update(image) }.value
        updating = true
        try {
            while (fwAcks.tryReceive().isSuccess) Unit
            send(Command.FwBegin(image.size, crc))
            expectAck("FW_BEGIN")
            var offset = 0
            var retries = 0
            while (offset < image.size) {
                val sectorEnd = minOf(image.size, (offset / FirmwareFlash.SECTOR + 1) * FirmwareFlash.SECTOR)
                var chunk = offset
                while (chunk < sectorEnd) {
                    val n = minOf(FirmwareFlash.CHUNK, sectorEnd - chunk)
                    send(Command.FwData(chunk, image.copyOfRange(chunk, chunk + n)))
                    chunk += n
                }
                if (sectorEnd % FirmwareFlash.SECTOR != 0) break // the last, partial sector is flushed by FW_END
                val ack = awaitAck("FW_DATA")
                when {
                    ack.status == FwStatus.OK && ack.offset == sectorEnd -> offset = sectorEnd
                    ack.status == FwStatus.BAD_OFFSET && retries++ < MAX_FIRMWARE_RETRIES -> {
                        // A frame got lost: resend from what is in flash, after the rest of the burst has drained.
                        delay(RESYNC_MS)
                        while (fwAcks.tryReceive().isSuccess) Unit
                        offset = ack.offset
                    }
                    else -> throw FirmwareUpdateException("FW_DATA at $offset: status ${ack.status}, offset ${ack.offset}")
                }
                onProgress(offset * 100 / image.size)
            }
            send(Command.FwEnd)
            expectAck("FW_END")
            onProgress(100)
        } finally {
            updating = false
        }
    }

    /** LINK-044: the running image works (it answered HELLO), so the loader keeps it. */
    suspend fun confirmFirmware() {
        while (fwAcks.tryReceive().isSuccess) Unit
        send(Command.FwConfirm)
        expectAck("FW_CONFIRM")
    }

    /** Restarts the robot base through the loader (installs a staged image); the link closes. DISARMED only. */
    suspend fun reboot() = send(Command.Reboot())

    private suspend fun awaitAck(what: String): Report.FwAck =
        withTimeoutOrNull(timing.firmwareAckTimeoutMs) { fwAcks.receive() }
            ?: throw FirmwareUpdateException("no answer to $what")

    private suspend fun expectAck(what: String) {
        val ack = awaitAck(what)
        if (ack.status != FwStatus.OK) throw FirmwareUpdateException("$what: status ${ack.status}")
    }

    /** Closes the link; a READY link is disarmed first (best effort). */
    suspend fun close() {
        if (closed.get()) return
        if (status.value.phase == LinkPhase.READY) {
            withTimeoutOrNull(DISARM_ON_CLOSE_TIMEOUT_MS) { send(Command.Disarm) }
        }
        shutdown()
    }

    private fun shutdown() {
        if (!closed.compareAndSet(false, true)) return
        mainJob?.cancel()
        runCatching { transport.close() }
        _status.update { it.copy(phase = LinkPhase.CLOSED) }
    }

    private fun readLoop() {
        val buffer = ByteArray(AccessoryTransport.READ_BUFFER_SIZE)
        while (!closed.get()) {
            val n = transport.read(buffer)
            if (n < 0) return
            decoder.push(buffer, 0, n, ::onFrame)
            if (decoder.errors != status.value.decodeErrors) {
                _status.update { it.copy(decodeErrors = decoder.errors) }
            }
        }
    }

    private fun onFrame(frame: Frame) {
        when (val r = Report.parse(frame)) {
            is Report.HelloAck -> _status.update {
                if (it.phase != LinkPhase.HANDSHAKE) return@update it
                val phase = if (r.protocolVersion == PROTOCOL_VERSION) LinkPhase.READY else LinkPhase.INCOMPATIBLE
                it.copy(phase = phase, firmware = r)
            }
            is Report.Telemetry -> _status.update {
                it.copy(telemetry = r, robotState = r.state ?: it.robotState, faults = r.faults)
            }
            is Report.Event -> _status.update {
                when (r.code) {
                    EventCode.STATE -> it.copy(lastEvent = r, robotState = RobotState.of(r.arg) ?: it.robotState)
                    EventCode.FAULTS -> it.copy(lastEvent = r, faults = r.arg)
                    else -> it.copy(lastEvent = r)
                }
            }
            is Report.Nack -> _status.update { it.copy(lastNack = r) }
            is Report.FwAck -> fwAcks.trySend(r)
            Report.Pong -> Unit
            null -> _status.update { it.copy(decodeErrors = it.decodeErrors + 1) }
        }
    }

    private suspend fun handshake(): Boolean {
        repeat(timing.helloAttempts) {
            send(Command.Hello())
            val reply = withTimeoutOrNull(timing.helloTimeoutMs) {
                status.first { it.phase != LinkPhase.HANDSHAKE }
            }
            if (reply != null) return reply.phase == LinkPhase.READY
        }
        _status.update { if (it.phase == LinkPhase.HANDSHAKE) it.copy(phase = LinkPhase.NO_RESPONSE) else it }
        return false
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private suspend fun driveLoop() {
        status.map { it.robotState == RobotState.ARMED }.distinctUntilChanged().collectLatest { armed ->
            while (armed) {
                send(driveTarget.value)
                delay(timing.drivePeriodMs)
            }
        }
    }

    private suspend fun send(command: Command) {
        if (closed.get()) return
        try {
            withContext(io) {
                writeLock.withLock {
                    transport.write(FrameCodec.encode(command.type, seq, command.payload()))
                    seq = (seq + 1) and 0xFF
                }
            }
        } catch (e: IOException) {
            shutdown()
        }
    }

    private fun zeroDrive(source: DriveSource) = Command.Drive(0, 0, source, timing.driveTtlMs)

    private companion object {
        const val DISARM_ON_CLOSE_TIMEOUT_MS = 200L
        const val MAX_FIRMWARE_RETRIES = 5
        const val RESYNC_MS = 200L
    }
}
