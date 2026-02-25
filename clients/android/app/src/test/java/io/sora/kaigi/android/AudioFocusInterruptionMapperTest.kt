package io.sora.kaigi.android

import android.media.AudioManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AudioFocusInterruptionMapperTest {
    @Test
    fun gainWithoutPriorLossDoesNotEmitSignal() {
        val mapper = AudioFocusInterruptionMapper()

        assertNull(mapper.onFocusChange(AudioManager.AUDIOFOCUS_GAIN))
        assertNull(mapper.onFocusChange(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT))
    }

    @Test
    fun lossThenGainEmitsBeganThenEnded() {
        val mapper = AudioFocusInterruptionMapper()

        assertEquals(AudioInterruptionSignal.Began, mapper.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT))
        assertEquals(AudioInterruptionSignal.Ended, mapper.onFocusChange(AudioManager.AUDIOFOCUS_GAIN))
    }

    @Test
    fun duplicateLossWhileInterruptedIsSuppressed() {
        val mapper = AudioFocusInterruptionMapper()

        assertEquals(AudioInterruptionSignal.Began, mapper.onFocusChange(AudioManager.AUDIOFOCUS_LOSS))
        assertNull(mapper.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK))
    }

    @Test
    fun duplicateGainAfterRecoveryIsSuppressed() {
        val mapper = AudioFocusInterruptionMapper()
        mapper.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT)
        mapper.onFocusChange(AudioManager.AUDIOFOCUS_GAIN)

        assertNull(mapper.onFocusChange(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK))
    }
}
