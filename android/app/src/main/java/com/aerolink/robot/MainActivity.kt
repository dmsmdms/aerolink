package com.aerolink.robot

import android.Manifest
import android.content.Intent
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.MaterialTheme
import androidx.core.content.IntentCompat
import com.aerolink.robot.service.RobotConfig
import com.aerolink.robot.service.RobotLinkService
import com.aerolink.robot.service.RobotSettings
import com.aerolink.robot.ui.DiagnosticsScreen

// Entry point for accessory attach (res/xml/accessory_filter.xml) and the bench diagnostics screen.
// The robot normally runs with the screen off; all work lives in RobotLinkService (PWR-010).
class MainActivity : ComponentActivity() {
    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Camera and microphone are used only during operator sessions; a Device Owner gets them silently (PWR-014).
        val wanted = mutableListOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) wanted += Manifest.permission.POST_NOTIFICATIONS
        permissions.launch(wanted.toTypedArray())
        startLinkService(intent)
        setContent {
            MaterialTheme {
                DiagnosticsScreen(onSaveConfig = ::saveConfig)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        startLinkService(intent)
    }

    private fun startLinkService(intent: Intent?) {
        val attached = intent
            ?.takeIf { it.action == UsbManager.ACTION_USB_ACCESSORY_ATTACHED }
            ?.let { IntentCompat.getParcelableExtra(it, UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java) }
        val accessory = attached
            ?: (getSystemService(USB_SERVICE) as UsbManager).accessoryList?.firstOrNull(RobotLinkService::isRobotBase)
        val robotBase = accessory?.takeIf(RobotLinkService::isRobotBase)
        val config = RobotSettings.load(this)
        if (robotBase != null || config.online || config.simulate) RobotLinkService.start(this, robotBase)
    }

    private fun saveConfig(config: RobotConfig) {
        RobotSettings.save(this, config)
        RobotLinkService.start(this) // re-reads the settings; stops itself if there is nothing to do
    }
}
