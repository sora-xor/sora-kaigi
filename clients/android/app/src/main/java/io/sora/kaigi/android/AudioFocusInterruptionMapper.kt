package io.sora.kaigi.android

import android.media.AudioManager

internal enum class AudioInterruptionSignal {
    Began,
    Ended
}

/**
 * Maps noisy audio-focus callback streams to interruption lifecycle edges.
 * We only emit Ended after a prior Began to avoid spurious reconnect triggers.
 */
internal class AudioFocusInterruptionMapper {
    private var interruptionActive = false

    fun onFocusChange(change: Int): AudioInterruptionSignal? {
        return when (change) {
            AudioManager.AUDIOFOCUS_LOSS,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                if (interruptionActive) {
                    null
                } else {
                    interruptionActive = true
                    AudioInterruptionSignal.Began
                }
            }

            AudioManager.AUDIOFOCUS_GAIN,
            AudioManager.AUDIOFOCUS_GAIN_TRANSIENT,
            AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE,
            AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK -> {
                if (!interruptionActive) {
                    null
                } else {
                    interruptionActive = false
                    AudioInterruptionSignal.Ended
                }
            }

            else -> null
        }
    }
}
