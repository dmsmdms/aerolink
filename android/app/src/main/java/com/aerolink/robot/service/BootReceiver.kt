package com.aerolink.robot.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * A provisioned robot goes online after a reboot without anyone touching the phone (PWR-010), and again after
 * an app update replaced the running app (ROB-008).
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED && intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        if (RobotSettings.load(context).online) RobotLinkService.start(context)
    }
}
