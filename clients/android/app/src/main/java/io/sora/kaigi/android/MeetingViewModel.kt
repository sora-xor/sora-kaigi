package io.sora.kaigi.android

import android.os.Build
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.security.MessageDigest

data class MeetingUiState(
    val config: MeetingConfig = MeetingConfig(),
    val session: ProtocolSessionState = ProtocolSessionState.initial(MeetingConfig()),
    val connected: Boolean = false,
    val transportState: String = ConnectionPhase.Disconnected.label,
    val fallbackActive: Boolean = false,
    val fallbackRtoMs: Long? = null,
    val lastError: String? = null,
    val logs: List<MeetingLog> = emptyList()
)

class MeetingViewModel(
    private val savedStateHandle: SavedStateHandle = SavedStateHandle(),
    private val transport: ProtocolTransport = OkHttpProtocolTransport(),
    private val reducer: ProtocolReducer = DefaultProtocolReducer(),
    private val telemetrySink: MeetingTelemetrySink = NoOpMeetingTelemetrySink,
    private val reconnectBackoffMs: List<Long> = listOf(1_000L, 2_000L, 4_000L, 8_000L)
) : ViewModel(), ProtocolTransport.Listener {

    private val state = MutableStateFlow(createInitialState())
    private var reconnectJob: Job? = null
    private var reconnectAttempt = 0
    private var userInitiatedDisconnect = false
    private var appInBackground = false
    private var participantId: String = resolveParticipantId(state.value.config)

    val uiState: StateFlow<MeetingUiState> = state

    fun updateConfig(transform: (MeetingConfig) -> MeetingConfig) {
        val nextConfig = transform(state.value.config)
        participantId = resolveParticipantId(nextConfig)
        apply(ProtocolEvent.ConfigUpdated(nextConfig))
    }

    fun connect() {
        dispatch(SessionCommand.Connect)
    }

    fun disconnect() {
        dispatch(SessionCommand.Disconnect)
    }

    fun sendPing() {
        dispatch(SessionCommand.Ping)
    }

    fun recoverFromFallback() {
        apply(ProtocolEvent.FallbackRecovered)
        dispatch(SessionCommand.Connect)
    }

    fun onAppForegrounded() {
        dispatch(SessionCommand.LifecycleForegrounded)
    }

    fun onAppBackgrounded() {
        dispatch(SessionCommand.LifecycleBackgrounded)
    }

    fun onConnectivityChanged(available: Boolean) {
        dispatch(SessionCommand.ConnectivityChanged(available))
    }

    fun onAudioInterruptionBegan() {
        dispatch(SessionCommand.AudioInterruptionBegan)
    }

    fun onAudioInterruptionEnded(shouldReconnect: Boolean = true) {
        dispatch(SessionCommand.AudioInterruptionEnded(shouldReconnect = shouldReconnect))
    }

    fun onAudioRouteChanged(reason: String) {
        dispatch(SessionCommand.AudioRouteChanged(reason))
    }

    fun dispatch(command: SessionCommand) {
        when (command) {
            SessionCommand.Connect -> connectInternal(resetBackoff = true, source = "manual")
            SessionCommand.Disconnect -> disconnectInternal()
            SessionCommand.Ping -> sendPingInternal()
            is SessionCommand.Moderate -> sendModeration(command)
            SessionCommand.LifecycleForegrounded -> handleForeground()
            SessionCommand.LifecycleBackgrounded -> handleBackground()
            is SessionCommand.ConnectivityChanged -> handleConnectivity(command.available)
            SessionCommand.AudioInterruptionBegan -> handleAudioInterruptionBegan()
            is SessionCommand.AudioInterruptionEnded -> handleAudioInterruptionEnded(command.shouldReconnect)
            is SessionCommand.AudioRouteChanged -> handleAudioRouteChanged(command.reason)
        }
    }

    override fun onTransportEvent(event: TransportEvent) {
        when (event) {
            TransportEvent.Connected -> {
                if (appInBackground) {
                    transport.disconnect("backgrounded_before_handshake")
                    return
                }
                reconnectAttempt = 0
                apply(ProtocolEvent.TransportConnected)
                emitConnectionTelemetry("transport_connected")
                appendLog(MeetingLogLevel.INFO, "Socket connected")
                sendHandshakeFrames()
            }

            is TransportEvent.Disconnected -> {
                if (userInitiatedDisconnect) {
                    return
                }
                apply(ProtocolEvent.TransportDisconnected(event.reason))
                emitConnectionTelemetry(
                    name = "transport_disconnected",
                    attributes = mapOf("reason" to event.reason)
                )
                appendLog(MeetingLogLevel.WARN, "Socket disconnected: ${event.reason}")
                if (appInBackground) {
                    appendLog(MeetingLogLevel.INFO, "Reconnect deferred while app is backgrounded")
                    return
                }
                scheduleReconnect(event.reason)
            }

            is TransportEvent.FrameReceived -> {
                apply(ProtocolEvent.FrameReceived(event.frame))
                if (event.frame is ProtocolFrame.E2eeKeyEpoch &&
                    hasRequiredSignature(event.frame.signature) &&
                    state.value.session.e2eeState.currentEpoch >= event.frame.epoch
                ) {
                    val ack = ProtocolFrame.KeyRotationAck(
                        participantId = participantId,
                        ackEpoch = event.frame.epoch,
                        receivedAtMs = System.currentTimeMillis()
                    )
                    val sent = transport.send(ack)
                    if (sent) {
                        apply(ProtocolEvent.FrameReceived(ack))
                    } else {
                        apply(ProtocolEvent.FrameSendFailed("key rotation ack send failed"))
                    }
                }
                when (val frame = event.frame) {
                    is ProtocolFrame.Ping -> {
                        transport.send(ProtocolFrame.Pong(System.currentTimeMillis()))
                    }

                    is ProtocolFrame.Error -> {
                        appendLog(MeetingLogLevel.ERROR, "Protocol error [${frame.code}]: ${frame.message}")
                        if (frame.category == SessionErrorCategory.PolicyFailure && state.value.config.preferWebFallbackOnPolicyFailure) {
                            activateFallback("Policy rejection: ${frame.message}")
                        }
                    }

                    is ProtocolFrame.MediaProfileNegotiation -> {
                        if (frame.preferredProfile == MediaProfile.HDR && frame.negotiatedProfile == MediaProfile.SDR) {
                            appendLog(MeetingLogLevel.WARN, "HDR negotiation downgraded to SDR")
                        }
                    }

                    else -> appendLog(MeetingLogLevel.INFO, "Recv frame: ${frame.kind}")
                }
            }

            is TransportEvent.RawMessage -> appendLog(MeetingLogLevel.INFO, "Recv raw: ${event.message}")

            is TransportEvent.SendFailed -> {
                apply(ProtocolEvent.FrameSendFailed(event.message))
                appendLog(MeetingLogLevel.ERROR, "Send failed: ${event.message}")
            }

            is TransportEvent.Failed -> {
                if (userInitiatedDisconnect) {
                    return
                }
                apply(ProtocolEvent.TransportFailure(event.message))
                emitConnectionTelemetry(
                    name = "transport_failed",
                    attributes = mapOf("message" to event.message)
                )
                appendLog(MeetingLogLevel.ERROR, "Transport failure: ${event.message}")
                if (appInBackground) {
                    appendLog(MeetingLogLevel.INFO, "Reconnect deferred while app is backgrounded")
                    return
                }
                scheduleReconnect(event.message)
            }
        }
    }

    private fun connectInternal(resetBackoff: Boolean, source: String) {
        if (appInBackground) {
            appendLog(MeetingLogLevel.INFO, "Connect deferred while app is backgrounded")
            return
        }
        val cfg = state.value.config
        if (cfg.signalingUriOrNull() == null) {
            appendLog(MeetingLogLevel.ERROR, "Invalid signaling URL")
            return
        }
        if (!cfg.isJoinable()) {
            appendLog(MeetingLogLevel.WARN, "Room ID is required")
            return
        }

        userInitiatedDisconnect = false
        cancelReconnect()
        if (resetBackoff) {
            reconnectAttempt = 0
        }

        apply(ProtocolEvent.ConnectRequested)
        emitConnectionTelemetry(
            name = "connect_attempt",
            attributes = mapOf("source" to source)
        )
        appendLog(MeetingLogLevel.INFO, "Connecting to ${cfg.signalingUrl}")
        transport.connect(cfg, this)
    }

    private fun disconnectInternal() {
        userInitiatedDisconnect = true
        cancelReconnect()
        transport.disconnect("user_requested")
        apply(ProtocolEvent.ManualDisconnected)
        emitConnectionTelemetry("manual_disconnect")
        appendLog(MeetingLogLevel.INFO, "Disconnected")
    }

    private fun sendPingInternal() {
        val sent = transport.send(ProtocolFrame.Ping(System.currentTimeMillis()))
        if (sent) {
            appendLog(MeetingLogLevel.INFO, "Sent ping")
        } else {
            apply(ProtocolEvent.FrameSendFailed("socket unavailable"))
            appendLog(MeetingLogLevel.WARN, "Cannot ping while disconnected")
        }
    }

    private fun sendModeration(command: SessionCommand.Moderate) {
        if (!hasLocalModerationAuthority()) {
            val message = "Host/co-host role is required for moderation actions"
            apply(
                ProtocolEvent.FrameReceived(
                    ProtocolFrame.Error(
                        category = SessionErrorCategory.PolicyFailure,
                        code = "moderation_not_authorized",
                        message = message
                    )
                )
            )
            appendLog(MeetingLogLevel.ERROR, "Policy reject [moderation_not_authorized]: $message")
            if (state.value.config.preferWebFallbackOnPolicyFailure) {
                activateFallback(message)
            }
            return
        }

        val sentAtMs = System.currentTimeMillis()
        val frame = ProtocolFrame.ModerationSigned(
            sentAtMs = sentAtMs,
            targetParticipantId = command.targetParticipantId,
            action = command.action,
            issuedBy = participantId,
            signature = makeFrameSignature(
                "moderationSigned",
                participantId,
                command.targetParticipantId,
                command.action.wire,
                sentAtMs.toString()
            )
        )
        val sent = transport.send(frame)
        if (sent) {
            appendLog(MeetingLogLevel.INFO, "Moderation action sent: ${command.action.wire}")
        } else {
            apply(ProtocolEvent.FrameSendFailed("moderation send failed"))
        }
    }

    private fun sendHandshakeFrames() {
        val cfg = state.value.config
        val now = System.currentTimeMillis()
        val preferredProfile = preferredProfileForRuntime()

        val handshake = ProtocolFrame.Handshake(
            roomId = cfg.roomId,
            participantId = participantId,
            participantName = cfg.participant,
            walletIdentity = cfg.walletIdentity,
            resumeToken = state.value.session.resumeToken,
            preferredProfile = preferredProfile,
            hdrCapture = supportsHdrCapture(),
            hdrRender = supportsHdrRender(),
            sentAtMs = now
        )
        transport.send(handshake)

        val capability = ProtocolFrame.DeviceCapability(
            participantId = participantId,
            codecs = listOf("h264", "vp9"),
            hdrCapture = supportsHdrCapture(),
            hdrRender = supportsHdrRender(),
            maxStreams = 4,
            updatedAtMs = now
        )
        transport.send(capability)

        if (cfg.requirePaymentSettlement) {
            transport.send(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://payment-policy"
                )
            )
        }
    }

    private fun handleForeground() {
        if (userInitiatedDisconnect) return
        appInBackground = false
        emitConnectionTelemetry("app_foregrounded")
        val phase = state.value.session.connectionPhase
        if (phase == ConnectionPhase.Disconnected || phase == ConnectionPhase.Degraded || phase == ConnectionPhase.Error) {
            appendLog(MeetingLogLevel.INFO, "Foreground resume connect")
            connectInternal(resetBackoff = false, source = "foreground")
        }
    }

    private fun handleBackground() {
        appInBackground = true
        appendLog(MeetingLogLevel.INFO, "App moved to background")
        emitConnectionTelemetry("app_backgrounded")
        if (userInitiatedDisconnect) return
        val phase = state.value.session.connectionPhase
        if (phase == ConnectionPhase.Connected || phase == ConnectionPhase.Connecting || phase == ConnectionPhase.Degraded) {
            transport.disconnect("app_backgrounded")
            apply(ProtocolEvent.TransportDisconnected("app_backgrounded"))
        }
    }

    private fun handleConnectivity(available: Boolean) {
        if (userInitiatedDisconnect) return
        if (available) {
            appendLog(MeetingLogLevel.INFO, "Connectivity restored")
            emitConnectionTelemetry("network_available")
            if (appInBackground) {
                appendLog(MeetingLogLevel.INFO, "Connectivity restore deferred while app is backgrounded")
                return
            }
            val phase = state.value.session.connectionPhase
            if (phase != ConnectionPhase.Connected && phase != ConnectionPhase.Connecting) {
                connectInternal(resetBackoff = false, source = "connectivity_restore")
            }
        } else {
            appendLog(MeetingLogLevel.WARN, "Connectivity lost")
            emitConnectionTelemetry("network_unavailable")
            apply(ProtocolEvent.TransportFailure("network_unavailable"))
            scheduleReconnect("network_unavailable")
        }
    }

    private fun handleAudioInterruptionBegan() {
        if (userInitiatedDisconnect) return
        appendLog(MeetingLogLevel.WARN, "Audio interruption began")
        emitConnectionTelemetry("audio_interruption_began")
        apply(ProtocolEvent.TransportFailure("audio_interruption"))
    }

    private fun handleAudioInterruptionEnded(shouldReconnect: Boolean) {
        if (userInitiatedDisconnect) return
        appendLog(MeetingLogLevel.INFO, "Audio interruption ended")
        emitConnectionTelemetry(
            name = "audio_interruption_ended",
            attributes = mapOf("should_reconnect" to shouldReconnect.toString())
        )
        if (!shouldReconnect || appInBackground || state.value.session.fallback.active) {
            return
        }
        val phase = state.value.session.connectionPhase
        if (phase != ConnectionPhase.Connected && phase != ConnectionPhase.Connecting) {
            connectInternal(resetBackoff = false, source = "audio_interruption_end")
        }
    }

    private fun handleAudioRouteChanged(reason: String) {
        appendLog(MeetingLogLevel.INFO, "Audio route changed: $reason")
        emitConnectionTelemetry(
            name = "audio_route_changed",
            attributes = mapOf("reason" to reason)
        )
    }

    private fun scheduleReconnect(reason: String) {
        if (userInitiatedDisconnect) return
        if (appInBackground) return
        if (state.value.session.fallback.active) return
        if (reconnectJob != null) return

        if (reconnectAttempt >= reconnectBackoffMs.size) {
            activateFallback("Reconnect exhausted after $reconnectAttempt attempts: $reason")
            return
        }

        val delayMs = reconnectBackoffMs[reconnectAttempt]
        val attemptNumber = reconnectAttempt + 1
        reconnectAttempt += 1

        appendLog(MeetingLogLevel.WARN, "Scheduling reconnect #$attemptNumber in ${delayMs}ms")
        emitConnectionTelemetry(
            name = "reconnect_scheduled",
            attributes = mapOf(
                "attempt" to attemptNumber.toString(),
                "delay_ms" to delayMs.toString(),
                "trigger" to reason
            )
        )

        reconnectJob = viewModelScope.launch {
            delay(delayMs)
            reconnectJob = null
            if (!userInitiatedDisconnect && !appInBackground) {
                appendLog(MeetingLogLevel.INFO, "Reconnect attempt #$attemptNumber")
                connectInternal(resetBackoff = false, source = "reconnect")
            }
        }
    }

    private fun activateFallback(reason: String) {
        if (state.value.session.fallback.active) return
        cancelReconnect()
        apply(ProtocolEvent.FallbackActivated(reason))
        emitFallbackTelemetry(
            name = "fallback_activated",
            attributes = mapOf("reason" to reason)
        )
        appendLog(MeetingLogLevel.WARN, "Fallback activated: $reason")
        transport.disconnect("fallback_activated")
    }

    private fun apply(event: ProtocolEvent) {
        val previousSession = state.value.session
        val nextSession = reducer.reduce(previousSession, event, System.currentTimeMillis())
        val shouldTriggerFallback = shouldTriggerFallbackFromPolicyFailure(previous = previousSession, current = nextSession)

        if (previousSession.fallback.active && !nextSession.fallback.active && nextSession.fallback.lastRtoMs != null) {
            appendLog(MeetingLogLevel.INFO, "Fallback recovered in ${nextSession.fallback.lastRtoMs} ms")
            emitFallbackTelemetry(
                name = "fallback_recovered",
                attributes = mapOf("rto_ms" to nextSession.fallback.lastRtoMs.toString())
            )
        }

        if (previousSession.connectionPhase != nextSession.connectionPhase) {
            emitConnectionTelemetry(
                name = "phase_transition",
                attributes = mapOf(
                    "from" to previousSession.connectionPhase.label,
                    "to" to nextSession.connectionPhase.label
                )
            )
        }

        val currentError = nextSession.lastError
        if (currentError != null &&
            currentError.category == SessionErrorCategory.PolicyFailure &&
            previousSession.lastError?.atMs != currentError.atMs
        ) {
            emitPolicyFailureTelemetry(
                code = currentError.code,
                message = currentError.message
            )
        }

        persistConfig(nextSession.config)
        savedStateHandle[KEY_RESUME_TOKEN] = nextSession.resumeToken
        savedStateHandle[KEY_FALLBACK_ACTIVE] = nextSession.fallback.active
        savedStateHandle[KEY_FALLBACK_REASON] = nextSession.fallback.reason

        val lastError = nextSession.lastError?.let { "${it.code}: ${it.message}" }

        state.update {
            it.copy(
                config = nextSession.config,
                session = nextSession,
                connected = nextSession.connectionPhase.online,
                transportState = nextSession.connectionPhase.label,
                fallbackActive = nextSession.fallback.active,
                fallbackRtoMs = nextSession.fallback.lastRtoMs,
                lastError = lastError
            )
        }

        if (shouldTriggerFallback) {
            activateFallback(nextSession.lastError?.message ?: "policy_failure")
        }
    }

    private fun cancelReconnect() {
        reconnectJob?.cancel()
        reconnectJob = null
    }

    private fun appendLog(level: MeetingLogLevel, message: String) {
        state.update {
            val next = listOf(MeetingLog(level, message)) + it.logs
            it.copy(logs = next.take(400))
        }
    }

    private fun createInitialState(): MeetingUiState {
        val config = restoredConfig()
        val restoredFallbackActive: Boolean = savedStateHandle[KEY_FALLBACK_ACTIVE] ?: false
        val restoredFallbackReason: String? = savedStateHandle[KEY_FALLBACK_REASON]

        val baseSession = ProtocolSessionState.initial(config).copy(
            resumeToken = savedStateHandle[KEY_RESUME_TOKEN],
            fallback = FallbackState.DEFAULT.copy(
                active = restoredFallbackActive,
                reason = restoredFallbackReason
            )
        )

        val session = if (restoredFallbackActive) {
            baseSession.copy(connectionPhase = ConnectionPhase.FallbackActive)
        } else {
            baseSession
        }
        return MeetingUiState(
            config = config,
            session = session,
            connected = session.connectionPhase.online,
            transportState = session.connectionPhase.label,
            fallbackActive = session.fallback.active,
            fallbackRtoMs = session.fallback.lastRtoMs,
            lastError = null,
            logs = emptyList()
        )
    }

    private fun restoredConfig(): MeetingConfig {
        val defaults = MeetingConfig()
        return MeetingConfig(
            signalingUrl = savedStateHandle[KEY_SIGNALING_URL] ?: defaults.signalingUrl,
            fallbackUrl = savedStateHandle[KEY_FALLBACK_URL] ?: defaults.fallbackUrl,
            roomId = savedStateHandle[KEY_ROOM_ID] ?: defaults.roomId,
            participant = savedStateHandle[KEY_PARTICIPANT] ?: defaults.participant,
            participantId = savedStateHandle[KEY_PARTICIPANT_ID] ?: defaults.participantId,
            walletIdentity = savedStateHandle[KEY_WALLET_IDENTITY] ?: defaults.walletIdentity,
            requireSignedModeration = savedStateHandle[KEY_REQUIRE_SIGNED] ?: defaults.requireSignedModeration,
            requirePaymentSettlement = savedStateHandle[KEY_REQUIRE_PAYMENT] ?: defaults.requirePaymentSettlement,
            preferWebFallbackOnPolicyFailure = savedStateHandle[KEY_PREFER_WEB_FALLBACK] ?: defaults.preferWebFallbackOnPolicyFailure,
            supportsHdrCapture = savedStateHandle[KEY_SUPPORTS_HDR_CAPTURE] ?: defaults.supportsHdrCapture,
            supportsHdrRender = savedStateHandle[KEY_SUPPORTS_HDR_RENDER] ?: defaults.supportsHdrRender
        )
    }

    private fun persistConfig(config: MeetingConfig) {
        savedStateHandle[KEY_SIGNALING_URL] = config.signalingUrl
        savedStateHandle[KEY_FALLBACK_URL] = config.fallbackUrl
        savedStateHandle[KEY_ROOM_ID] = config.roomId
        savedStateHandle[KEY_PARTICIPANT] = config.participant
        savedStateHandle[KEY_PARTICIPANT_ID] = config.participantId
        savedStateHandle[KEY_WALLET_IDENTITY] = config.walletIdentity
        savedStateHandle[KEY_REQUIRE_SIGNED] = config.requireSignedModeration
        savedStateHandle[KEY_REQUIRE_PAYMENT] = config.requirePaymentSettlement
        savedStateHandle[KEY_PREFER_WEB_FALLBACK] = config.preferWebFallbackOnPolicyFailure
        savedStateHandle[KEY_SUPPORTS_HDR_CAPTURE] = config.supportsHdrCapture
        savedStateHandle[KEY_SUPPORTS_HDR_RENDER] = config.supportsHdrRender
    }

    private fun preferredProfileForRuntime(): MediaProfile {
        return if (supportsHdrCapture() && supportsHdrRender()) {
            MediaProfile.HDR
        } else {
            MediaProfile.SDR
        }
    }

    private fun supportsHdrCapture(): Boolean = state.value.config.supportsHdrCapture ?: (Build.VERSION.SDK_INT >= 33)

    private fun supportsHdrRender(): Boolean = state.value.config.supportsHdrRender ?: (Build.VERSION.SDK_INT >= 33)

    private fun resolveParticipantId(config: MeetingConfig): String {
        val explicit = config.participantId?.trim().orEmpty()
        if (explicit.isNotEmpty()) {
            return normalizeParticipantId(explicit)
        }
        return normalizeParticipantId(config.participant)
    }

    private fun normalizeParticipantId(raw: String): String {
        val base = raw.trim().ifBlank { "participant" }
        val normalized = buildString {
            base.lowercase().forEach { ch ->
                when {
                    (ch in 'a'..'z') || (ch in '0'..'9') || ch == '-' || ch == '_' -> append(ch)
                    ch.isWhitespace() -> append('-')
                }
            }
        }
        return normalized.ifBlank { "participant" }
    }

    private fun hasLocalModerationAuthority(): Boolean {
        val actor = state.value.session.participants[participantId] ?: return false
        return actor.role == ParticipantRole.Host || actor.role == ParticipantRole.CoHost
    }

    private fun hasRequiredSignature(signature: String): Boolean {
        if (!state.value.config.requireSignedModeration) {
            return true
        }
        return signature.isNotBlank()
    }

    private fun shouldTriggerFallbackFromPolicyFailure(
        previous: ProtocolSessionState,
        current: ProtocolSessionState
    ): Boolean {
        if (!current.config.preferWebFallbackOnPolicyFailure) return false
        if (current.fallback.active) return false
        val error = current.lastError ?: return false
        if (error.category != SessionErrorCategory.PolicyFailure) return false
        return previous.lastError?.atMs != error.atMs
    }

    private fun makeFrameSignature(vararg parts: String): String {
        val payload = parts.joinToString(separator = "|")
        val digest = MessageDigest.getInstance("SHA-256").digest(payload.toByteArray())
        return digest.joinToString(separator = "") { byte -> "%02x".format(byte.toInt() and 0xff) }
    }

    private fun emitConnectionTelemetry(name: String, attributes: Map<String, String> = emptyMap()) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category = MeetingTelemetryCategory.ConnectionLifecycle,
                name = name,
                attributes = attributes
            )
        )
    }

    private fun emitFallbackTelemetry(name: String, attributes: Map<String, String> = emptyMap()) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category = MeetingTelemetryCategory.FallbackLifecycle,
                name = name,
                attributes = attributes
            )
        )
    }

    private fun emitPolicyFailureTelemetry(code: String, message: String) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category = MeetingTelemetryCategory.PolicyFailure,
                name = "policy_reject",
                attributes = mapOf(
                    "code" to code,
                    "message" to message
                )
            )
        )
    }

    override fun onCleared() {
        cancelReconnect()
        transport.shutdown()
        super.onCleared()
    }

    companion object {
        private const val KEY_SIGNALING_URL = "cfg_signaling_url"
        private const val KEY_FALLBACK_URL = "cfg_fallback_url"
        private const val KEY_ROOM_ID = "cfg_room_id"
        private const val KEY_PARTICIPANT = "cfg_participant"
        private const val KEY_PARTICIPANT_ID = "cfg_participant_id"
        private const val KEY_WALLET_IDENTITY = "cfg_wallet_identity"
        private const val KEY_REQUIRE_SIGNED = "cfg_require_signed"
        private const val KEY_REQUIRE_PAYMENT = "cfg_require_payment"
        private const val KEY_PREFER_WEB_FALLBACK = "cfg_prefer_web_fallback"
        private const val KEY_SUPPORTS_HDR_CAPTURE = "cfg_supports_hdr_capture"
        private const val KEY_SUPPORTS_HDR_RENDER = "cfg_supports_hdr_render"
        private const val KEY_RESUME_TOKEN = "session_resume_token"
        private const val KEY_FALLBACK_ACTIVE = "session_fallback_active"
        private const val KEY_FALLBACK_REASON = "session_fallback_reason"
    }
}
