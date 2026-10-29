package com.aerolink.robot.teleop

import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.LinkStatus
import com.aerolink.robot.link.RobotState
import org.json.JSONException
import org.json.JSONObject

// Peer-to-peer DataChannel messages, console ↔ robot (docs/08-backend.md "Control channel", CTL-001/002).

/** `ctl` channel, console → robot. */
sealed class CtlIn {
    data object Arm : CtlIn()
    data object Disarm : CtlIn()

    /** CTL-004: the phone's flashlight as a headlight. */
    data class Light(val on: Boolean) : CtlIn()

    /** [ts] is the console's clock, echoed back untouched. */
    data class Ping(val ts: Any) : CtlIn()

    companion object {
        fun parse(text: String): CtlIn? = try {
            val o = JSONObject(text)
            when (o.optString("t")) {
                "arm" -> Arm
                "disarm" -> Disarm
                "ping" -> Ping(o.get("ts"))
                "light" -> Light(o.getBoolean("on"))
                else -> null
            }
        } catch (e: JSONException) {
            null
        }
    }
}

/** `drive` channel: speed + = forward, steer + = right, ‰. */
data class DriveMsg(val seq: Long, val speed: Int, val steer: Int) {
    companion object {
        fun parse(text: String): DriveMsg? = try {
            val o = JSONObject(text)
            DriveMsg(o.getLong("seq"), o.getInt("speed").coerceIn(-1000, 1000), o.getInt("steer").coerceIn(-1000, 1000))
        } catch (e: JSONException) {
            null
        }
    }
}

object CtlOut {
    fun pong(ts: Any): String = JSONObject().put("t", "pong").put("ts", ts).toString()

    /**
     * `telemetry`; `base` = false when no robot base is connected (or it is not usable), so the console
     * does not offer ARM. [light] is null when the camera has no flashlight (the console hides the button).
     */
    fun telemetry(link: LinkStatus?, phoneBattery: Int?, light: Boolean? = null): String {
        val ready = link?.phase == LinkPhase.READY
        val state = if (ready) link!!.robotState else RobotState.DISARMED
        val t = link?.telemetry
        // The base sends TELEMETRY only while ARMED (LINK-020); otherwise the last wheel speeds are stale.
        val moving = state == RobotState.ARMED || state == RobotState.STOPPING
        return JSONObject()
            .put("t", "telemetry")
            .put("base", ready)
            .put("state", state.name)
            .put("faults", if (ready) link!!.faults else 0)
            .putOpt("speed_l", if (ready) (if (moving) t?.speedLeft ?: 0 else 0) else null)
            .putOpt("speed_r", if (ready) (if (moving) t?.speedRight ?: 0 else 0) else null)
            .putOpt("battery_mv", t?.batteryMv?.takeIf { ready })
            .putOpt("phone_battery", phoneBattery)
            .putOpt("light", light)
            .toString()
    }
}
