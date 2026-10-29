package com.aerolink.robot.service

import android.Manifest
import android.app.admin.DeviceAdminReceiver
import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context
import android.os.Build
import android.util.Log

/** Device admin component for `adb shell dpm set-device-owner com.aerolink.robot/.service.AdminReceiver`. */
class AdminReceiver : DeviceAdminReceiver()

/**
 * PWR-014: on the robot phone the app is Device Owner (set once over adb on a phone without accounts,
 * docs/09-deployment.md "Robot phone"). That gives silent app updates (ROB-008), runtime permissions granted
 * without anyone tapping the screen, and no lock screen. Without it (bench phones) everything works except
 * these; nothing here is required.
 */
object DeviceOwner {
    private const val TAG = "AeroLinkService"

    fun isOwner(context: Context): Boolean = policy(context).isDeviceOwnerApp(context.packageName)

    /** Grants what a session needs and drops the lock screen; a no-op unless Device Owner. */
    fun apply(context: Context) {
        if (!isOwner(context)) return
        val dpm = policy(context)
        val admin = ComponentName(context, AdminReceiver::class.java)
        val permissions = buildList {
            add(Manifest.permission.CAMERA)
            add(Manifest.permission.RECORD_AUDIO)
            // ADM-002: the position for the admins' map; in the background (passive fixes) on Android 10+ too.
            add(Manifest.permission.ACCESS_FINE_LOCATION)
            add(Manifest.permission.ACCESS_COARSE_LOCATION)
            if (Build.VERSION.SDK_INT >= 29) add(Manifest.permission.ACCESS_BACKGROUND_LOCATION)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }
        for (permission in permissions) {
            val granted = dpm.setPermissionGrantState(
                admin, context.packageName, permission, DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
            )
            if (!granted) Log.w(TAG, "device owner: $permission not granted")
        }
        // Refused while a PIN/pattern is set; the robot phone has none.
        if (!dpm.setKeyguardDisabled(admin, true)) Log.w(TAG, "device owner: lock screen stays (secure lock set)")
        Log.i(TAG, "device owner: permissions granted")
    }

    /** Gives the phone back (decommissioning, bench): afterwards it is an ordinary app again. */
    @Suppress("DEPRECATION") // the replacement needs API 28 + a policy; this works on every version for the owner
    fun release(context: Context) {
        val dpm = policy(context)
        if (!dpm.isDeviceOwnerApp(context.packageName)) return
        dpm.setKeyguardDisabled(ComponentName(context, AdminReceiver::class.java), false)
        dpm.clearDeviceOwnerApp(context.packageName)
        Log.i(TAG, "device owner released")
    }

    private fun policy(context: Context) =
        context.getSystemService(Context.DEVICE_POLICY_SERVICE) as DevicePolicyManager
}
