package com.aerolink.robot.power

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.IOException

enum class ChargeControl { PROBING, ACTIVE, UNAVAILABLE }

data class ChargeStatus(
    val control: ChargeControl = ChargeControl.PROBING,
    val switchPath: String? = null,
    val levelPercent: Int? = null,
    val tempDeciC: Int? = null,
    val pluggedIn: Boolean = false,
    /** What the policy wants; only enforced when [control] is ACTIVE. */
    val chargingAllowed: Boolean = true,
)

/**
 * Keeps the phone battery in the 45–60 % window (docs/04-power.md PWR-020..024) by toggling a vendor
 * sysfs charge switch as root. Without root or a known switch everything stays untouched (PWR-023).
 * Battery data comes from the sticky ACTION_BATTERY_CHANGED broadcast, no polling (PWR-024).
 */
class ChargeController(private val context: Context, private val scope: CoroutineScope) {
    private val _status = MutableStateFlow(ChargeStatus())
    val status: StateFlow<ChargeStatus> = _status.asStateFlow()

    private var switch: ChargeSwitch? = null
    private var applied: Boolean? = null

    fun start() {
        scope.launch {
            switch = withContext(Dispatchers.IO) { ChargeSwitch.probe() }
            _status.update {
                it.copy(
                    control = if (switch != null) ChargeControl.ACTIVE else ChargeControl.UNAVAILABLE,
                    switchPath = switch?.path,
                )
            }
            Log.i(TAG, "charge control: ${switch?.path ?: "unavailable"}")
            batteryUpdates().collect(::onBattery)
        }
    }

    /**
     * Re-enables charging; called when the service stops so the phone never stays without charging.
     * Blocks for one su call (~100 ms) on the caller's thread.
     */
    fun release() {
        val sw = switch ?: return
        if (applied == false) sw.set(true)
        applied = null
    }

    private suspend fun onBattery(b: Battery) {
        val previous = _status.value
        val allowed = if (previous.levelPercent == null) {
            ChargePolicy.initial(b.levelPercent, b.tempDeciC)
        } else {
            ChargePolicy.next(previous.chargingAllowed, b.levelPercent, b.tempDeciC)
        }
        _status.update {
            it.copy(levelPercent = b.levelPercent, tempDeciC = b.tempDeciC, pluggedIn = b.pluggedIn, chargingAllowed = allowed)
        }
        val sw = switch ?: return
        if (applied != allowed) {
            val ok = withContext(Dispatchers.IO) { sw.set(allowed) }
            if (ok) applied = allowed
            Log.i(TAG, "charging ${if (allowed) "enabled" else "disabled"} at ${b.levelPercent} %, " +
                "${b.tempDeciC / 10.0} °C: ${if (ok) "ok" else "failed"}")
        }
    }

    private data class Battery(val levelPercent: Int, val tempDeciC: Int, val pluggedIn: Boolean)

    private fun batteryUpdates(): Flow<Battery> = callbackFlow {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
                val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
                if (level < 0 || scale <= 0) return
                trySend(
                    Battery(
                        levelPercent = level * 100 / scale,
                        tempDeciC = intent.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, 0),
                        pluggedIn = intent.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0,
                    ),
                )
            }
        }
        ContextCompat.registerReceiver(
            context, receiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED), ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        awaitClose { context.unregisterReceiver(receiver) }
    }

    private companion object {
        const val TAG = "AeroLinkCharge"
    }
}

/** A writable sysfs node that suspends charging while the USB link stays up (PWR-022). */
private class ChargeSwitch(val path: String, private val enableValue: String, private val disableValue: String) {
    fun set(enabled: Boolean): Boolean =
        RootShell.run("echo ${if (enabled) enableValue else disableValue} > $path")?.exitCode == 0

    companion object {
        // Priority order, first writable node wins.
        private val candidates = listOf(
            ChargeSwitch("/sys/class/power_supply/battery/batt_slate_mode", enableValue = "0", disableValue = "1"),
            ChargeSwitch("/sys/class/power_supply/battery/charging_enabled", enableValue = "1", disableValue = "0"),
            ChargeSwitch("/sys/class/power_supply/battery/input_suspend", enableValue = "0", disableValue = "1"),
        )

        /** One su call (one Magisk prompt): prints our uid, then every writable candidate. */
        fun probe(): ChargeSwitch? {
            val paths = candidates.joinToString(" ") { it.path }
            val result = RootShell.run("id -u; for f in $paths; do [ -w \"\$f\" ] && echo \"\$f\"; done")
                ?: return null
            val lines = result.output.lines().map { it.trim() }
            if (lines.firstOrNull() != "0") return null
            return candidates.firstOrNull { it.path in lines }
        }
    }
}

private object RootShell {
    class Result(val exitCode: Int, val output: String)

    /** Runs [command] via `su -c`; null when su is missing or denied. Blocking: call on Dispatchers.IO. */
    fun run(command: String): Result? =
        try {
            val process = ProcessBuilder("su", "-c", command).redirectErrorStream(true).start()
            val output = process.inputStream.bufferedReader().readText()
            Result(process.waitFor(), output)
        } catch (e: IOException) {
            null
        } catch (e: InterruptedException) {
            null
        }
}
