package com.aerolink.robot.update

import com.aerolink.robot.signal.AppRelease
import com.aerolink.robot.update.UpdatePolicy.Decision
import org.junit.Assert.assertEquals
import org.junit.Test

class UpdatePolicyTest {
    private val v7 = AppRelease.Apk(10, "aa")
    private val v8 = AppRelease.Apk(12, "bb")
    private val release = AppRelease(5, "0.5.0", mapOf("armeabi-v7a" to v7, "arm64-v8a" to v8))

    @Test
    fun onlyNewerReleases() {
        assertEquals(Decision.Current, UpdatePolicy.decide(release, installedCode = 5, abis = listOf("arm64-v8a"), attempts = 0))
        assertEquals(Decision.Current, UpdatePolicy.decide(release, installedCode = 6, abis = listOf("arm64-v8a"), attempts = 0))
        assertEquals(Decision.Fetch("arm64-v8a", v8), UpdatePolicy.decide(release, 4, listOf("arm64-v8a"), 0))
    }

    @Test
    fun picksThePhonesPreferredAbi() {
        // The SM-J710F: 32-bit only.
        assertEquals(Decision.Fetch("armeabi-v7a", v7), UpdatePolicy.decide(release, 4, listOf("armeabi-v7a", "armeabi"), 0))
        assertEquals(Decision.Fetch("arm64-v8a", v8), UpdatePolicy.decide(release, 4, listOf("arm64-v8a", "armeabi-v7a"), 0))
        assertEquals(Decision.NoApk, UpdatePolicy.decide(release, 4, listOf("x86_64"), 0))
    }

    @Test
    fun givesUpAfterSomeAttempts() {
        val last = UpdatePolicy.MAX_ATTEMPTS - 1
        assertEquals(Decision.Fetch("arm64-v8a", v8), UpdatePolicy.decide(release, 4, listOf("arm64-v8a"), last))
        assertEquals(Decision.GaveUp, UpdatePolicy.decide(release, 4, listOf("arm64-v8a"), last + 1))
    }
}
