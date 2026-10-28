package com.aerolink.robot.link

import java.nio.ByteBuffer
import java.nio.ByteOrder

// USB link message set, protocol v1 (docs/03-usb-link.md "Messages"); mirrors rp2040/src/messages.h.

enum class DriveSource(val code: Int) { LOCAL(0), OPERATOR(1), AUTOPILOT(2) }

enum class RobotState(val code: Int) {
    DISARMED(0), ARMED(1), STOPPING(2), FAULT(3);

    companion object {
        fun of(code: Int) = entries.firstOrNull { it.code == code }
    }
}

object Caps {
    const val MOTOR_FEEDBACK = 1 shl 0
    const val BATTERY_VOLTAGE = 1 shl 1
    const val MOTOR_SIMULATED = 1 shl 2
}

object Faults {
    const val MOTOR_LINK_LOST = 1 shl 0
    const val MOTOR_FAULT = 1 shl 1
    const val LOW_BATTERY = 1 shl 2
    const val OVERTEMP = 1 shl 3
}

object EventCode {
    const val BOOT = 1
    const val STATE = 2
    const val FAULTS = 3
    const val CMD_TIMEOUT = 4
}

object NackReason {
    const val BAD_STATE = 1
    const val BAD_ARG = 2
    const val FAULT_ACTIVE = 3
}

private fun payload(size: Int, fill: ByteBuffer.() -> Unit): ByteArray =
    ByteBuffer.allocate(size).order(ByteOrder.LITTLE_ENDIAN).apply(fill).array()

/** Phone -> firmware. */
sealed class Command(val type: Int) {
    abstract fun payload(): ByteArray

    data class Hello(val protocolVersion: Int = PROTOCOL_VERSION) : Command(0x01) {
        override fun payload() = payload(1) { put(protocolVersion.toByte()) }
    }

    data object Arm : Command(0x02) {
        override fun payload() = ByteArray(0)
    }

    data object Disarm : Command(0x03) {
        override fun payload() = ByteArray(0)
    }

    /** [speed] and [steer] in ‰ (−1000..1000; + forward / + right). */
    data class Drive(val speed: Int, val steer: Int, val source: DriveSource, val ttlMs: Int) : Command(0x04) {
        override fun payload() = payload(7) {
            putShort(speed.coerceIn(-1000, 1000).toShort())
            putShort(steer.coerceIn(-1000, 1000).toShort())
            put(source.code.toByte())
            putShort(ttlMs.coerceIn(0, 0xFFFF).toShort())
        }
    }

    data object Ping : Command(0x05) {
        override fun payload() = ByteArray(0)
    }

    data class Config(
        val maxSpeed: Int,
        val accel: Int,
        val telemetryPeriodMs: Int,
        val cmdTimeoutMs: Int,
    ) : Command(0x06) {
        override fun payload() = payload(8) {
            putShort(maxSpeed.toShort())
            putShort(accel.toShort())
            putShort(telemetryPeriodMs.toShort())
            putShort(cmdTimeoutMs.toShort())
        }
    }

    data object RebootBootsel : Command(0x07) {
        const val MAGIC = 0x544F4F42 // "BOOT"
        override fun payload() = payload(4) { putInt(MAGIC) }
    }
}

/** Firmware -> phone. */
sealed class Report {
    data class HelloAck(
        val protocolVersion: Int,
        val firmwareVersion: String,
        val caps: Int,
        val boardId: Long,
    ) : Report()

    data class Telemetry(
        val state: RobotState?,
        val faults: Int,
        /** null if unknown. */
        val batteryMv: Int?,
        val speedLeft: Int,
        val speedRight: Int,
        /** 0.1 °C, null if unknown. */
        val tempDeciC: Int?,
        val rxErrors: Int,
    ) : Report()

    data class Event(val code: Int, val arg: Int) : Report()

    data class Nack(val rejectedType: Int, val reason: Int) : Report()

    data object Pong : Report()

    companion object {
        /** Typed view of a firmware frame, or null for unknown type / wrong length (LINK-012). */
        fun parse(frame: Frame): Report? {
            val p = ByteBuffer.wrap(frame.payload).order(ByteOrder.LITTLE_ENDIAN)
            fun u8() = p.get().toInt() and 0xFF
            fun u16() = p.short.toInt() and 0xFFFF
            fun i16() = p.short.toInt()
            return when {
                frame.type == 0x81 && frame.payload.size == 16 -> HelloAck(
                    protocolVersion = u8(),
                    firmwareVersion = "${u8()}.${u8()}.${u8()}",
                    caps = p.int,
                    boardId = p.order(ByteOrder.BIG_ENDIAN).long,
                )
                frame.type == 0x82 && frame.payload.size == 13 -> Telemetry(
                    state = RobotState.of(u8()),
                    faults = u16(),
                    batteryMv = u16().takeIf { it != 0 },
                    speedLeft = i16(),
                    speedRight = i16(),
                    tempDeciC = i16().takeIf { it != Short.MIN_VALUE.toInt() },
                    rxErrors = u16(),
                )
                frame.type == 0x83 && frame.payload.size == 3 -> Event(u8(), u16())
                frame.type == 0x84 && frame.payload.size == 2 -> Nack(u8(), u8())
                frame.type == 0x85 && frame.payload.isEmpty() -> Pong
                else -> null
            }
        }
    }
}
