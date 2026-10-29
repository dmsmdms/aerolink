package com.aerolink.robot.link

// USB link framing (docs/03-usb-link.md, LINK-010..013): COBS + CRC-16/CCITT-FALSE,
// 0x00-delimited. Must stay byte-compatible with rp2040/src/proto.c (shared protocol/vectors.txt).

const val PROTOCOL_VERSION = 2
const val MAX_FRAME = 64
const val MAX_PAYLOAD = MAX_FRAME - 4
private const val MAX_ENCODED = MAX_FRAME + 1

class Frame(val type: Int, val seq: Int, val payload: ByteArray)

object FrameCodec {
    fun crc16(data: ByteArray, len: Int = data.size): Int {
        var crc = 0xFFFF
        for (i in 0 until len) {
            crc = crc xor ((data[i].toInt() and 0xFF) shl 8)
            repeat(8) {
                crc = if (crc and 0x8000 != 0) (crc shl 1) xor 0x1021 else crc shl 1
                crc = crc and 0xFFFF
            }
        }
        return crc
    }

    /** Encodes a frame for the wire (COBS + trailing 0x00). */
    fun encode(type: Int, seq: Int, payload: ByteArray = EMPTY): ByteArray {
        require(payload.size <= MAX_PAYLOAD) { "payload too long: ${payload.size}" }
        val raw = ByteArray(payload.size + 4)
        raw[0] = type.toByte()
        raw[1] = seq.toByte()
        payload.copyInto(raw, 2)
        val crc = crc16(raw, payload.size + 2)
        raw[payload.size + 2] = crc.toByte()
        raw[payload.size + 3] = (crc shr 8).toByte()
        return cobsEncode(raw) + 0
    }

    private fun cobsEncode(input: ByteArray): ByteArray {
        val out = ByteArray(input.size + input.size / 254 + 1)
        var codeIdx = 0
        var o = 1
        var code = 1
        for (b in input) {
            if (b.toInt() == 0) {
                out[codeIdx] = code.toByte()
                codeIdx = o++
                code = 1
            } else {
                out[o++] = b
                if (++code == 0xFF) {
                    out[codeIdx] = code.toByte()
                    codeIdx = o++
                    code = 1
                }
            }
        }
        out[codeIdx] = code.toByte()
        return out.copyOf(o)
    }

    /** Returns the decoded bytes, or null if the input is not valid COBS or longer than [cap]. */
    internal fun cobsDecode(input: ByteArray, len: Int, cap: Int): ByteArray? {
        val out = ByteArray(cap)
        var i = 0
        var o = 0
        while (i < len) {
            val code = input[i++].toInt() and 0xFF
            if (code == 0 || i + code - 1 > len) return null
            for (k in 1 until code) {
                if (o >= cap) return null
                out[o++] = input[i++]
            }
            if (code != 0xFF && i < len) {
                if (o >= cap) return null
                out[o++] = 0
            }
        }
        return out.copyOf(o)
    }

    private val EMPTY = ByteArray(0)
}

/**
 * Streaming decoder: feed raw bytes as they arrive from USB, in any chunking.
 * Malformed frames are dropped and counted in [errors] (LINK-012).
 */
class FrameDecoder {
    private val buf = ByteArray(MAX_ENCODED)
    private var len = 0
    private var overflow = false

    var errors: Long = 0
        private set

    fun push(data: ByteArray, offset: Int = 0, length: Int = data.size, onFrame: (Frame) -> Unit) {
        for (i in offset until offset + length) {
            val b = data[i]
            if (b.toInt() != 0) {
                if (len < buf.size) buf[len++] = b else overflow = true
                continue
            }
            val frame = if (overflow || len == 0) null else decode()
            len = 0
            overflow = false
            if (frame != null) onFrame(frame) else errors++
        }
    }

    fun reset() {
        len = 0
        overflow = false
    }

    private fun decode(): Frame? {
        val raw = FrameCodec.cobsDecode(buf, len, MAX_FRAME) ?: return null
        if (raw.size < 4) return null
        val body = raw.size - 2
        val crc = (raw[body].toInt() and 0xFF) or ((raw[body + 1].toInt() and 0xFF) shl 8)
        if (FrameCodec.crc16(raw, body) != crc) return null
        return Frame(raw[0].toInt() and 0xFF, raw[1].toInt() and 0xFF, raw.copyOfRange(2, body))
    }
}
