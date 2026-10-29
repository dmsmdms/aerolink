package com.aerolink.robot.service

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Bundle
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat
import com.aerolink.robot.signal.Fix

/**
 * ADM-002: the robot's position for the admins' map. During an operator session from the GPS, sent when moved
 * ≥ 10 m; outside a session only the passive provider (fixes other apps already paid for) and the last fix of the
 * session when it ends. The GPS is never switched on just for the map (`docs/04-power.md`).
 *
 * Needs the location permission (granted by the Device Owner, PWR-014); without it nothing happens. A fix that
 * could not be sent (offline) is sent on the next [flush]. Main thread only.
 */
class LocationReporter(context: Context, private val send: (Fix) -> Boolean) {
    private val app = context.applicationContext
    private val manager = app.getSystemService(Context.LOCATION_SERVICE) as LocationManager?
    private var sent: Location? = null
    private var pending: Fix? = null
    private var session: Boolean? = null

    private val listener = object : LocationListener {
        override fun onLocationChanged(location: Location) = report(location)

        // Required before API 29.
        @Deprecated("Deprecated in Java")
        override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) = Unit
        override fun onProviderEnabled(provider: String) = Unit
        override fun onProviderDisabled(provider: String) = Unit
    }

    val permitted: Boolean
        get() = ContextCompat.checkSelfPermission(app, Manifest.permission.ACCESS_FINE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED

    /** Switches between the GPS (session) and the passive provider. */
    @SuppressLint("MissingPermission") // checked by [permitted]
    fun session(active: Boolean) {
        val manager = manager ?: return
        if (session == active || !permitted) return
        val wasSession = session == true
        session = active
        manager.removeUpdates(listener)
        if (wasSession) manager.getLastKnownLocation(LocationManager.GPS_PROVIDER)?.let(::report)
        val provider = if (active) LocationManager.GPS_PROVIDER else LocationManager.PASSIVE_PROVIDER
        try {
            manager.requestLocationUpdates(
                provider, if (active) SESSION_INTERVAL_MS else IDLE_INTERVAL_MS, MIN_DISTANCE_M, listener, Looper.getMainLooper(),
            )
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "no $provider location provider")
        }
    }

    fun flush() {
        val fix = pending ?: return
        if (send(fix)) pending = null
    }

    fun stop() {
        manager?.removeUpdates(listener)
        session = null
    }

    private fun report(location: Location) {
        val last = sent
        if (last != null && last.distanceTo(location) < MIN_DISTANCE_M) return
        sent = location
        pending = Fix(location.latitude, location.longitude, location.accuracy.takeIf { location.hasAccuracy() }, location.time / 1000)
        flush()
    }

    private companion object {
        const val TAG = "AeroLinkService"
        const val MIN_DISTANCE_M = 10f
        const val SESSION_INTERVAL_MS = 2_000L
        const val IDLE_INTERVAL_MS = 60_000L
    }
}
