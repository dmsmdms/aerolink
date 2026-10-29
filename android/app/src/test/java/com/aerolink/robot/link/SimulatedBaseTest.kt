package com.aerolink.robot.link

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SimulatedBaseTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val base = SimulatedBase(cmdTimeoutMs = 150, telemetryPeriodMs = 50)
    private val link = RobotLink(base, scope)

    @After
    fun tearDown() = scope.cancel()

    private fun await(what: (LinkStatus) -> Boolean) = runBlocking { withTimeout(2000) { link.status.first(what) } }

    @Test
    fun drivesLikeTheFirmware() {
        link.start()
        val ready = await { it.phase == LinkPhase.READY }
        assertTrue(ready.firmware!!.caps and Caps.MOTOR_SIMULATED != 0)

        link.arm()
        await { it.robotState == RobotState.ARMED }
        link.setDrive(400, 100, DriveSource.OPERATOR)
        val moving = await { it.telemetry?.speedLeft == 500 }
        assertEquals(300, moving.telemetry!!.speedRight)
        assertEquals(36500, moving.telemetry!!.batteryMv)

        link.disarm()
        await { it.robotState == RobotState.DISARMED }
    }

    @Test
    fun commandTimeoutDisarms() { // SAFE-002, without RobotLink's DRIVE stream
        val buffer = ByteArray(AccessoryTransport.READ_BUFFER_SIZE)
        val decoder = FrameDecoder()
        val events = mutableListOf<Report>()
        fun pump(until: (Report) -> Boolean) {
            while (true) {
                val n = base.read(buffer)
                var done = false
                decoder.push(buffer, 0, n) { f -> Report.parse(f)?.let { events += it; if (until(it)) done = true } }
                if (done) return
            }
        }
        base.write(FrameCodec.encode(0x02, 0, ByteArray(0)))
        val start = System.nanoTime()
        pump { it == Report.Event(EventCode.STATE, RobotState.DISARMED.code) }
        val ms = (System.nanoTime() - start) / 1_000_000
        assertTrue("timeout after $ms ms", ms in 140..400)
        assertTrue(events.contains(Report.Event(EventCode.CMD_TIMEOUT, 0)))
        assertTrue(events.any { it is Report.Telemetry })
    }

    @Test
    fun takesFirmwareUpdates(): Unit = runBlocking { // ROB-009 on the bench
        SimulatedBase.eraseFlash()
        link.start()
        assertEquals(0L, await { it.phase == LinkPhase.READY }.firmware!!.firmwareCrc)
        val image = ByteArray(3 * FirmwareFlash.SECTOR + 1234) { (it * 31 + 5).toByte() }
        link.flashFirmware(image)
        link.reboot()
        await { it.phase == LinkPhase.CLOSED } // the base "restarts"

        val next = RobotLink(SimulatedBase(), scope).apply { start() }
        val fw = withTimeout(2000) { next.status.first { it.phase == LinkPhase.READY } }.firmware!!
        assertEquals(java.util.zip.CRC32().apply { update(image) }.value, fw.firmwareCrc)
        assertEquals(FirmwareBoot.TRIAL, fw.boot)
        next.confirmFirmware()
        next.close()
        val third = RobotLink(SimulatedBase(), scope).apply { start() }
        assertEquals(FirmwareBoot.NORMAL, withTimeout(2000) { third.status.first { it.firmware != null } }.firmware!!.boot)
        third.close()
        SimulatedBase.eraseFlash()
    }
}
