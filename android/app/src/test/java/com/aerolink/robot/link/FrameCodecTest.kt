package com.aerolink.robot.link

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

// Same reference frames as the firmware tests: protocol/vectors.txt (tools/gen_vectors.py).
class FrameCodecTest {
    private sealed class Vector(val name: String, val wire: ByteArray) {
        class Ok(name: String, val type: Int, val seq: Int, val payload: ByteArray, wire: ByteArray) : Vector(name, wire)
        class Bad(name: String, wire: ByteArray) : Vector(name, wire)
    }

    private val vectors: List<Vector> by lazy {
        val path = System.getProperty("aerolink.vectors") ?: error("aerolink.vectors system property not set")
        File(path).readLines()
            .filter { it.isNotBlank() && !it.startsWith("#") }
            .map { line ->
                val f = line.trim().split(Regex("\\s+"))
                when (f[0]) {
                    "ok" -> Vector.Ok(f[1], f[2].toInt(16), f[3].toInt(), hex(f[4]), hex(f[5]))
                    "bad" -> Vector.Bad(f[1], hex(f[2]))
                    else -> error("bad vector line: $line")
                }
            }
    }

    private val ok get() = vectors.filterIsInstance<Vector.Ok>()
    private fun vector(name: String) = ok.first { it.name == name }

    private fun hex(s: String): ByteArray =
        if (s == "-") ByteArray(0) else s.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    private fun decodeAll(wire: ByteArray, chunk: Int = wire.size): Pair<List<Frame>, Long> {
        val decoder = FrameDecoder()
        val frames = mutableListOf<Frame>()
        var i = 0
        while (i < wire.size) {
            val n = minOf(chunk, wire.size - i)
            decoder.push(wire, i, n) { frames += it }
            i += n
        }
        return frames to decoder.errors
    }

    @Test
    fun crcCheckValue() {
        assertEquals(0x29B1, FrameCodec.crc16("123456789".toByteArray()))
    }

    @Test
    fun encodeMatchesVectors() {
        assertTrue(ok.isNotEmpty())
        for (v in ok) {
            assertArrayEquals(v.name, v.wire, FrameCodec.encode(v.type, v.seq, v.payload))
        }
    }

    @Test
    fun decodeMatchesVectors() {
        for (v in ok) {
            val (frames, errors) = decodeAll(v.wire)
            assertEquals(v.name, 1, frames.size)
            assertEquals(v.name, 0L, errors)
            assertEquals(v.name, v.type, frames[0].type)
            assertEquals(v.name, v.seq, frames[0].seq)
            assertArrayEquals(v.name, v.payload, frames[0].payload)
        }
    }

    @Test
    fun badVectorsAreRejected() {
        val bad = vectors.filterIsInstance<Vector.Bad>()
        assertTrue(bad.isNotEmpty())
        for (v in bad) {
            val (frames, errors) = decodeAll(v.wire)
            assertEquals(v.name, 0, frames.size)
            assertEquals(v.name, 1L, errors)
        }
    }

    @Test
    fun streamInArbitraryChunks() {
        val stream = ok.fold(ByteArray(0)) { acc, v -> acc + v.wire }
        for (chunk in listOf(1, 3, 7, 64, stream.size)) {
            val (frames, errors) = decodeAll(stream, chunk)
            assertEquals(ok.map { it.seq }, frames.map { it.seq })
            assertEquals(0L, errors)
        }
    }

    @Test
    fun commandsEncodeLikeVectors() {
        val cases = mapOf(
            "hello" to Command.Hello(),
            "arm" to Command.Arm,
            "disarm" to Command.Disarm,
            "drive_forward" to Command.Drive(500, 0, DriveSource.OPERATOR, 250),
            "drive_reverse_left" to Command.Drive(-1000, -250, DriveSource.LOCAL, 100),
            "drive_zero" to Command.Drive(0, 0, DriveSource.AUTOPILOT, 0),
            "ping" to Command.Ping,
            "config" to Command.Config(600, 1500, 200, 250),
            "reboot" to Command.Reboot(),
            "reboot_bootsel" to Command.Reboot(bootsel = true),
            "fw_begin" to Command.FwBegin(39936, 0x1C291CA3),
            "fw_data" to Command.FwData(4096, ByteArray(56) { (it + 1).toByte() }),
            "fw_data_last" to Command.FwData(39928, ByteArray(8)),
            "fw_end" to Command.FwEnd,
            "fw_confirm" to Command.FwConfirm,
        )
        for ((name, cmd) in cases) {
            val v = vector(name)
            assertEquals(name, v.type, cmd.type)
            assertArrayEquals(name, v.payload, cmd.payload())
        }
    }

    @Test
    fun reportsParseFromVectors() {
        fun parse(name: String) = vector(name).let { Report.parse(Frame(it.type, it.seq, it.payload)) }

        assertEquals(
            Report.HelloAck(
                2, "0.1.0", Caps.MOTOR_SIMULATED, 0xe66038b3cb3d2a2fUL.toLong(), 0x1C291CA3, FirmwareBoot.TRIAL,
            ),
            parse("hello_ack"),
        )
        assertEquals(
            Report.Telemetry(RobotState.ARMED, 0, 38400, 480, 520, null, 3),
            parse("telemetry"),
        )
        assertEquals(
            Report.Telemetry(RobotState.FAULT, Faults.MOTOR_LINK_LOST or Faults.MOTOR_FAULT, null, 0, 0, 412, 0),
            parse("telemetry_fault"),
        )
        assertEquals(Report.Event(EventCode.STATE, RobotState.ARMED.code), parse("event_state"))
        assertEquals(Report.Nack(0x02, NackReason.FAULT_ACTIVE), parse("nack"))
        assertEquals(Report.Pong, parse("pong"))
        assertEquals(Report.FwAck(4096, FwStatus.OK), parse("fw_ack"))
        assertEquals(Report.FwAck(36864, FwStatus.BAD_OFFSET), parse("fw_ack_bad_offset"))
    }

    @Test
    fun wrongLengthOrUnknownTypeIsNotParsed() {
        assertEquals(null, Report.parse(Frame(0x82, 0, ByteArray(12))))
        assertEquals(null, Report.parse(Frame(0x9F, 0, ByteArray(0))))
        assertNotNull(Report.parse(Frame(0x85, 0, ByteArray(0))))
    }

    @Test
    fun driveClampsOutOfRange() {
        assertArrayEquals(
            Command.Drive(1000, -1000, DriveSource.LOCAL, 0xFFFF).payload(),
            Command.Drive(5000, -5000, DriveSource.LOCAL, 100000).payload(),
        )
    }
}
