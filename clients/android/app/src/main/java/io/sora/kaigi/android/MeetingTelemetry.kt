package io.sora.kaigi.android

enum class MeetingTelemetryCategory(val wire: String) {
    ConnectionLifecycle("connection_lifecycle"),
    FallbackLifecycle("fallback_lifecycle"),
    PolicyFailure("policy_failure")
}

data class MeetingTelemetryEvent(
    val category: MeetingTelemetryCategory,
    val name: String,
    val atMs: Long = System.currentTimeMillis(),
    val attributes: Map<String, String> = emptyMap()
)

fun interface MeetingTelemetrySink {
    fun record(event: MeetingTelemetryEvent)
}

object NoOpMeetingTelemetrySink : MeetingTelemetrySink {
    override fun record(event: MeetingTelemetryEvent) = Unit
}
