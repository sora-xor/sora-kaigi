package io.sora.kaigi.android

import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

enum class MeetingLogLevel { INFO, WARN, ERROR }

data class MeetingLog(
    val level: MeetingLogLevel,
    val message: String,
    val at: Long = System.currentTimeMillis()
) {
    fun formatted(): String {
        val stamp = SimpleDateFormat("HH:mm:ss", Locale.US).format(Date(at))
        return "[$stamp] ${level.name}: $message"
    }
}
