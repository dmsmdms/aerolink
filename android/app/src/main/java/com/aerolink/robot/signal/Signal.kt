package com.aerolink.robot.signal

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

// Robot side of the signaling protocol (docs/08-backend.md "Signaling"): JSON text messages with a `type`.
// The server relays SDP and ICE without parsing them; so does the phone, apart from handing them to WebRTC.

/** `RTCSessionDescriptionInit`. */
data class Sdp(val type: String, val sdp: String) {
    fun toJson(): JSONObject = JSONObject().put("type", type).put("sdp", sdp)

    companion object {
        fun parse(o: JSONObject) = Sdp(o.getString("type"), o.getString("sdp"))
    }
}

/** `RTCIceCandidateInit`. */
data class Ice(val candidate: String, val sdpMid: String?, val sdpMLineIndex: Int?) {
    fun toJson(): JSONObject = JSONObject().put("candidate", candidate)
        .put("sdpMid", sdpMid ?: JSONObject.NULL)
        .put("sdpMLineIndex", sdpMLineIndex ?: JSONObject.NULL)

    companion object {
        fun parse(o: JSONObject) = Ice(
            candidate = o.getString("candidate"),
            sdpMid = o.optStringOrNull("sdpMid"),
            sdpMLineIndex = if (o.isNull("sdpMLineIndex")) null else o.getInt("sdpMLineIndex"),
        )
    }
}

/** `RTCIceServer` with per-session TURN credentials from the server. */
data class IceServer(val urls: List<String>, val username: String?, val credential: String?) {
    companion object {
        fun parse(o: JSONObject): IceServer {
            val urls = when (val u = o.get("urls")) {
                is JSONArray -> List(u.length()) { u.getString(it) }
                else -> listOf(u.toString())
            }
            return IceServer(urls, o.optStringOrNull("username"), o.optStringOrNull("credential"))
        }
    }
}

/** The robot app release on the server (ROB-008): one APK per ABI, downloaded from `/api/v1/robot/apk/<abi>`. */
data class AppRelease(val versionCode: Int, val versionName: String, val apks: Map<String, Apk>) {
    data class Apk(val size: Long, val sha256: String)

    companion object {
        fun parse(o: JSONObject): AppRelease {
            val apks = o.getJSONObject("apks")
            return AppRelease(
                versionCode = o.getInt("version_code"),
                versionName = o.getString("version_name"),
                apks = apks.keys().asSequence().associateWith { abi ->
                    apks.getJSONObject(abi).let { Apk(it.getLong("size"), it.getString("sha256")) }
                },
            )
        }
    }
}

/** Server → robot. */
sealed class SignalIn {
    data class Welcome(val id: Long, val name: String) : SignalIn()

    /** An operator took the robot; the offer follows. */
    data class Session(
        val session: Long,
        val operatorId: Long,
        val operatorName: String,
        val iceServers: List<IceServer>,
    ) : SignalIn()

    data class Offer(val session: Long, val sdp: Sdp) : SignalIn()

    data class Candidate(val session: Long, val ice: Ice) : SignalIn()

    data class End(val session: Long, val reason: String) : SignalIn()

    /** After `welcome` and whenever the server's release changes. */
    data class Update(val release: AppRelease) : SignalIn()

    companion object {
        /** Typed message, or null for unknown types and malformed JSON (newer servers may add types). */
        fun parse(text: String): SignalIn? = try {
            val o = JSONObject(text)
            when (o.optString("type")) {
                "welcome" -> o.getJSONObject("you").let { Welcome(it.getLong("id"), it.getString("name")) }
                "session" -> {
                    val peer = o.getJSONObject("peer")
                    val servers = o.optJSONArray("ice_servers") ?: JSONArray()
                    Session(
                        session = o.getLong("session"),
                        operatorId = peer.getLong("id"),
                        operatorName = peer.getString("name"),
                        iceServers = List(servers.length()) { IceServer.parse(servers.getJSONObject(it)) },
                    )
                }
                "offer" -> Offer(o.getLong("session"), Sdp.parse(o.getJSONObject("sdp")))
                "ice" -> Candidate(o.getLong("session"), Ice.parse(o.getJSONObject("candidate")))
                "end" -> End(o.getLong("session"), o.optString("reason"))
                "update" -> Update(AppRelease.parse(o))
                else -> null
            }
        } catch (e: JSONException) {
            null
        }
    }
}

/** ARCH-002. */
enum class RobotMode { IDLE, TELEOP, AUTO, NEEDS_HELP }

enum class NetworkType(val wire: String) { WIFI("wifi"), CELLULAR("cellular"), OTHER("other") }

/** ADM-001: the phone's network; [signal] 0..4, sampled when the network changes, not tracked. */
data class NetworkInfo(val type: NetworkType, val operator: String? = null, val signal: Int? = null)

/**
 * `status` message; sent whole, and only when it changes (SIG-004, PWR-012). Every field is coarse for that
 * reason: the battery in 5 % steps outside a session ([batteryStep]), the service start time instead of an uptime.
 */
data class RobotStatus(
    val mode: RobotMode = RobotMode.IDLE,
    val phoneBattery: Int? = null,
    val robotBatteryMv: Int? = null,
    /** PWR-023: false without root, null while probing. */
    val chargeControl: Boolean? = null,
    val app: String? = null,
    val firmware: String? = null,
    val phone: String? = null,
    val android: String? = null,
    val network: NetworkInfo? = null,
    /** ROB-008 state: `current`, `blocked`, `waiting`, `downloading`, `installing`, `failed`. */
    val appUpdate: String? = null,
    /** ROB-009 state: `current`, `no_image`, `waiting`, `sending`, `installing`, `failed`. */
    val firmwareUpdate: String? = null,
    val deviceOwner: Boolean? = null,
    /** When the robot service started, Unix seconds. */
    val started: Long? = null,
)

/** ADM-001: 1 % steps during a session (the operator watches it), 5 % otherwise (no chatter while idle). */
fun batteryStep(percent: Int?, session: Boolean): Int? = if (session || percent == null) percent else percent / 5 * 5

/** ADM-002: a position fix; [at] in Unix seconds. */
data class Fix(val lat: Double, val lon: Double, val accuracyM: Float?, val at: Long)

enum class EventLevel(val wire: String) { INFO("info"), WARNING("warning"), ERROR("error") }

/** ADM-003: something an admin should know about; queued while offline. [at] in Unix seconds. */
data class RobotEvent(val level: EventLevel, val code: String, val message: String, val at: Long) {
    fun toJson(): JSONObject =
        JSONObject().put("level", level.wire).put("code", code).put("message", message).put("at", at)

    companion object {
        fun parse(o: JSONObject) = RobotEvent(
            level = EventLevel.entries.firstOrNull { it.wire == o.optString("level") } ?: EventLevel.INFO,
            code = o.getString("code"),
            message = o.optString("message"),
            at = o.getLong("at"),
        )
    }
}

/** Robot → server. */
object SignalOut {
    fun status(s: RobotStatus): String = message("status")
        .put("mode", s.mode.name)
        .putOpt("phone_battery", s.phoneBattery)
        .putOpt("robot_battery_mv", s.robotBatteryMv)
        .putOpt("charge_control", s.chargeControl)
        .putOpt("app", s.app)
        .putOpt("firmware", s.firmware)
        .putOpt("phone", s.phone)
        .putOpt("android", s.android)
        .putOpt("network", s.network?.let { n ->
            JSONObject().put("type", n.type.wire).putOpt("operator", n.operator).putOpt("signal", n.signal)
        })
        .putOpt("app_update", s.appUpdate)
        .putOpt("firmware_update", s.firmwareUpdate)
        .putOpt("device_owner", s.deviceOwner)
        .putOpt("started", s.started)
        .toString()

    fun location(f: Fix): String = message("location")
        .put("lat", f.lat)
        .put("lon", f.lon)
        .putOpt("accuracy_m", f.accuracyM?.toDouble())
        .put("at", f.at)
        .toString()

    fun event(e: RobotEvent): String {
        val o = e.toJson()
        return message("event").also { m -> o.keys().forEach { m.put(it, o.get(it)) } }.toString()
    }

    fun answer(session: Long, sdp: Sdp): String = message("answer").put("session", session).put("sdp", sdp.toJson()).toString()

    fun ice(session: Long, ice: Ice): String = message("ice").put("session", session).put("candidate", ice.toJson()).toString()

    fun end(session: Long): String = message("end").put("session", session).toString()

    private fun message(type: String) = JSONObject().put("type", type)
}

private fun JSONObject.optStringOrNull(name: String): String? = if (isNull(name)) null else getString(name)
