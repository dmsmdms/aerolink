package com.aerolink.robot.link

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** Fake RP2040 on the other end of the transport; frames the phone wrote land in [received]. */
private class FakeBase : LinkTransport {
    private val toPhone = LinkedBlockingQueue<ByteArray>()
    private val decoder = FrameDecoder()
    val received = LinkedBlockingQueue<Frame>()

    @Volatile
    var closed = false

    override fun read(buffer: ByteArray): Int {
        val data = toPhone.take()
        if (data.isEmpty()) return -1
        data.copyInto(buffer)
        return data.size
    }

    override fun write(data: ByteArray) {
        if (closed) throw IOException("closed")
        synchronized(decoder) { decoder.push(data) { received.put(it) } }
    }

    override fun close() {
        closed = true
        detach()
    }

    fun detach() = toPhone.put(ByteArray(0))

    fun send(type: Int, payload: ByteArray = ByteArray(0)) = toPhone.put(FrameCodec.encode(type, 0, payload))

    fun helloAck(protocol: Int = PROTOCOL_VERSION, boot: FirmwareBoot = FirmwareBoot.NORMAL) = send(
        0x81,
        ByteBuffer.allocate(21).order(ByteOrder.LITTLE_ENDIAN)
            .put(protocol.toByte()).put(0).put(1).put(0).putInt(Caps.MOTOR_SIMULATED)
            .order(ByteOrder.BIG_ENDIAN).putLong(0x1122334455667788)
            .order(ByteOrder.LITTLE_ENDIAN).putInt(0xCAFEF00D.toInt()).put(boot.code.toByte()).array(),
    )

    fun fwAck(offset: Int, status: Int = FwStatus.OK) =
        send(0x86, ByteBuffer.allocate(5).order(ByteOrder.LITTLE_ENDIAN).putInt(offset).put(status.toByte()).array())

    fun event(code: Int, arg: Int) =
        send(0x83, ByteBuffer.allocate(3).order(ByteOrder.LITTLE_ENDIAN).put(code.toByte()).putShort(arg.toShort()).array())

    fun next(timeoutMs: Long = 1000): Frame? = received.poll(timeoutMs, TimeUnit.MILLISECONDS)

    fun expect(type: Int, timeoutMs: Long = 1000): Frame {
        val f = next(timeoutMs) ?: error("no frame 0x%02X within $timeoutMs ms".format(type))
        assertEquals("frame type", type, f.type)
        return f
    }
}

private fun Frame.drive(): Triple<Int, Int, Int> {
    val p = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN)
    return Triple(p.short.toInt(), p.short.toInt(), p.get().toInt())
}

class RobotLinkTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val base = FakeBase()
    private val timing = LinkTiming(helloTimeoutMs = 300, helloAttempts = 2, drivePeriodMs = 20, driveTtlMs = 150)
    private val link = RobotLink(base, scope, timing)

    @After
    fun tearDown() = scope.cancel()

    private fun awaitPhase(phase: LinkPhase) = runBlocking {
        withTimeout(2000) { link.status.first { it.phase == phase } }
    }

    private fun awaitState(state: RobotState) = runBlocking {
        withTimeout(2000) { link.status.first { it.robotState == state } }
    }

    private fun connect() {
        link.start()
        base.expect(0x01) // HELLO
        base.helloAck()
        awaitPhase(LinkPhase.READY)
    }

    private fun armed() {
        connect()
        link.arm()
        base.expect(0x02)
        base.event(EventCode.STATE, RobotState.ARMED.code)
        awaitState(RobotState.ARMED)
    }

    @Test
    fun handshakeReadsFirmwareInfo() {
        connect()
        val fw = link.status.value.firmware!!
        assertEquals("0.1.0", fw.firmwareVersion)
        assertEquals(Caps.MOTOR_SIMULATED, fw.caps)
        assertEquals(0x1122334455667788, fw.boardId)
        assertEquals(0xCAFEF00DL, fw.firmwareCrc)
        assertEquals(FirmwareBoot.NORMAL, fw.boot)
    }

    @Test
    fun incompatibleFirmwareIsNeverArmed() {
        link.start()
        base.expect(0x01)
        base.helloAck(protocol = PROTOCOL_VERSION + 1)
        awaitPhase(LinkPhase.INCOMPATIBLE)
        link.arm()
        assertNull(base.next(200))
    }

    @Test
    fun silentBaseEndsInNoResponse() {
        link.start()
        base.expect(0x01)
        base.expect(0x01) // retry
        awaitPhase(LinkPhase.NO_RESPONSE)
        assertNull(base.next(100))
    }

    @Test
    fun disarmedLinkIsSilent() {
        connect()
        link.setDrive(500, 0, DriveSource.LOCAL)
        assertNull(base.next(200)) // LINK-020
    }

    @Test
    fun driveIsStreamedOnlyWhileArmed() {
        armed()
        assertEquals(Triple(0, 0, DriveSource.LOCAL.code), base.expect(0x04).drive())
        link.setDrive(500, -100, DriveSource.LOCAL)
        var frame = base.expect(0x04)
        while (frame.drive().first == 0) frame = base.expect(0x04)
        assertEquals(Triple(500, -100, DriveSource.LOCAL.code), frame.drive())

        base.event(EventCode.STATE, RobotState.STOPPING.code)
        awaitState(RobotState.STOPPING)
        while (base.next(100) != null) Unit // drain frames sent before the event
        assertNull(base.next(200))
    }

    @Test
    fun armResetsHeldJoystick() {
        connect()
        link.setDrive(800, 300, DriveSource.LOCAL)
        link.arm()
        base.expect(0x02)
        base.event(EventCode.STATE, RobotState.ARMED.code)
        assertEquals(Triple(0, 0, DriveSource.LOCAL.code), base.expect(0x04).drive())
    }

    @Test
    fun telemetryAndEventsUpdateStatus() {
        connect()
        base.event(EventCode.FAULTS, Faults.LOW_BATTERY)
        runBlocking { withTimeout(2000) { link.status.first { it.faults == Faults.LOW_BATTERY } } }
        val telemetry = ByteBuffer.allocate(13).order(ByteOrder.LITTLE_ENDIAN)
            .put(RobotState.FAULT.code.toByte()).putShort(Faults.MOTOR_FAULT.toShort()).putShort(36000.toShort())
            .putShort(10).putShort(-10).putShort(Short.MIN_VALUE).putShort(3).array()
        base.send(0x82, telemetry)
        awaitState(RobotState.FAULT)
        val t = link.status.value.telemetry!!
        assertEquals(36000, t.batteryMv)
        assertNull(t.tempDeciC)
        assertEquals(Faults.MOTOR_FAULT, link.status.value.faults)
    }

    @Test
    fun detachClosesLink() {
        armed()
        base.detach()
        awaitPhase(LinkPhase.CLOSED)
    }

    @Test
    fun closeDisarmsFirst() {
        connect()
        runBlocking { link.close() }
        base.expect(0x03)
        assertEquals(LinkPhase.CLOSED, link.status.value.phase)
    }

    private fun Frame.u32(at: Int = 0) = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN).getInt(at)

    /** Reads FW_DATA frames until the end of the sector starting at [from]; returns the next offset. */
    private fun receiveSector(image: ByteArray, from: Int): Int {
        var offset = from
        val end = minOf(image.size, (from / FirmwareFlash.SECTOR + 1) * FirmwareFlash.SECTOR)
        while (offset < end) {
            val f = base.expect(0x09)
            assertEquals(offset, f.u32())
            val data = f.payload.copyOfRange(4, f.payload.size)
            assertArrayEquals(image.copyOfRange(offset, offset + data.size), data)
            offset += data.size
        }
        return offset
    }

    @Test
    fun firmwareIsSentSectorBySector(): Unit = runBlocking {
        connect()
        val image = ByteArray(2 * FirmwareFlash.SECTOR + 100) { (it * 7).toByte() }
        val progress = mutableListOf<Int>()
        val job = scope.launch { link.flashFirmware(image) { progress += it } }

        val begin = base.expect(0x08)
        assertEquals(image.size, begin.u32())
        assertEquals(java.util.zip.CRC32().apply { update(image) }.value.toInt(), begin.u32(4))
        // ARM is refused while the update runs (SAFE-007).
        link.arm()
        base.fwAck(0)
        assertEquals(FirmwareFlash.SECTOR, receiveSector(image, 0))
        assertNull(base.next(100)) // waits for the sector's FW_ACK
        // A frame got lost: the firmware answers BAD_OFFSET with what it has in flash; the phone resends from there.
        base.fwAck(0, FwStatus.BAD_OFFSET)
        assertEquals(FirmwareFlash.SECTOR, receiveSector(image, 0))
        base.fwAck(FirmwareFlash.SECTOR)
        assertEquals(2 * FirmwareFlash.SECTOR, receiveSector(image, FirmwareFlash.SECTOR))
        base.fwAck(2 * FirmwareFlash.SECTOR)
        assertEquals(image.size, receiveSector(image, 2 * FirmwareFlash.SECTOR))
        base.expect(0x0A) // FW_END
        base.fwAck(image.size)
        withTimeout(2000) { job.join() }
        assertEquals(100, progress.last())
        assertNull(base.next(100)) // no ARM went out
        link.arm() // staged: driving is allowed again until the reboot
        base.expect(0x02)
    }

    @Test
    fun firmwareRejectedByTheBaseFails(): Unit = runBlocking {
        connect()
        val result = scope.async { runCatching { link.flashFirmware(ByteArray(10)) } }
        base.expect(0x08)
        base.fwAck(0, FwStatus.BAD_STATE)
        val error = withTimeout(2000) { result.await() }.exceptionOrNull()
        assertTrue(error is FirmwareUpdateException)
        // Not updating any more: ARM goes out again.
        link.arm()
        base.expect(0x02)
    }

    @Test
    fun trialFirmwareIsConfirmed(): Unit = runBlocking {
        link.start()
        base.expect(0x01)
        base.helloAck(boot = FirmwareBoot.TRIAL)
        awaitPhase(LinkPhase.READY)
        val job = scope.launch { link.confirmFirmware() }
        base.expect(0x0B)
        base.fwAck(0)
        withTimeout(2000) { job.join() }
    }
}
