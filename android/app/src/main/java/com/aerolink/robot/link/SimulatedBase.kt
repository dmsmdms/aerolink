package com.aerolink.robot.link

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * A robot base in software, for bench tests without the RP2040 (diagnostics screen switch, debug builds):
 * answers HELLO, follows ARM/DISARM/DRIVE like the firmware's control state machine including the command
 * timeout (`SAFE-002`), sends EVENTs on change and TELEMETRY while ARMED; wheel speeds follow the last DRIVE
 * (differential mix). Announces `MOTOR_SIMULATED`. Takes firmware updates like the real firmware (ROB-009): the
 * received image is "installed" on REBOOT, which looks like a detach; the next instance reports it (in trial until
 * FW_CONFIRM). The simulated flash lives as long as the process.
 *
 * Time runs in [read]: the caller's blocking read waits for the next frame or the next deadline, so there is
 * no thread of its own, and nothing runs while DISARMED (`LINK-020`).
 */
class SimulatedBase(
    private val cmdTimeoutMs: Long = 250,
    private val telemetryPeriodMs: Long = 200,
    private val batteryMv: Int = 36500,
) : LinkTransport {
    private val toPhone = LinkedBlockingQueue<ByteArray>()
    private val decoder = FrameDecoder()
    private val lock = Any()

    @Volatile
    private var closed = false

    // Guarded by lock.
    private var state = RobotState.DISARMED
    private var speedLeft = 0
    private var speedRight = 0
    private var driveDeadline = 0L
    private var nextTelemetry = 0L
    private var seq = 0

    // Firmware update in progress (guarded by lock).
    private var updating = false
    private var updateSize = 0
    private var updateCrc = 0L
    private var image = ByteArrayOutputStream()
    private var written = 0

    override fun read(buffer: ByteArray): Int {
        while (true) {
            if (closed) return -1
            val wait = synchronized(lock) { tick(now()) }
            val data = if (wait == null) toPhone.take() else toPhone.poll(wait, TimeUnit.MILLISECONDS)
            if (data == null) continue
            if (data.isEmpty()) return -1 // close() wakes us up
            data.copyInto(buffer)
            return data.size
        }
    }

    override fun write(data: ByteArray) {
        if (closed) throw IOException("closed")
        synchronized(lock) { decoder.push(data) { onCommand(it, now()) } }
    }

    override fun close() {
        closed = true
        toPhone.put(ByteArray(0))
    }

    /** Applies deadlines; returns ms until the next one, or null when idle. */
    private fun tick(now: Long): Long? {
        if (state != RobotState.ARMED) return null
        if (now >= driveDeadline) {
            event(EventCode.CMD_TIMEOUT, 0)
            setState(RobotState.DISARMED)
            return null
        }
        if (now >= nextTelemetry) {
            telemetry()
            nextTelemetry = now + telemetryPeriodMs
        }
        return minOf(driveDeadline, nextTelemetry) - now
    }

    private fun onCommand(frame: Frame, now: Long) {
        val p = ByteBuffer.wrap(frame.payload).order(ByteOrder.LITTLE_ENDIAN)
        when (frame.type) {
            0x01 -> {
                updating = false
                helloAck()
            }
            0x02 -> if (state != RobotState.ARMED && !updating) {
                driveDeadline = now + cmdTimeoutMs
                nextTelemetry = now
                setState(RobotState.ARMED)
            }
            0x03 -> setState(RobotState.DISARMED)
            0x04 -> if (state == RobotState.ARMED && frame.payload.size == 7) {
                val speed = p.short.toInt()
                val steer = p.short.toInt()
                speedLeft = (speed + steer).coerceIn(-1000, 1000)
                speedRight = (speed - steer).coerceIn(-1000, 1000)
                driveDeadline = now + cmdTimeoutMs
            }
            0x05 -> send(0x85, ByteArray(0))
            0x07 -> if (state == RobotState.DISARMED && frame.payload.size == 5) reboot()
            0x08 -> fwBegin(p.int, p.int.toLong() and 0xFFFFFFFFL)
            0x09 -> fwData(p.int, frame.payload.copyOfRange(4, frame.payload.size))
            0x0A -> fwEnd()
            0x0B -> {
                flash.boot = FirmwareBoot.NORMAL
                fwAck(0, FwStatus.OK)
            }
        }
        // No wake-up needed: ARM queues an EVENT (the reader then picks up the new deadlines), and a DRIVE
        // only moves the deadline later.
    }

    private fun setState(new: RobotState) {
        if (new != RobotState.ARMED) {
            speedLeft = 0
            speedRight = 0
        }
        if (new == state) return
        state = new
        event(EventCode.STATE, new.code)
    }

    private fun helloAck() = send(
        0x81,
        ByteBuffer.allocate(21).order(ByteOrder.LITTLE_ENDIAN)
            .put(PROTOCOL_VERSION.toByte()).put(0).put(0).put(0).putInt(Caps.MOTOR_SIMULATED or Caps.BATTERY_VOLTAGE)
            .order(ByteOrder.BIG_ENDIAN).putLong(0)
            .order(ByteOrder.LITTLE_ENDIAN).putInt(flash.crc.toInt()).put(flash.boot.code.toByte()).array(),
    )

    private fun fwBegin(size: Int, crc: Long) {
        val status = when {
            state != RobotState.DISARMED || flash.boot == FirmwareBoot.TRIAL -> FwStatus.BAD_STATE
            size <= 0 || size > FirmwareFlash.SLOT_SIZE -> FwStatus.TOO_BIG
            else -> FwStatus.OK
        }
        if (status == FwStatus.OK) {
            updating = true
            updateSize = size
            updateCrc = crc
            image = ByteArrayOutputStream()
            written = 0
            flash.staged = null
        }
        fwAck(0, status)
    }

    private fun fwData(offset: Int, data: ByteArray) {
        if (!updating || flash.staged != null) return fwAck(written, FwStatus.BAD_STATE)
        if (offset == written && image.size() != written) {
            image = ByteArrayOutputStream().apply { write(image.toByteArray(), 0, written) }
        }
        if (offset != image.size()) return fwAck(written, FwStatus.BAD_OFFSET)
        if (data.size > updateSize - image.size()) return fwAck(written, FwStatus.TOO_BIG)
        image.write(data)
        val sectors = image.size() / FirmwareFlash.SECTOR * FirmwareFlash.SECTOR
        if (sectors > written) {
            written = sectors
            fwAck(written, FwStatus.OK)
        }
    }

    private fun fwEnd() {
        val bytes = image.toByteArray()
        val status = when {
            !updating || flash.staged != null -> FwStatus.BAD_STATE
            bytes.size != updateSize -> FwStatus.BAD_OFFSET
            crc32(bytes) != updateCrc -> FwStatus.CRC_FAIL.also { updating = false }
            else -> FwStatus.OK.also {
                flash.staged = bytes
                updating = false
            }
        }
        written = bytes.size
        fwAck(written, status)
    }

    /** The loader installs a staged image; the phone sees the base go away and come back. */
    private fun reboot() {
        flash.staged?.let {
            flash.crc = crc32(it)
            flash.boot = FirmwareBoot.TRIAL
            flash.staged = null
        }
        close()
    }

    private fun fwAck(offset: Int, status: Int) =
        send(0x86, ByteBuffer.allocate(5).order(ByteOrder.LITTLE_ENDIAN).putInt(offset).put(status.toByte()).array())

    private fun event(code: Int, arg: Int) =
        send(0x83, ByteBuffer.allocate(3).order(ByteOrder.LITTLE_ENDIAN).put(code.toByte()).putShort(arg.toShort()).array())

    private fun telemetry() = send(
        0x82,
        ByteBuffer.allocate(13).order(ByteOrder.LITTLE_ENDIAN)
            .put(state.code.toByte()).putShort(0).putShort(batteryMv.toShort())
            .putShort(speedLeft.toShort()).putShort(speedRight.toShort())
            .putShort(Short.MIN_VALUE).putShort(0).array(),
    )

    private fun send(type: Int, payload: ByteArray) {
        toPhone.put(FrameCodec.encode(type, seq, payload))
        seq = (seq + 1) and 0xFF
    }

    private fun now() = System.nanoTime() / 1_000_000

    /** What survives a simulated reboot. */
    private class Flash {
        @Volatile var crc = 0L // no image yet: the first app start "updates" the simulated base
        @Volatile var boot = FirmwareBoot.NORMAL
        @Volatile var staged: ByteArray? = null
    }

    companion object {
        private val flash = Flash()

        private fun crc32(data: ByteArray) = java.util.zip.CRC32().apply { update(data) }.value

        /** Tests: a base that never got an image. */
        internal fun eraseFlash() {
            flash.crc = 0
            flash.boot = FirmwareBoot.NORMAL
            flash.staged = null
        }
    }
}
