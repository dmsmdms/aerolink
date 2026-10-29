package com.aerolink.robot.service

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.telephony.TelephonyManager
import com.aerolink.robot.signal.NetworkInfo
import com.aerolink.robot.signal.NetworkType
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * ADM-001: which network the phone uses, for the admin table. Event-driven (network callbacks, no polling); the
 * signal level is read when the network changes and not tracked afterwards, so a moving robot does not send a
 * status for every bar it gains or loses (PWR-012).
 */
class NetworkMonitor(context: Context) {
    private val connectivity = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val telephony = context.getSystemService(Context.TELEPHONY_SERVICE) as TelephonyManager?
    private val _info = MutableStateFlow<NetworkInfo?>(null)
    val info: StateFlow<NetworkInfo?> = _info.asStateFlow()
    private var current: Network? = null

    // Binder threads; the state flow is thread-safe.
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            // Capabilities change with every signal fluctuation: only a different default network counts.
            if (network == connectivity.activeNetwork && network != current) refresh()
        }

        override fun onLost(network: Network) = refresh()
    }

    fun start() {
        val request = NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET).build()
        connectivity.registerNetworkCallback(request, callback)
        refresh()
    }

    fun stop() {
        connectivity.unregisterNetworkCallback(callback)
    }

    @Synchronized
    private fun refresh() {
        val network = connectivity.activeNetwork
        current = network
        val caps = network?.let { connectivity.getNetworkCapabilities(it) }
        _info.value = when {
            caps == null -> null
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> NetworkInfo(NetworkType.WIFI, signal = wifiSignal(caps))
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> NetworkInfo(
                NetworkType.CELLULAR,
                operator = telephony?.networkOperatorName?.takeIf { it.isNotBlank() },
                signal = if (Build.VERSION.SDK_INT >= 28) telephony?.signalStrength?.level else null,
            )
            else -> NetworkInfo(NetworkType.OTHER)
        }
    }

    private fun wifiSignal(caps: NetworkCapabilities): Int? =
        if (Build.VERSION.SDK_INT >= 29) wifiLevel(caps.signalStrength) else null

    companion object {
        /** RSSI (dBm) → 0..4 bars, as Android's own Wi-Fi icon roughly does; null when unknown. */
        fun wifiLevel(rssi: Int): Int? = when {
            rssi == Int.MIN_VALUE || rssi >= 0 -> null
            rssi >= -55 -> 4
            rssi >= -66 -> 3
            rssi >= -77 -> 2
            rssi >= -88 -> 1
            else -> 0
        }
    }
}
