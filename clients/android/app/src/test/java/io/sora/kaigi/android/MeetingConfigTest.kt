package io.sora.kaigi.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MeetingConfigTest {
    @Test
    fun acceptsWebSocketUrl() {
        val config = MeetingConfig(signalingUrl = "wss://relay.example.com/ws")
        assertNotNull(config.signalingUriOrNull())
        assertTrue(config.isJoinable())
    }

    @Test
    fun rejectsInvalidScheme() {
        val config = MeetingConfig(signalingUrl = "ftp://relay.example.com")
        assertNull(config.signalingUriOrNull())
        assertFalse(config.isJoinable())
    }
}
