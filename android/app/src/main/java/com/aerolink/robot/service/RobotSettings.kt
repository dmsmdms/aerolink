package com.aerolink.robot.service

import android.content.Context
import com.aerolink.robot.BuildConfig
import com.aerolink.robot.signal.SignalingClient

/**
 * Robot provisioning: server origin and robot token (`aerolink robot add` prints it). Kept in the app's private
 * preferences, entered on the diagnostics screen.
 */
data class RobotConfig(
    /** Console origin, e.g. `https://robots.example.org`. */
    val server: String = "",
    /** `<id>.<secret>`. */
    val token: String = "",
    /** Bench only: a [com.aerolink.robot.link.SimulatedBase] when no RP2040 is attached (debug builds). */
    val simulateBase: Boolean = false,
    /** Clockwise rotation of the video, degrees (0/90/180/270): 0 = phone upright, else as it is mounted. */
    val videoRotation: Int = 0,
) {
    val socketUrl: String? get() = SignalingClient.robotSocketUrl(server)
    val online: Boolean get() = socketUrl != null && token.isNotBlank()
    val simulate: Boolean get() = simulateBase && BuildConfig.DEBUG
}

object RobotSettings {
    private const val FILE = "robot"

    fun load(context: Context): RobotConfig {
        val p = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        return RobotConfig(
            server = p.getString("server", null).orEmpty(),
            token = p.getString("token", null).orEmpty(),
            simulateBase = p.getBoolean("simulate_base", false),
            videoRotation = p.getInt("video_rotation", 0),
        )
    }

    fun save(context: Context, config: RobotConfig) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit()
            .putString("server", config.server.trim())
            .putString("token", config.token.trim())
            .putBoolean("simulate_base", config.simulateBase)
            .putInt("video_rotation", config.videoRotation)
            .apply()
    }
}
