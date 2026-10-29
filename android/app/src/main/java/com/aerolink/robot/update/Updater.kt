package com.aerolink.robot.update

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.content.pm.PackageInstaller
import android.os.Build
import android.util.Log
import com.aerolink.robot.BuildConfig
import com.aerolink.robot.service.DeviceOwner
import com.aerolink.robot.service.RobotConfig
import com.aerolink.robot.signal.AppRelease
import com.aerolink.robot.signal.SignalingClient
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.transformLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.IOException
import java.security.MessageDigest

/** What the diagnostics screen shows about app updates (ROB-008). */
sealed class UpdateStatus {
    /** No release announced, or this is it. */
    object Current : UpdateStatus()

    /** A newer release exists but this phone cannot install it without a tap on the screen (not Device Owner). */
    data class Blocked(val version: String) : UpdateStatus()

    /** Waiting for the operator session to end. */
    data class Waiting(val version: String) : UpdateStatus()

    data class Downloading(val version: String, val percent: Int) : UpdateStatus()

    /** Handed to the system installer; the app is restarted by it on success. */
    data class Installing(val version: String) : UpdateStatus()

    data class Failed(val version: String, val error: String) : UpdateStatus()
}

/** The decision for an announced release (pure, JVM-tested). */
object UpdatePolicy {
    /** Per release: a broken download or install is not retried forever over a metered link. */
    const val MAX_ATTEMPTS = 3

    sealed class Decision {
        object Current : Decision()
        object NoApk : Decision()
        object GaveUp : Decision()
        data class Fetch(val abi: String, val apk: AppRelease.Apk) : Decision()
    }

    /** [abis]: the phone's ABIs, preferred first; [attempts]: attempts already made for this release. */
    fun decide(release: AppRelease, installedCode: Int, abis: List<String>, attempts: Int): Decision {
        if (release.versionCode <= installedCode) return Decision.Current
        val abi = abis.firstOrNull { it in release.apks } ?: return Decision.NoApk
        if (attempts >= MAX_ATTEMPTS) return Decision.GaveUp
        return Decision.Fetch(abi, release.apks.getValue(abi))
    }
}

/**
 * ROB-008: installs a newer app release announced by the server, only while no operator session runs ([idle]).
 * The APK is streamed into a [PackageInstaller] session while its SHA-256 is checked, so nothing is stored
 * twice; Android itself refuses an APK signed with another key or with a lower versionCode.
 *
 * Silent installs need Device Owner (PWR-014) — or, on Android 12+, a self-update with "install unknown apps"
 * allowed (bench phones). Otherwise the release is only reported as [UpdateStatus.Blocked]: a robot has nobody to
 * confirm an install dialog. On success the system kills the app and restarts the service
 * (`MY_PACKAGE_REPLACED`, [com.aerolink.robot.service.BootReceiver]).
 */
class Updater(
    private val context: Context,
    private val scope: CoroutineScope,
    private val idle: Flow<Boolean>,
    private val config: () -> RobotConfig,
    private val http: OkHttpClient = SignalingClient.defaultHttp,
) {
    private val _status = MutableStateFlow<UpdateStatus>(UpdateStatus.Current)
    val status: StateFlow<UpdateStatus> = _status.asStateFlow()

    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val installer = context.packageManager.packageInstaller
    private var job: Job? = null
    private var offered: AppRelease? = null

    init {
        // Leftovers of a download the process did not survive.
        for (session in installer.mySessions) installer.abandonSession(session.sessionId)
    }

    /** The server announced [release] (after every reconnect, so the same one comes again and again). */
    fun offer(release: AppRelease) {
        if (release == offered && job?.isActive == true) return
        offered = release
        job?.cancel()
        job = scope.launch { handle(release) }
    }

    fun close() {
        job?.cancel()
        offered = null
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private suspend fun handle(release: AppRelease) {
        val version = release.versionName
        val attempts = if (prefs.getInt(KEY_CODE, 0) == release.versionCode) prefs.getInt(KEY_ATTEMPTS, 0) else 0
        val decision = UpdatePolicy.decide(release, BuildConfig.VERSION_CODE, Build.SUPPORTED_ABIS.toList(), attempts)
        _status.value = when (decision) {
            UpdatePolicy.Decision.Current -> UpdateStatus.Current
            UpdatePolicy.Decision.NoApk -> UpdateStatus.Failed(version, "no APK for ${Build.SUPPORTED_ABIS.joinToString()}")
            UpdatePolicy.Decision.GaveUp -> UpdateStatus.Failed(version, prefs.getString(KEY_ERROR, null) ?: "gave up")
            is UpdatePolicy.Decision.Fetch -> if (canInstallSilently()) UpdateStatus.Waiting(version) else UpdateStatus.Blocked(version)
        }
        if (decision !is UpdatePolicy.Decision.Fetch || _status.value is UpdateStatus.Blocked) {
            if (decision !is UpdatePolicy.Decision.Current) Log.i(TAG, "release $version (${release.versionCode}): ${_status.value}")
            return
        }
        // A session that starts during the download cancels it; the attempt is made again once idle.
        idle.distinctUntilChanged().transformLatest { free ->
            if (free) emit(attempt(release, decision, attempts + 1)) else _status.value = UpdateStatus.Waiting(version)
        }.first()
    }

    private suspend fun attempt(release: AppRelease, fetch: UpdatePolicy.Decision.Fetch, attempt: Int) {
        val version = release.versionName
        prefs.edit().putInt(KEY_CODE, release.versionCode).putInt(KEY_ATTEMPTS, attempt).apply()
        Log.i(TAG, "updating to $version (${release.versionCode}, ${fetch.abi}), attempt $attempt")
        _status.value = UpdateStatus.Downloading(version, 0)
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(fetch.apk.size)
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val id = installer.createSession(params)
        try {
            withContext(Dispatchers.IO) { installer.openSession(id).use { download(it, fetch, version) } }
        } catch (e: Exception) {
            installer.abandonSession(id)
            if (e is CancellationException) throw e
            return fail(version, e.message ?: e.toString())
        }
        _status.value = UpdateStatus.Installing(version)
        results.value = null
        installer.openSession(id).use { it.commit(InstallReceiver.sender(context)) }
        val result = withTimeoutOrNull(INSTALL_TIMEOUT_MS) { results.filterNotNull().first() }
        // Success usually never gets here: the system replaces the app and kills this process first.
        if (result != null && result.ok) Log.i(TAG, "installed $version") else fail(version, result?.error ?: "no answer from the installer")
    }

    private suspend fun download(session: PackageInstaller.Session, fetch: UpdatePolicy.Decision.Fetch, version: String) {
        val cfg = config()
        val request = Request.Builder()
            .url(cfg.server.trim().trimEnd('/') + "/api/v1/robot/apk/" + fetch.abi)
            .header("Authorization", "Bearer ${cfg.token}")
            .build()
        http.newCall(request).execute().use { response ->
            if (!response.isSuccessful) throw IOException("HTTP ${response.code}")
            val digest = MessageDigest.getInstance("SHA-256")
            var total = 0L
            var percent = 0
            session.openWrite("aerolink.apk", 0, fetch.apk.size).use { out ->
                response.body!!.byteStream().use { input ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        currentCoroutineContext().ensureActive()
                        val n = input.read(buffer)
                        if (n < 0) break
                        total += n
                        if (total > fetch.apk.size) throw IOException("more than ${fetch.apk.size} bytes")
                        digest.update(buffer, 0, n)
                        out.write(buffer, 0, n)
                        val now = (total * 100 / fetch.apk.size).toInt()
                        if (now >= percent + 10) {
                            percent = now
                            _status.value = UpdateStatus.Downloading(version, percent)
                        }
                    }
                }
                session.fsync(out)
            }
            if (total != fetch.apk.size) throw IOException("got $total of ${fetch.apk.size} bytes")
            val sha = digest.digest().joinToString("") { "%02x".format(it) }
            if (sha != fetch.apk.sha256) throw IOException("SHA-256 mismatch")
        }
    }

    private fun fail(version: String, error: String) {
        Log.w(TAG, "update to $version failed: $error")
        prefs.edit().putString(KEY_ERROR, error).apply()
        _status.value = UpdateStatus.Failed(version, error)
    }

    private fun canInstallSilently(): Boolean =
        DeviceOwner.isOwner(context) || (Build.VERSION.SDK_INT >= 31 && context.packageManager.canRequestPackageInstalls())

    internal data class Result(val ok: Boolean, val error: String?)

    companion object {
        private const val TAG = "AeroLinkUpdate"
        private const val PREFS = "update"
        private const val KEY_CODE = "code"
        private const val KEY_ATTEMPTS = "attempts"
        private const val KEY_ERROR = "error"
        private const val INSTALL_TIMEOUT_MS = 5 * 60_000L

        /** Set by [InstallReceiver] (main thread). */
        internal val results = MutableStateFlow<Result?>(null)
    }
}

/** The system installer's answer to a commit. */
class InstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        Updater.results.value = when (status) {
            PackageInstaller.STATUS_SUCCESS -> Updater.Result(true, null)
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                // Nobody is there to confirm: not Device Owner after all (e.g. ownership was just removed).
                val id = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
                if (id >= 0) context.packageManager.packageInstaller.abandonSession(id)
                Updater.Result(false, "the installer asks for confirmation on the screen")
            }
            else -> Updater.Result(false, "installer status $status: $message")
        }
    }

    companion object {
        fun sender(context: Context): IntentSender {
            // Mutable: the installer adds its status extras.
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
            return PendingIntent.getBroadcast(context, 0, Intent(context, InstallReceiver::class.java), flags).intentSender
        }
    }
}
