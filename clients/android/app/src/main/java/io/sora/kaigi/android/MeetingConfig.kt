package io.sora.kaigi.android

import java.net.URI

data class MeetingConfig(
    val signalingUrl: String = "ws://10.0.2.2:9000",
    val fallbackUrl: String = "https://10.0.2.2:8080",
    val roomId: String = "daily-standup",
    val participant: String = "Alice",
    val participantId: String? = null,
    val walletIdentity: String? = "nexus://wallet/alice",
    val requireSignedModeration: Boolean = true,
    val requirePaymentSettlement: Boolean = false,
    val preferWebFallbackOnPolicyFailure: Boolean = true,
    val supportsHdrCapture: Boolean? = null,
    val supportsHdrRender: Boolean? = null
) {
    fun signalingUriOrNull(): URI? = normalizedUri(signalingUrl)

    fun fallbackUriOrNull(): URI? = normalizedUri(fallbackUrl)

    fun isJoinable(): Boolean = signalingUriOrNull() != null && roomId.trim().isNotEmpty()

    private fun normalizedUri(raw: String): URI? {
        val trimmed = raw.trim()
        if (trimmed.isEmpty()) return null
        val uri = try {
            URI(trimmed)
        } catch (_: IllegalArgumentException) {
            return null
        }
        val scheme = uri.scheme?.lowercase() ?: return null
        if (scheme !in setOf("ws", "wss", "http", "https")) return null
        return uri
    }
}
