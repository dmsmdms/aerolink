package com.aerolink.robot.rtc

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

/**
 * The operator's voice is for the people around the robot: during a session audio goes to the loudspeaker
 * in communication mode (hardware echo cancellation, so the robot's microphone does not send it back), not to
 * the earpiece. The volume is whatever the phone is set to. [release] restores the previous routing.
 */
internal class Speaker(context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val previousMode = audio.mode

    @Suppress("DEPRECATION")
    private val previousSpeakerphone = audio.isSpeakerphoneOn

    init {
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        if (Build.VERSION.SDK_INT >= 31) {
            audio.availableCommunicationDevices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                ?.let(audio::setCommunicationDevice)
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = true
        }
    }

    fun release() {
        if (Build.VERSION.SDK_INT >= 31) {
            audio.clearCommunicationDevice()
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = previousSpeakerphone
        }
        audio.mode = previousMode
    }
}
