package com.aerolink.robot.update

import com.aerolink.robot.link.FirmwareBoot
import com.aerolink.robot.link.Report
import com.aerolink.robot.update.FirmwarePolicy.Decision
import org.junit.Assert.assertEquals
import org.junit.Test

class FirmwarePolicyTest {
    private fun running(crc: Long) = Report.HelloAck(2, "0.1.0", 0, 0, crc, FirmwareBoot.NORMAL)

    @Test
    fun keepsTheBaseOnTheBundledImage() {
        assertEquals(Decision.CURRENT, FirmwarePolicy.decide(0xAAAA, running(0xAAAA), attempts = 0))
        // Any other image is replaced, older or newer: the app and its firmware ship together.
        assertEquals(Decision.FLASH, FirmwarePolicy.decide(0xAAAA, running(0xBBBB), attempts = 0))
    }

    @Test
    fun withoutAnImageNothingHappens() {
        assertEquals(Decision.NO_IMAGE, FirmwarePolicy.decide(null, running(0xBBBB), attempts = 0))
    }

    @Test
    fun givesUpAfterSomeAttempts() {
        val last = FirmwarePolicy.MAX_ATTEMPTS - 1
        assertEquals(Decision.FLASH, FirmwarePolicy.decide(0xAAAA, running(0xBBBB), last))
        assertEquals(Decision.GAVE_UP, FirmwarePolicy.decide(0xAAAA, running(0xBBBB), last + 1))
        // Reaching the image clears the matter whatever the count.
        assertEquals(Decision.CURRENT, FirmwarePolicy.decide(0xAAAA, running(0xAAAA), last + 1))
    }
}
