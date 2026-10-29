package com.aerolink.robot.link

import android.os.ParcelFileDescriptor
import java.io.Closeable
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException

/** Byte pipe to the RP2040. [read] blocks; [close] must unblock a pending [read]. */
interface LinkTransport : Closeable {
    /** Reads at least one byte into [buffer]; returns the count, or -1 when the link is gone. */
    fun read(buffer: ByteArray): Int

    /** Writes one or more complete wire frames. Throws [IOException] when the link is gone. */
    fun write(data: ByteArray)
}

/**
 * Android Open Accessory endpoints (docs/03-usb-link.md, LINK-001). The accessory gadget delivers at
 * most one USB transfer per read, so callers must pass a buffer of [READ_BUFFER_SIZE] bytes:
 * a smaller buffer than the incoming transfer loses data.
 */
class AccessoryTransport(private val fd: ParcelFileDescriptor) : LinkTransport {
    private val input = FileInputStream(fd.fileDescriptor)
    private val output = FileOutputStream(fd.fileDescriptor)

    override fun read(buffer: ByteArray): Int =
        try {
            input.read(buffer)
        } catch (e: IOException) {
            -1 // detached (ENODEV) or closed
        }

    override fun write(data: ByteArray) = output.write(data)

    override fun close() = fd.close()

    companion object {
        const val READ_BUFFER_SIZE = 16384
    }
}
