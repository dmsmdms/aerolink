package com.aerolink.robot.teleop

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.Executors

class DriveGateTest {
    private val executor = Executors.newSingleThreadExecutor()
    private val dispatcher = executor.asCoroutineDispatcher()
    private val scope = CoroutineScope(dispatcher)
    private val calls = mutableListOf<String>() // touched on the dispatcher thread only
    private val sink = object : DriveSink {
        override fun arm() { calls += "arm" }
        override fun drive(speed: Int, steer: Int) { calls += "drive $speed $steer" }
        override fun stop() { calls += "stop" }
    }
    private val gate = DriveGate(scope, sink, timeoutMs = 100, armGraceMs = 300)

    @After
    fun tearDown() {
        scope.cancel()
        executor.shutdown()
    }

    private fun <T> on(block: () -> T): T = runBlocking(dispatcher) { block() }
    private fun calls() = on { calls.toList() }

    @Test
    fun staleDrivesAreDropped() { // SAFE-012
        on {
            gate.arm()
            assertTrue(gate.drive(DriveMsg(1, 100, 0)))
            assertTrue(gate.drive(DriveMsg(3, 300, 0)))
            assertFalse(gate.drive(DriveMsg(2, 200, 0))) // arrived late on the unordered channel
            assertFalse(gate.drive(DriveMsg(3, 300, 0)))
            gate.disarm()
        }
        assertEquals(listOf("arm", "drive 100 0", "drive 300 0", "stop"), calls())
    }

    @Test
    fun silenceStopsTheRobot() { // SAFE-010
        on {
            gate.arm()
            gate.drive(DriveMsg(1, 500, 0))
        }
        Thread.sleep(60)
        on { gate.drive(DriveMsg(2, 500, 0)) } // keeps it alive
        Thread.sleep(60)
        assertEquals(listOf("arm", "drive 500 0", "drive 500 0"), calls())
        Thread.sleep(120)
        assertEquals("stop", calls().last())
        Thread.sleep(150)
        assertEquals(1, calls().count { it == "stop" }) // once
    }

    @Test
    fun armWithoutDrivingStopsAfterTheGrace() {
        on { gate.arm() }
        Thread.sleep(200)
        assertEquals(listOf("arm"), calls()) // 100 ms timeout does not apply before the first DRIVE
        Thread.sleep(200)
        assertEquals(listOf("arm", "stop"), calls())
    }

    @Test
    fun disarmCancelsTheWatchdog() {
        on {
            gate.arm()
            gate.drive(DriveMsg(1, 0, 0))
            gate.disarm()
        }
        Thread.sleep(400)
        assertEquals(listOf("arm", "drive 0 0", "stop"), calls())
    }
}
