package com.aerolink.robot.rtc

import android.content.Context
import android.content.pm.PackageManager
import android.hardware.Camera
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.os.Handler
import android.util.Log
import android.util.Range
import android.view.Surface
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import org.webrtc.CameraEnumerationAndroid
import org.webrtc.CameraVideoCapturer

/**
 * The flashlight of the camera WebRTC is capturing from (CTL-004). Android cannot switch the torch of a
 * camera that is in use (`CameraManager.setTorchMode` fails with CAMERA_IN_USE), and WebRTC's capturer has no
 * torch API, so this reaches into its camera session (private fields, kept by the `org.webrtc.**` ProGuard
 * rule) and changes the capture request on the camera thread, as flutter-webrtc does. When that fails (another
 * WebRTC version renamed something) the light just stays off.
 */
internal object Torch {
    private const val TAG = "AeroLinkRtc"

    fun available(context: Context, camera2: Boolean, cameraName: String): Boolean = try {
        if (camera2) {
            val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
            manager.getCameraCharacteristics(cameraName).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
        } else {
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_FLASH) // back camera
        }
    } catch (e: Exception) {
        false
    }

    suspend fun set(capturer: CameraVideoCapturer, on: Boolean): Boolean {
        val handler = runCatching { field(capturer, "cameraThreadHandler") as Handler }.getOrNull() ?: return false
        val done = CompletableDeferred<Boolean>()
        handler.post {
            done.complete(
                try {
                    val session = field(capturer, "currentSession")
                    when (session?.javaClass?.name) {
                        "org.webrtc.Camera2Session" -> camera2(session, handler, on)
                        "org.webrtc.Camera1Session" -> camera1(session, on)
                        else -> false
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "torch", e)
                    false
                },
            )
        }
        return withTimeoutOrNull(2000) { done.await() } ?: false
    }

    /** The repeating request of `Camera2Session.CaptureSessionCallback.onConfigured`, plus the flash mode. */
    private fun camera2(session: Any, handler: Handler, on: Boolean): Boolean {
        val device = field(session, "cameraDevice") as CameraDevice
        val capture = field(session, "captureSession") as CameraCaptureSession
        val surface = field(session, "surface") as Surface
        val format = field(session, "captureFormat") as CameraEnumerationAndroid.CaptureFormat
        val fpsUnit = field(session, "fpsUnitFactor") as Int
        val characteristics = field(session, "cameraCharacteristics") as CameraCharacteristics
        val request = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(format.framerate.min / fpsUnit, format.framerate.max / fpsUnit))
            set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON)
            set(CaptureRequest.CONTROL_AE_LOCK, false)
            val af = characteristics.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)
            if (af?.contains(CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO) == true) {
                set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            }
            set(CaptureRequest.FLASH_MODE, if (on) CaptureRequest.FLASH_MODE_TORCH else CaptureRequest.FLASH_MODE_OFF)
            addTarget(surface)
        }.build()
        capture.setRepeatingRequest(request, null, handler)
        return true
    }

    @Suppress("DEPRECATION")
    private fun camera1(session: Any, on: Boolean): Boolean {
        val camera = field(session, "camera") as Camera
        val params = camera.parameters
        if (params.supportedFlashModes?.contains(Camera.Parameters.FLASH_MODE_TORCH) != true) return false
        params.flashMode = if (on) Camera.Parameters.FLASH_MODE_TORCH else Camera.Parameters.FLASH_MODE_OFF
        camera.parameters = params
        return true
    }

    private fun field(target: Any, name: String): Any? {
        var c: Class<*>? = target.javaClass
        while (c != null) {
            try {
                return c.getDeclaredField(name).apply { isAccessible = true }.get(target)
            } catch (e: NoSuchFieldException) {
                c = c.superclass
            }
        }
        throw NoSuchFieldException(name)
    }
}
