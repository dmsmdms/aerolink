package com.aerolink.robot.signal

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class SignalTest {
    @Test
    fun parsesServerMessages() {
        assertEquals(SignalIn.Welcome(7, "r1"), SignalIn.parse("""{"type":"welcome","you":{"id":7,"name":"r1"}}"""))
        assertEquals(
            SignalIn.Session(
                session = 3, operatorId = 42, operatorName = "Alice",
                iceServers = listOf(
                    IceServer(listOf("stun:s:3478"), null, null),
                    IceServer(listOf("turn:s:3478?transport=udp", "turns:s:5349"), "1700000000:robot7", "c2VjcmV0"),
                ),
            ),
            SignalIn.parse(
                """{"type":"session","session":3,"robot":7,"peer":{"id":42,"name":"Alice"},"ice_servers":[
                {"urls":["stun:s:3478"]},
                {"urls":["turn:s:3478?transport=udp","turns:s:5349"],"username":"1700000000:robot7","credential":"c2VjcmV0"}]}""",
            ),
        )
        assertEquals(
            SignalIn.Offer(3, Sdp("offer", "v=0\r\n")),
            SignalIn.parse("""{"type":"offer","session":3,"sdp":{"type":"offer","sdp":"v=0\r\n"}}"""),
        )
        assertEquals(
            SignalIn.Candidate(3, Ice("candidate:1 1 udp 1 1.2.3.4 5 typ host", "0", 0)),
            SignalIn.parse("""{"type":"ice","session":3,"candidate":{"candidate":"candidate:1 1 udp 1 1.2.3.4 5 typ host","sdpMid":"0","sdpMLineIndex":0}}"""),
        )
        assertEquals(SignalIn.End(3, "released"), SignalIn.parse("""{"type":"end","session":3,"reason":"released"}"""))
        assertEquals(
            SignalIn.Update(AppRelease(2, "0.2.0", mapOf("arm64-v8a" to AppRelease.Apk(9, "ab")))),
            SignalIn.parse("""{"type":"update","version_code":2,"version_name":"0.2.0","apks":{"arm64-v8a":{"size":9,"sha256":"ab"}}}"""),
        )
    }

    @Test
    fun ignoresUnknownAndMalformed() {
        assertNull(SignalIn.parse("""{"type":"robots","robots":[]}"""))
        assertNull(SignalIn.parse("""{"type":"offer","session":3}"""))
        assertNull(SignalIn.parse("not json"))
    }

    @Test
    fun buildsRobotMessages() {
        val status = JSONObject(SignalOut.status(RobotStatus(RobotMode.TELEOP, phoneBattery = 55, chargeControl = false, app = "0.1.0")))
        assertEquals("status", status.getString("type"))
        assertEquals("TELEOP", status.getString("mode"))
        assertEquals(55, status.getInt("phone_battery"))
        assertFalse(status.getBoolean("charge_control"))
        assertFalse(status.has("robot_battery_mv")) // unknown values are left out
        assertFalse(status.has("firmware"))

        val answer = JSONObject(SignalOut.answer(3, Sdp("answer", "v=0")))
        assertEquals(3, answer.getInt("session"))
        assertEquals("answer", answer.getJSONObject("sdp").getString("type"))

        val ice = JSONObject(SignalOut.ice(3, Ice("candidate:x", null, 0))).getJSONObject("candidate")
        assertEquals("candidate:x", ice.getString("candidate"))
        assert(ice.isNull("sdpMid"))
        val end = JSONObject(SignalOut.end(3))
        assertEquals("end", end.getString("type"))
        assertEquals(3, end.getInt("session"))
    }

    @Test
    fun buildsAdminMessages() {
        val status = JSONObject(
            SignalOut.status(
                RobotStatus(
                    phone = "SM-J710F", android = "6.0.1", deviceOwner = true, started = 1700000000,
                    network = NetworkInfo(NetworkType.CELLULAR, "kyivstar", 3), appUpdate = "blocked",
                ),
            ),
        )
        assertEquals("SM-J710F", status.getString("phone"))
        assertEquals("cellular", status.getJSONObject("network").getString("type"))
        assertEquals(3, status.getJSONObject("network").getInt("signal"))
        assertEquals("blocked", status.getString("app_update"))
        assertEquals(1700000000L, status.getLong("started"))
        assertFalse(JSONObject(SignalOut.status(RobotStatus(network = NetworkInfo(NetworkType.WIFI)))).getJSONObject("network").has("signal"))

        val location = JSONObject(SignalOut.location(Fix(50.45, 30.52, 8.5f, 99)))
        assertEquals("location", location.getString("type"))
        assertEquals(50.45, location.getDouble("lat"), 0.0)
        assertEquals(8.5, location.getDouble("accuracy_m"), 0.0)
        assertFalse(JSONObject(SignalOut.location(Fix(1.0, 2.0, null, 1))).has("accuracy_m"))

        val event = RobotEvent(EventLevel.ERROR, "crash", "boom", 5)
        val json = JSONObject(SignalOut.event(event))
        assertEquals("event", json.getString("type"))
        assertEquals("error", json.getString("level"))
        assertEquals(event, RobotEvent.parse(json))
    }

    @Test
    fun batteryStepsOutsideASession() {
        assertEquals(55, batteryStep(57, session = false))
        assertEquals(57, batteryStep(57, session = true))
        assertEquals(100, batteryStep(100, session = false))
        assertNull(batteryStep(null, session = false))
    }

    @Test
    fun socketUrlFromServerOrigin() {
        assertEquals("wss://robots.example.org/ws/robot", SignalingClient.robotSocketUrl("https://robots.example.org/"))
        assertEquals("ws://192.168.1.5:8080/ws/robot", SignalingClient.robotSocketUrl(" http://192.168.1.5:8080 "))
        assertNull(SignalingClient.robotSocketUrl("robots.example.org"))
        assertNull(SignalingClient.robotSocketUrl("https://"))
        assertNull(SignalingClient.robotSocketUrl(""))
    }
}
