package com.aerolink.robot.teleop

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Where operator commands go: the robot base, through `RobotLink`, with `source = OPERATOR`. */
interface DriveSink {
    fun arm()
    fun drive(speed: Int, steer: Int)

    /** Zero DRIVE, then DISARM. */
    fun stop()
}

/**
 * Operator commands → robot base, with the phone's own safety rules on top of the firmware's (docs/05-safety.md):
 * - `SAFE-012` a DRIVE whose `seq` is not newer than the last one is dropped (the channel is unordered);
 * - `SAFE-010` once armed, more than [timeoutMs] without a DRIVE stops the robot (zero DRIVE, DISARM).
 *   Right after ARM the console needs one telemetry round trip before it streams, hence [armGraceMs].
 *
 * One instance per operator session; all calls on the scope's thread. The watchdog is one coroutine
 * sleeping until the deadline, not a timer per DRIVE.
 */
class DriveGate(
    private val scope: CoroutineScope,
    private val sink: DriveSink,
    private val timeoutMs: Long = 500,
    private val armGraceMs: Long = 1500,
) {
    private var lastSeq = 0L // the console counts from 1
    private var deadline = 0L
    private var watchdog: Job? = null

    fun arm() {
        sink.arm()
        expectDrive(armGraceMs)
    }

    fun disarm() = stop()

    /** False if the DRIVE was dropped as stale. */
    fun drive(msg: DriveMsg): Boolean {
        if (msg.seq <= lastSeq) return false
        lastSeq = msg.seq
        sink.drive(msg.speed, msg.steer)
        expectDrive(timeoutMs)
        return true
    }

    /** Session over, link lost, or the operator's disarm. */
    fun stop() {
        watchdog?.cancel()
        watchdog = null
        sink.stop()
    }

    private fun expectDrive(withinMs: Long) {
        deadline = now() + withinMs
        if (watchdog != null) return
        watchdog = scope.launch {
            while (true) {
                val wait = deadline - now()
                if (wait <= 0) break
                delay(wait)
            }
            watchdog = null
            sink.stop()
        }
    }

    private fun now() = System.nanoTime() / 1_000_000
}
