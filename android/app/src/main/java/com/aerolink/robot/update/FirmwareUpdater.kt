package com.aerolink.robot.update

import android.content.Context
import android.util.Log
import com.aerolink.robot.link.FirmwareBoot
import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.Report
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.link.RobotState
import com.aerolink.robot.signal.EventLevel
import com.aerolink.robot.signal.RobotEvent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import java.io.IOException
import java.util.zip.CRC32

/** What the diagnostics screen shows about the robot base's firmware (ROB-009). */
sealed class FirmwareStatus {
    /** No robot base, or it does not answer HELLO. */
    object NoBase : FirmwareStatus()

    /** This build carries no firmware image (debug build without `make rp2040`). */
    object NoImage : FirmwareStatus()

    /** The base runs the image this app carries. */
    object Current : FirmwareStatus()

    /** Waiting for the operator session to end and the base to be disarmed. */
    object Waiting : FirmwareStatus()

    data class Sending(val percent: Int) : FirmwareStatus()

    /** Staged: the base restarts and its loader installs it. */
    object Installing : FirmwareStatus()

    data class Failed(val error: String) : FirmwareStatus()

    /** For the server's `status` (ADM-001); null without a robot base. */
    val wire: String?
        get() = when (this) {
            NoBase -> null
            NoImage -> "no_image"
            Current -> "current"
            Waiting -> "waiting"
            is Sending -> "sending"
            Installing -> "installing"
            is Failed -> "failed"
        }
}

/** The decision for a robot base that answered HELLO (pure, JVM-tested). */
object FirmwarePolicy {
    /** Per image: a transfer or an image that keeps failing is not retried forever. */
    const val MAX_ATTEMPTS = 3

    enum class Decision { NO_IMAGE, CURRENT, GAVE_UP, FLASH }

    /** [bundledCrc]: CRC-32 of the image in the APK; [attempts]: attempts already made for that image. */
    fun decide(bundledCrc: Long?, running: Report.HelloAck, attempts: Int): Decision = when {
        bundledCrc == null -> Decision.NO_IMAGE
        running.firmwareCrc == bundledCrc -> Decision.CURRENT
        attempts >= MAX_ATTEMPTS -> Decision.GAVE_UP
        else -> Decision.FLASH
    }
}

/**
 * ROB-009: keeps the robot base on the firmware built together with this app (`assets/firmware/`, put there by
 * `make android`). On every link: a new image on trial is confirmed as soon as it answered HELLO (LINK-044); an
 * image other than the bundled one is replaced, only while no operator session runs and the base is DISARMED.
 * The transfer itself is not cancelled by a session starting (it takes about a second; ARM is refused meanwhile),
 * the reboot waits for the session to end. A rolled-back image counts as a failed attempt.
 */
class FirmwareUpdater(
    context: Context,
    private val scope: CoroutineScope,
    private val links: StateFlow<RobotLink?>,
    private val idle: Flow<Boolean>,
    /** ADM-003: failed transfers and rollbacks are reported to the admins. */
    private val report: (RobotEvent) -> Unit = {},
) {
    private val _status = MutableStateFlow<FirmwareStatus>(FirmwareStatus.NoBase)
    val status: StateFlow<FirmwareStatus> = _status.asStateFlow()

    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val image: ByteArray? = try {
        context.assets.open(ASSET).use { it.readBytes() }
    } catch (e: IOException) {
        null
    }
    private val bundledCrc: Long? = image?.let { CRC32().apply { update(it) }.value }

    fun start() {
        scope.launch {
            links.collectLatest { link ->
                _status.value = FirmwareStatus.NoBase
                if (link != null) handle(link)
            }
        }
    }

    private suspend fun handle(link: RobotLink) {
        val first = link.status.first { it.firmware != null || it.phase in GONE }
        val running = first.firmware ?: return
        when (running.boot) {
            FirmwareBoot.NORMAL -> Unit
            FirmwareBoot.TRIAL -> Log.i(TAG, "new firmware ${crcHex(running.firmwareCrc)} started, confirming")
            FirmwareBoot.ROLLED_BACK -> {
                Log.w(TAG, "the new firmware did not start, the base went back to ${crcHex(running.firmwareCrc)}")
                prefs.edit().putString(KEY_ERROR, "the new firmware did not start; rolled back").apply()
                report(event(EventLevel.ERROR, "firmware_rolled_back", "back to ${crcHex(running.firmwareCrc)}"))
            }
        }
        if (running.boot != FirmwareBoot.NORMAL) {
            try {
                link.confirmFirmware()
            } catch (e: IOException) {
                Log.w(TAG, "confirm failed: ${e.message}")
            }
        }

        val attempts = if (prefs.getLong(KEY_CRC, -1) == bundledCrc) prefs.getInt(KEY_ATTEMPTS, 0) else 0
        when (FirmwarePolicy.decide(bundledCrc, running, attempts)) {
            FirmwarePolicy.Decision.NO_IMAGE -> _status.value = FirmwareStatus.NoImage
            FirmwarePolicy.Decision.CURRENT -> {
                _status.value = FirmwareStatus.Current
                if (attempts > 0) prefs.edit().clear().apply()
            }
            FirmwarePolicy.Decision.GAVE_UP ->
                _status.value = FirmwareStatus.Failed(prefs.getString(KEY_ERROR, null) ?: "gave up")
            FirmwarePolicy.Decision.FLASH -> flash(link, running, attempts + 1)
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private suspend fun flash(link: RobotLink, running: Report.HelloAck, attempt: Int) {
        val image = image ?: return
        _status.value = FirmwareStatus.Waiting
        // Also returns when the link closes, so a detached base does not keep us waiting.
        val ready = combine(idle, link.status) { free, s ->
            if (s.phase in GONE) false else if (free && s.robotState == RobotState.DISARMED) true else null
        }.first { it != null }
        if (ready != true) return

        prefs.edit().putLong(KEY_CRC, bundledCrc!!).putInt(KEY_ATTEMPTS, attempt).apply()
        Log.i(TAG, "updating firmware ${crcHex(running.firmwareCrc)} -> ${crcHex(bundledCrc)} (${image.size} bytes), attempt $attempt")
        _status.value = FirmwareStatus.Sending(0)
        try {
            link.flashFirmware(image) { _status.value = FirmwareStatus.Sending(it) }
        } catch (e: IOException) {
            val error = e.message ?: e.toString()
            Log.w(TAG, "firmware update failed: $error")
            prefs.edit().putString(KEY_ERROR, error).apply()
            _status.value = FirmwareStatus.Failed(error)
            report(event(EventLevel.ERROR, "firmware_update_failed", "attempt $attempt: $error"))
            return
        }
        _status.value = FirmwareStatus.Installing
        idle.first { it }
        Log.i(TAG, "firmware staged, restarting the robot base")
        link.reboot()
        // The link closes when the base restarts; the next one reports the new image (in trial).
    }

    private companion object {
        const val TAG = "AeroLinkFirmware"
        const val ASSET = "firmware/aerolink-rp2040.bin"
        const val PREFS = "firmware_update"
        const val KEY_CRC = "crc"
        const val KEY_ATTEMPTS = "attempts"
        const val KEY_ERROR = "error"
        val GONE = setOf(LinkPhase.NO_RESPONSE, LinkPhase.CLOSED)

        fun crcHex(crc: Long?) = "%08x".format(crc ?: 0)

        fun event(level: EventLevel, code: String, message: String) =
            RobotEvent(level, code, message, System.currentTimeMillis() / 1000)
    }
}
