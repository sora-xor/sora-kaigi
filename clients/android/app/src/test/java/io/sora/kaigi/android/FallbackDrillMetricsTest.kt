package io.sora.kaigi.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FallbackDrillMetricsTest {
    @Test
    fun drillMeasurementChecksRtoLimit() {
        val measurement = FallbackDrillMetrics.measure(
            activatedAtMs = 1_000,
            recoveredAtMs = 1_000 + 18 * 60_000L
        )

        assertTrue(measurement.withinLimitMinutes(maxRtoMinutes = 20))
        assertFalse(measurement.withinLimitMinutes(maxRtoMinutes = 10))
    }
}
