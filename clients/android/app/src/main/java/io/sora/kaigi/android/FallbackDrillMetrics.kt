package io.sora.kaigi.android

data class FallbackDrillMeasurement(
    val activatedAtMs: Long,
    val recoveredAtMs: Long,
    val rtoMs: Long
) {
    fun withinLimitMinutes(maxRtoMinutes: Int): Boolean {
        return rtoMs <= maxRtoMinutes * 60_000L
    }
}

object FallbackDrillMetrics {
    fun measure(activatedAtMs: Long, recoveredAtMs: Long): FallbackDrillMeasurement {
        val clampedRecovered = if (recoveredAtMs >= activatedAtMs) recoveredAtMs else activatedAtMs
        return FallbackDrillMeasurement(
            activatedAtMs = activatedAtMs,
            recoveredAtMs = clampedRecovered,
            rtoMs = clampedRecovered - activatedAtMs
        )
    }
}
