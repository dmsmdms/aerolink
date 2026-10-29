package com.aerolink.robot.power

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ChargePolicyTest {
    private val cool = 300

    /** Feeds a level sequence through the hysteresis, returns the decision after each step. */
    private fun run(start: Boolean, vararg levels: Int) =
        levels.runningFold(start) { allowed, level -> ChargePolicy.next(allowed, level, cool) }.drop(1)

    @Test
    fun initialDecision() {
        assertTrue(ChargePolicy.initial(59, cool))
        assertFalse(ChargePolicy.initial(60, cool))
        assertFalse(ChargePolicy.initial(30, 410))
    }

    @Test
    fun windowHysteresis() {
        assertEquals(listOf(true, true, false), run(true, 50, 59, 60))
        assertEquals(listOf(false, false, false, true), run(false, 59, 50, 46, 45))
        assertEquals(listOf(true, true), run(true, 46, 50)) // still charging inside the window
    }

    @Test
    fun heatStopsChargingAtAnyLevel() {
        assertFalse(ChargePolicy.next(true, 20, 401))
        assertFalse(ChargePolicy.next(false, 20, 380)) // not cool enough yet
        assertTrue(ChargePolicy.next(false, 20, 369))
        assertTrue(ChargePolicy.next(true, 20, 400)) // 40.0 °C is still allowed
    }
}
