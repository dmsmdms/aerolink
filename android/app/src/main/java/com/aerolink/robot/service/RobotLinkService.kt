package com.aerolink.robot.service

import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import com.aerolink.robot.BuildConfig
import com.aerolink.robot.MainActivity
import com.aerolink.robot.R
import com.aerolink.robot.link.AccessoryTransport
import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.LinkTransport
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.link.SimulatedBase
import com.aerolink.robot.power.ChargeControl
import com.aerolink.robot.power.ChargeController
import com.aerolink.robot.rtc.VideoSettings
import com.aerolink.robot.rtc.WebRtcPeerFactory
import com.aerolink.robot.signal.RobotMode
import com.aerolink.robot.signal.SignalIn
import com.aerolink.robot.signal.RobotStatus
import com.aerolink.robot.signal.SignalOut
import com.aerolink.robot.signal.SignalPhase
import com.aerolink.robot.signal.SignalingClient
import com.aerolink.robot.teleop.Teleop
import com.aerolink.robot.update.FirmwareUpdater
import com.aerolink.robot.update.Updater
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * The robot's foreground service (docs/02-architecture.md, PWR-010):
 * - the USB link to the RP2040 while the accessory is attached (or a [SimulatedBase] on the bench), and its
 *   firmware updates ([FirmwareUpdater], ROB-009);
 * - the signaling WebSocket to the server whenever the robot is provisioned ([RobotSettings]);
 * - operator sessions ([Teleop]): camera, microphone and a partial wake lock exist only during one (PWR-012/013);
 * - the phone charge window;
 * - app updates announced by the server ([Updater], ROB-008), installed only between sessions.
 * Started by [MainActivity], on boot and after an update ([BootReceiver]) and on accessory attach.
 */
class RobotLinkService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var usb: UsbManager
    private lateinit var charge: ChargeController
    private lateinit var teleop: Teleop
    private lateinit var updater: Updater
    private lateinit var firmwareUpdater: FirmwareUpdater
    private lateinit var wakeLock: PowerManager.WakeLock
    private var config = RobotConfig()

    private val link = MutableStateFlow<RobotLink?>(null)
    private var simulated = false
    private var signaling: SignalingClient? = null
    private var signalingJob: Job? = null

    private val detachReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            Log.i(TAG, "accessory detached")
            if (!simulated) scope.launch { closeLink() }
        }
    }

    /** A network came up (Wi-Fi ↔ cellular handover): reconnect now instead of waiting out the backoff. */
    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            signaling?.reconnectNow()
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        usb = getSystemService(USB_SERVICE) as UsbManager
        wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "AeroLink:session")
            .apply { setReferenceCounted(false) }
        DeviceOwner.apply(this) // PWR-014: permissions before the first session needs them
        startInForeground(session = false)
        ContextCompat.registerReceiver(
            this, detachReceiver, IntentFilter(UsbManager.ACTION_USB_ACCESSORY_DETACHED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        (getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager).registerNetworkCallback(
            NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET).build(),
            networkCallback,
        )
        charge = ChargeController(this, scope).also { it.start() }
        scope.launch { charge.status.collect { RobotHub.charge.value = it } }
        scope.launch { link.collect { RobotHub.link.value = it } }

        teleop = Teleop(
            scope = scope,
            links = link,
            peers = WebRtcPeerFactory(this, scope) { VideoSettings(rotation = config.videoRotation) },
            signal = { signaling?.send(it) ?: false },
            phoneBattery = { charge.status.value.levelPercent },
        )
        scope.launch { teleop.status.collect { RobotHub.teleop.value = it } }
        scope.launch {
            teleop.status.map { it.active }.distinctUntilChanged().collect(::sessionResources)
        }
        updater = Updater(this, scope, idle = teleop.status.map { !it.active }, config = { config })
        scope.launch { updater.status.collect { RobotHub.update.value = it } }
        firmwareUpdater = FirmwareUpdater(this, scope, link, idle = teleop.status.map { !it.active }).also { it.start() }
        scope.launch { firmwareUpdater.status.collect { RobotHub.firmware.value = it } }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val newConfig = RobotSettings.load(this)
        val reconnect = newConfig.server != config.server || newConfig.token != config.token || signalingJob == null
        config = newConfig // the video rotation applies from the next session
        if (reconnect) restartSignaling()

        val accessory = intent?.let { IntentCompat.getParcelableExtra(it, UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java) }
            ?: usb.accessoryList?.firstOrNull(::isRobotBase)
        when {
            link.value != null && !simulated -> Unit
            accessory == null -> Log.i(TAG, "no robot base attached")
            intent?.action == ACTION_PERMISSION && !intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false) ->
                Log.w(TAG, "USB permission denied")
            usb.hasPermission(accessory) -> openAccessory(accessory)
            else -> requestPermission(accessory)
        }
        updateSimulatedBase()
        if (accessory == null && link.value == null && !config.online) stopSelf()
        // Provisioned robots stay online: the system restarts the service if it has to kill it.
        return if (config.online) START_STICKY else START_NOT_STICKY
    }

    override fun onDestroy() {
        unregisterReceiver(detachReceiver)
        (getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager).unregisterNetworkCallback(networkCallback)
        teleop.close() // the peer is released outside this scope
        updater.close()
        signaling?.stop()
        // Runs before the scope dies: DISARM the robot and give charging back (PWR-023).
        runBlocking { link.value?.close() }
        charge.release()
        if (wakeLock.isHeld) wakeLock.release()
        RobotHub.link.value = null
        RobotHub.charge.value = null
        RobotHub.signal.value = null
        RobotHub.teleop.value = null
        RobotHub.update.value = null
        scope.cancel()
        super.onDestroy()
    }

    private fun restartSignaling() {
        teleop.close() // another server or token: the session cannot carry over
        updater.close()
        signalingJob?.cancel()
        signaling?.stop()
        signaling = null
        signalingJob = null
        RobotHub.signal.value = null
        val url = config.socketUrl
        if (url == null || !config.online) {
            Log.i(TAG, "not provisioned: no server connection")
            return
        }
        val onMessage = { msg: SignalIn ->
            if (msg is SignalIn.Update) updater.offer(msg.release) else teleop.onSignal(msg)
        }
        val client = SignalingClient(url, config.token, scope, onMessage, teleop::onSignalLost, { teleop.currentSession })
        signaling = client
        client.start()
        signalingJob = scope.launch {
            launch { client.state.collect { RobotHub.signal.value = it } }
            // SIG-004: the status goes out on change only, and again after every (re)connect.
            val online = client.state.map { it.phase == SignalPhase.ONLINE }.distinctUntilChanged()
            combine(robotStatus(), online) { status, up -> status.takeIf { up } }
                .collect { status -> status?.let { client.send(SignalOut.status(it)) } }
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private fun robotStatus() = combine(
        link.flatMapLatest { it?.status ?: flowOf(null) }
            // 0.1 V steps: the raw value changes with every TELEMETRY.
            .map { s -> s?.firmware?.firmwareVersion to s?.telemetry?.batteryMv?.let { it / 100 * 100 } }
            .distinctUntilChanged(),
        charge.status,
        teleop.status.map { it.active }.distinctUntilChanged(),
    ) { (firmware, batteryMv), charge, active ->
        RobotStatus(
            mode = if (active) RobotMode.TELEOP else RobotMode.IDLE,
            phoneBattery = charge.levelPercent,
            robotBatteryMv = batteryMv,
            chargeControl = when (charge.control) {
                ChargeControl.PROBING -> null
                ChargeControl.ACTIVE -> true
                ChargeControl.UNAVAILABLE -> false
            },
            app = BuildConfig.VERSION_NAME,
            firmware = firmware,
        )
    }.distinctUntilChanged()

    /** PWR-013: wake lock and camera/microphone service types only while an operator session lasts. */
    private fun sessionResources(active: Boolean) {
        if (active) wakeLock.acquire(MAX_SESSION_MS) else if (wakeLock.isHeld) wakeLock.release()
        startInForeground(session = active)
    }

    private fun openAccessory(accessory: UsbAccessory) {
        val fd = usb.openAccessory(accessory)
        if (fd == null) {
            Log.w(TAG, "openAccessory failed")
            return
        }
        Log.i(TAG, "accessory opened: ${accessory.model} ${accessory.version} ${accessory.serial}")
        scope.launch {
            if (simulated) closeLink(fallBackToSimulated = false)
            openLink(AccessoryTransport(fd), simulated = false)
        }
    }

    /** Bench: the simulated base stands in while no real one is attached. */
    private fun updateSimulatedBase() {
        when {
            config.simulate && link.value == null -> openLink(SimulatedBase(), simulated = true)
            !config.simulate && simulated -> scope.launch { closeLink() }
        }
    }

    private fun openLink(transport: LinkTransport, simulated: Boolean) {
        val newLink = RobotLink(transport, scope)
        this.simulated = simulated
        link.value = newLink
        newLink.start()
        scope.launch {
            newLink.status.first { it.phase == LinkPhase.CLOSED }
            Log.i(TAG, "link closed")
            if (link.value === newLink) closeLink()
        }
    }

    private suspend fun closeLink(fallBackToSimulated: Boolean = true) {
        link.value?.close()
        link.value = null
        simulated = false
        if (fallBackToSimulated) updateSimulatedBase()
        // Energy: an unprovisioned phone has nothing to do without the robot base; the next attach starts us again.
        if (link.value == null && !config.online) stopSelf()
    }

    private fun requestPermission(accessory: UsbAccessory) {
        val intent = Intent(this, RobotLinkService::class.java).setAction(ACTION_PERMISSION)
        // Mutable: the system adds EXTRA_ACCESSORY / EXTRA_PERMISSION_GRANTED.
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
        usb.requestPermission(accessory, PendingIntent.getService(this, 0, intent, flags))
    }

    private fun startInForeground(session: Boolean) {
        val channel = NotificationChannelCompat.Builder(CHANNEL_ID, NotificationManagerCompat.IMPORTANCE_LOW)
            .setName(getString(R.string.notification_channel))
            .build()
        NotificationManagerCompat.from(this).createNotificationChannel(channel)
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(getString(if (session) R.string.notification_session else R.string.notification_title))
            .setContentIntent(open)
            .setOngoing(true)
            .build()
        var type = 0
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) type = ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
        if (session && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        }
        try {
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
        } catch (e: RuntimeException) {
            // Android 14 refuses camera/microphone types without the runtime permission (SecurityException) or
            // when started from the background (IllegalStateException); the session goes on without them
            // (WebRtcPeer checks the permissions itself).
            if (!session) throw e
            Log.w(TAG, "foreground service type refused", e)
            startInForeground(session = false)
        }
    }

    companion object {
        private const val TAG = "AeroLinkService"
        private const val CHANNEL_ID = "link"
        private const val NOTIFICATION_ID = 1
        private const val ACTION_PERMISSION = "com.aerolink.robot.USB_PERMISSION"
        /** A wake lock is never left behind, even if a session's end is missed. */
        private const val MAX_SESSION_MS = 4 * 60 * 60 * 1000L

        // Must match rp2040/src/usb_aoa.c and res/xml/accessory_filter.xml (LINK-002/003).
        private const val MANUFACTURER = "AeroLink"
        private const val MODEL = "RobotBase"

        fun isRobotBase(accessory: UsbAccessory) =
            accessory.manufacturer == MANUFACTURER && accessory.model == MODEL

        /** Starts the service, or makes a running one re-read [RobotSettings] and look for the accessory. */
        fun start(context: Context, accessory: UsbAccessory? = null) {
            val intent = Intent(context, RobotLinkService::class.java)
            if (accessory != null) intent.putExtra(UsbManager.EXTRA_ACCESSORY, accessory)
            ContextCompat.startForegroundService(context, intent)
        }
    }
}
