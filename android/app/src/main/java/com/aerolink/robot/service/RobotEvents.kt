package com.aerolink.robot.service

import android.annotation.SuppressLint
import android.content.Context
import android.os.Process
import android.util.Log
import com.aerolink.robot.signal.EventLevel
import com.aerolink.robot.signal.RobotEvent
import com.aerolink.robot.signal.SignalOut
import org.json.JSONArray
import org.json.JSONException
import kotlin.system.exitProcess

/** Where [EventQueue] keeps its events across restarts. */
interface EventStore {
    fun load(): String?
    fun save(json: String)
}

/**
 * ADM-003: events for the admins, queued while the server is out of reach (bounded, the oldest dropped) and kept
 * across app restarts, sent in order once online. Main thread only; pure apart from [store] (JVM-tested).
 */
class EventQueue(private val store: EventStore, private val max: Int = MAX_EVENTS) {
    private val pending = ArrayDeque<RobotEvent>()

    init {
        try {
            val saved = JSONArray(store.load() ?: "[]")
            for (i in 0 until saved.length()) pending.addLast(RobotEvent.parse(saved.getJSONObject(i)))
        } catch (e: JSONException) {
            // Unreadable (an older format): start empty.
        }
        trim()
    }

    val size get() = pending.size

    fun add(event: RobotEvent) {
        pending.addLast(event)
        trim()
        persist()
    }

    /** Sends queued events in order while [send] takes them (false: not connected, they stay queued). */
    fun flush(send: (String) -> Boolean) {
        val before = pending.size
        while (pending.isNotEmpty() && send(SignalOut.event(pending.first()))) pending.removeFirst()
        if (pending.size != before) persist()
    }

    private fun trim() {
        while (pending.size > max) pending.removeFirst()
    }

    private fun persist() {
        store.save(JSONArray().apply { pending.forEach { put(it.toJson()) } }.toString())
    }

    companion object {
        const val MAX_EVENTS = 50

        fun prefsStore(context: Context): EventStore {
            val prefs = context.getSharedPreferences("robot_events", Context.MODE_PRIVATE)
            return object : EventStore {
                override fun load() = prefs.getString("queue", null)
                override fun save(json: String) = prefs.edit().putString("queue", json).apply()
            }
        }
    }
}

/**
 * ADM-003: an app crash becomes an `error` event on the next start. The handler writes synchronously (the process
 * is about to die).
 *
 * ROB-011: a provisioned robot must come back after a crash. Handed to the system's handler, a second crash within
 * a minute gets a crash dialog and the process marked "bad", and the system no longer restarts the service: the
 * robot stays offline until someone touches the phone. So for an online robot ([restart]) the handler logs the
 * crash and ends the process itself; the system then restarts the sticky service with its own growing delay
 * (seconds at first, minutes in a crash loop). Motors are safe meanwhile: the base stops on USB detach / timeout.
 */
object CrashRecorder {
    private const val TAG = "AeroLinkService"
    private const val PREFS = "crash"
    private const val MAX_TRACE = 1500

    /** [restart]: whether the service should come back after a crash (a provisioned robot). */
    fun install(context: Context, restart: () -> Boolean) {
        val app = context.applicationContext
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        if (previous is Recorder) return
        Thread.setDefaultUncaughtExceptionHandler(Recorder(app, previous, restart))
    }

    /** The crash of the previous run, once. */
    fun take(context: Context): RobotEvent? {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val trace = prefs.getString("trace", null) ?: return null
        val at = prefs.getLong("at", 0)
        prefs.edit().clear().apply()
        return RobotEvent(EventLevel.ERROR, "app_crash", trace, at)
    }

    private class Recorder(
        private val context: Context,
        private val previous: Thread.UncaughtExceptionHandler?,
        private val restart: () -> Boolean,
    ) : Thread.UncaughtExceptionHandler {
        @SuppressLint("ApplySharedPref") // commit(): the process dies right after
        override fun uncaughtException(thread: Thread, e: Throwable) {
            var quiet = false
            try {
                val trace = "${thread.name}: ${Log.getStackTraceString(e)}".take(MAX_TRACE)
                context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                    .putString("trace", trace)
                    .putLong("at", System.currentTimeMillis() / 1000)
                    .commit()
                quiet = restart()
            } finally {
                if (quiet) {
                    Log.e(TAG, "FATAL in ${thread.name}, the system restarts the service", e)
                    Process.killProcess(Process.myPid())
                    exitProcess(10)
                }
                previous?.uncaughtException(thread, e)
            }
        }
    }
}
