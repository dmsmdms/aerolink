package com.aerolink.robot.service

import com.aerolink.robot.signal.EventLevel
import com.aerolink.robot.signal.RobotEvent
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

class EventQueueTest {
    private class MemoryStore(var json: String? = null) : EventStore {
        override fun load() = json
        override fun save(json: String) {
            this.json = json
        }
    }

    private fun event(n: Int) = RobotEvent(EventLevel.WARNING, "c$n", "m$n", n.toLong())

    @Test
    fun sendsInOrderWhenOnline() {
        val queue = EventQueue(MemoryStore())
        queue.add(event(1))
        queue.add(event(2))
        val sent = mutableListOf<String>()
        queue.flush { sent += JSONObject(it).getString("code"); true }
        assertEquals(listOf("c1", "c2"), sent)
        assertEquals(0, queue.size)
    }

    @Test
    fun keepsWhatCouldNotBeSent() {
        val queue = EventQueue(MemoryStore())
        repeat(3) { queue.add(event(it)) }
        var budget = 1
        queue.flush { budget-- > 0 }
        assertEquals(2, queue.size)
        queue.flush { false }
        assertEquals(2, queue.size)
    }

    @Test
    fun boundedOldestDropped() {
        val queue = EventQueue(MemoryStore(), max = 3)
        repeat(5) { queue.add(event(it)) }
        val sent = mutableListOf<String>()
        queue.flush { sent += JSONObject(it).getString("code"); true }
        assertEquals(listOf("c2", "c3", "c4"), sent)
    }

    @Test
    fun survivesRestartsAndBadData() {
        val store = MemoryStore()
        EventQueue(store).add(event(7))
        val again = EventQueue(store)
        assertEquals(1, again.size)
        var got: RobotEvent? = null
        again.flush { got = RobotEvent.parse(JSONObject(it)); true }
        assertEquals(event(7), got)
        assertEquals(0, EventQueue(MemoryStore("{broken")).size)
    }
}
