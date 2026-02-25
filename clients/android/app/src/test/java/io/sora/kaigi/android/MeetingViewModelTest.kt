package io.sora.kaigi.android

import androidx.lifecycle.SavedStateHandle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MeetingViewModelTest {
    @Test
    fun configUpdateTriggersFallbackForPaymentPolicyFailure() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(5_000L)
        )

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            )
        )

        assertFalse(viewModel.uiState.value.fallbackActive)
        assertEquals(PaymentSettlementStatus.Pending, viewModel.uiState.value.session.paymentState.settlementStatus)

        viewModel.updateConfig { current ->
            current.copy(
                requirePaymentSettlement = true,
                preferWebFallbackOnPolicyFailure = true
            )
        }

        assertTrue(viewModel.uiState.value.fallbackActive)
        assertEquals(ConnectionPhase.FallbackActive, viewModel.uiState.value.session.connectionPhase)
        assertEquals("fallback_activated", viewModel.uiState.value.session.lastError?.code)
        assertEquals("fallback_activated", transport.lastDisconnectReason)
    }

    @Test
    fun reconnectExhaustionImmediatelyActivatesFallbackWhenNoBackoffConfigured() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = emptyList()
        )

        viewModel.onTransportEvent(TransportEvent.Failed("network_down"))

        assertTrue(viewModel.uiState.value.fallbackActive)
        assertEquals(ConnectionPhase.FallbackActive, viewModel.uiState.value.session.connectionPhase)
        assertEquals("fallback_activated", viewModel.uiState.value.session.lastError?.code)
        assertEquals("fallback_activated", transport.lastDisconnectReason)
    }

    @Test
    fun policyFailureFrameTriggersFallbackWhenPreferred() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(5_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(preferWebFallbackOnPolicyFailure = true)
        }

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.PolicyFailure,
                    code = "policy_reject",
                    message = "blocked by policy"
                )
            )
        )

        assertTrue(viewModel.uiState.value.fallbackActive)
        assertEquals(ConnectionPhase.FallbackActive, viewModel.uiState.value.session.connectionPhase)
        assertEquals("fallback_activated", viewModel.uiState.value.session.lastError?.code)
        assertEquals("fallback_activated", transport.lastDisconnectReason)
    }

    @Test
    fun policyFailureAndFallbackTelemetryEventsAreRecorded() {
        val transport = FakeTransport()
        val telemetry = FakeTelemetrySink()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            telemetrySink = telemetry,
            reconnectBackoffMs = listOf(5_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(preferWebFallbackOnPolicyFailure = true)
        }

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.PolicyFailure,
                    code = "policy_reject",
                    message = "blocked by policy"
                )
            )
        )

        val policyEvent = telemetry.events.firstOrNull { event ->
            event.category == MeetingTelemetryCategory.PolicyFailure &&
                event.attributes["code"] == "policy_reject"
        }
        assertTrue(policyEvent != null)

        val fallbackEvent = telemetry.events.firstOrNull { event ->
            event.category == MeetingTelemetryCategory.FallbackLifecycle &&
                event.name == "fallback_activated"
        }
        assertEquals("blocked by policy", fallbackEvent?.attributes?.get("reason"))
    }

    @Test
    fun fallbackRecoveryTelemetryContainsRto() {
        val transport = FakeTransport()
        val telemetry = FakeTelemetrySink()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            telemetrySink = telemetry,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(preferWebFallbackOnPolicyFailure = true)
        }

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.PolicyFailure,
                    code = "policy_reject",
                    message = "blocked by policy"
                )
            )
        )
        viewModel.recoverFromFallback()

        val recoveredEvent = telemetry.events.firstOrNull { event ->
            event.category == MeetingTelemetryCategory.FallbackLifecycle &&
                event.name == "fallback_recovered"
        }
        assertTrue(recoveredEvent != null)
        val rto = recoveredEvent?.attributes?.get("rto_ms")?.toLongOrNull()
        assertTrue(rto != null)
        assertTrue((rto ?: -1L) >= 0L)
    }

    @Test
    fun restoredFallbackStateStartsInFallbackPhase() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(
                mapOf(
                    "session_fallback_active" to true,
                    "session_fallback_reason" to "policy_failure"
                )
            ),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        assertTrue(viewModel.uiState.value.fallbackActive)
        assertEquals(ConnectionPhase.FallbackActive, viewModel.uiState.value.session.connectionPhase)
        assertEquals(ConnectionPhase.FallbackActive.label, viewModel.uiState.value.transportState)
        assertEquals("policy_failure", viewModel.uiState.value.session.fallback.reason)
    }

    @Test
    fun restoredResumeTokenIsSentInHandshake() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(
                mapOf(
                    "session_resume_token" to "resume-xyz"
                )
            ),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.connect()
        assertEquals(1, transport.connectCount)

        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals("resume-xyz", handshake?.resumeToken)
    }

    @Test
    fun restoredWalletIdentityIsSentInHandshake() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(
                mapOf(
                    "cfg_wallet_identity" to "nexus://wallet/tester"
                )
            ),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.connect()
        assertEquals(1, transport.connectCount)

        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals("nexus://wallet/tester", handshake?.walletIdentity)
    }

    @Test
    fun explicitParticipantIdIsUsedInHandshake() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                participant = "Alice Runner",
                participantId = "Primary Host"
            )
        }
        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals("primary-host", handshake?.participantId)
        assertEquals("Alice Runner", handshake?.participantName)
    }

    @Test
    fun explicitParticipantIdFallsBackToParticipantWhenNormalizedValueIsEmpty() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                participant = "Alice Runner",
                participantId = "###@@@"
            )
        }
        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals("participant", handshake?.participantId)
        assertEquals("Alice Runner", handshake?.participantName)
    }

    @Test
    fun handshakeFallsBackToParticipantNameWhenParticipantIdMissing() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                participant = "Alice Runner",
                participantId = null
            )
        }
        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals("alice-runner", handshake?.participantId)
        assertEquals("Alice Runner", handshake?.participantName)
    }

    @Test
    fun hdrCapabilityOverridesAreAppliedToHandshakeAndCapabilityFrames() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                supportsHdrCapture = true,
                supportsHdrRender = false
            )
        }
        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)

        val handshake = transport.sentFrames.filterIsInstance<ProtocolFrame.Handshake>().firstOrNull()
        assertEquals(MediaProfile.SDR, handshake?.preferredProfile)
        assertEquals(true, handshake?.hdrCapture)
        assertEquals(false, handshake?.hdrRender)

        val capability = transport.sentFrames.filterIsInstance<ProtocolFrame.DeviceCapability>().firstOrNull()
        assertEquals(true, capability?.hdrCapture)
        assertEquals(false, capability?.hdrRender)
    }

    @Test
    fun connectedEventSendsHandshakeCapabilityAndPaymentPolicyFrames() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(requirePaymentSettlement = true)
        }
        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)

        val kinds = transport.sentFrames.map { frame -> frame.kind }
        assertEquals(listOf("handshake", "deviceCapability", "paymentPolicy"), kinds)
    }

    @Test
    fun pingFrameTriggersPongResponse() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.onTransportEvent(TransportEvent.FrameReceived(ProtocolFrame.Ping(123L)))

        val lastFrame = transport.sentFrames.lastOrNull()
        assertTrue(lastFrame is ProtocolFrame.Pong)
    }

    @Test
    fun e2eeEpochFrameTriggersKeyRotationAck() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "host",
                    epoch = 3,
                    publicKey = "pk-3",
                    signature = "sig-3",
                    issuedAtMs = 300L
                )
            )
        )

        val lastFrame = transport.sentFrames.lastOrNull()
        assertTrue(lastFrame is ProtocolFrame.KeyRotationAck)
        assertEquals(3, (lastFrame as ProtocolFrame.KeyRotationAck).ackEpoch)
        assertEquals(3, viewModel.uiState.value.session.e2eeState.lastAckEpoch)
    }

    @Test
    fun unsignedE2eeEpochDoesNotTriggerAckWhenSignaturesRequired() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                requireSignedModeration = true,
                preferWebFallbackOnPolicyFailure = false
            )
        }

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "host",
                    epoch = 3,
                    publicKey = "pk-3",
                    signature = "sig-3",
                    issuedAtMs = 300L
                )
            )
        )
        assertTrue(transport.sentFrames.lastOrNull() is ProtocolFrame.KeyRotationAck)
        assertEquals(3, viewModel.uiState.value.session.e2eeState.lastAckEpoch)

        transport.sentFrames.clear()
        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "host",
                    epoch = 2,
                    publicKey = "pk-2",
                    signature = "",
                    issuedAtMs = 400L
                )
            )
        )

        assertTrue(transport.sentFrames.isEmpty())
        assertEquals(3, viewModel.uiState.value.session.e2eeState.lastAckEpoch)
        assertEquals("e2ee_signature_missing", viewModel.uiState.value.session.lastError?.code)
    }

    @Test
    fun keyRotationAckUsesResolvedParticipantIdWhenParticipantIdMissing() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                participant = "Alice Runner",
                participantId = null
            )
        }

        viewModel.onTransportEvent(
            TransportEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "host",
                    epoch = 3,
                    publicKey = "pk-3",
                    signature = "sig-3",
                    issuedAtMs = 300L
                )
            )
        )

        val ack = transport.sentFrames.lastOrNull() as? ProtocolFrame.KeyRotationAck
        assertEquals("alice-runner", ack?.participantId)
        assertEquals(3, ack?.ackEpoch)
    }

    @Test
    fun moderationCommandRequiresHostRoleEvenWhenSignaturesOptional() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.updateConfig { current ->
            current.copy(
                requireSignedModeration = false,
                preferWebFallbackOnPolicyFailure = false
            )
        }

        viewModel.dispatch(
            SessionCommand.Moderate(
                action = ModerationAction.Mute,
                targetParticipantId = "target"
            )
        )

        assertEquals("moderation_not_authorized", viewModel.uiState.value.session.lastError?.code)
        assertTrue(transport.sentFrames.none { frame -> frame is ProtocolFrame.ModerationSigned })
    }

    @Test
    fun backgroundSuspendsReconnectUntilForegrounded() {
        val transport = FakeTransport()
        val telemetry = FakeTelemetrySink()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            telemetrySink = telemetry,
            reconnectBackoffMs = listOf(10_000L)
        )

        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)
        assertEquals(1, transport.connectCount)

        viewModel.onAppBackgrounded()
        assertEquals("app_backgrounded", transport.lastDisconnectReason)
        assertEquals(ConnectionPhase.Degraded, viewModel.uiState.value.session.connectionPhase)

        viewModel.onTransportEvent(TransportEvent.Disconnected("app_backgrounded"))
        assertEquals(1, transport.connectCount)

        viewModel.onAppForegrounded()
        assertEquals(2, transport.connectCount)

        val backgroundEvent = telemetry.events.firstOrNull {
            it.category == MeetingTelemetryCategory.ConnectionLifecycle &&
                it.name == "app_backgrounded"
        }
        val foregroundEvent = telemetry.events.firstOrNull {
            it.category == MeetingTelemetryCategory.ConnectionLifecycle &&
                it.name == "app_foregrounded"
        }
        assertTrue(backgroundEvent != null)
        assertTrue(foregroundEvent != null)
    }

    @Test
    fun connectivityRestoreDefersWhileBackgrounded() {
        val transport = FakeTransport()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.onAppBackgrounded()
        viewModel.onConnectivityChanged(available = true)
        assertEquals(0, transport.connectCount)

        viewModel.onAppForegrounded()
        assertEquals(1, transport.connectCount)
    }

    @Test
    fun audioInterruptionHooksEmitTelemetryAndReconnect() {
        val transport = FakeTransport()
        val telemetry = FakeTelemetrySink()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = transport,
            telemetrySink = telemetry,
            reconnectBackoffMs = listOf(60_000L)
        )

        viewModel.connect()
        viewModel.onTransportEvent(TransportEvent.Connected)
        assertEquals(1, transport.connectCount)

        viewModel.onAudioInterruptionBegan()
        assertEquals(ConnectionPhase.Degraded, viewModel.uiState.value.session.connectionPhase)

        viewModel.onAudioInterruptionEnded(shouldReconnect = true)
        assertEquals(2, transport.connectCount)

        val interruptionBegan = telemetry.events.firstOrNull {
            it.category == MeetingTelemetryCategory.ConnectionLifecycle &&
                it.name == "audio_interruption_began"
        }
        val interruptionEnded = telemetry.events.firstOrNull {
            it.category == MeetingTelemetryCategory.ConnectionLifecycle &&
                it.name == "audio_interruption_ended"
        }
        assertTrue(interruptionBegan != null)
        assertEquals("true", interruptionEnded?.attributes?.get("should_reconnect"))
    }

    @Test
    fun audioRouteChangeEmitsTelemetry() {
        val telemetry = FakeTelemetrySink()
        val viewModel = MeetingViewModel(
            savedStateHandle = SavedStateHandle(),
            transport = FakeTransport(),
            telemetrySink = telemetry,
            reconnectBackoffMs = listOf(1_000L)
        )

        viewModel.onAudioRouteChanged("becoming_noisy")

        val routeChanged = telemetry.events.firstOrNull {
            it.category == MeetingTelemetryCategory.ConnectionLifecycle &&
                it.name == "audio_route_changed"
        }
        assertEquals("becoming_noisy", routeChanged?.attributes?.get("reason"))
    }

    private class FakeTransport : ProtocolTransport {
        var lastDisconnectReason: String? = null
        var connectCount: Int = 0
        val sentFrames = mutableListOf<ProtocolFrame>()

        override fun connect(config: MeetingConfig, listener: ProtocolTransport.Listener) {
            connectCount += 1
        }

        override fun disconnect(reason: String) {
            lastDisconnectReason = reason
        }

        override fun send(frame: ProtocolFrame): Boolean {
            sentFrames += frame
            return true
        }

        override fun shutdown() = Unit
    }

    private class FakeTelemetrySink : MeetingTelemetrySink {
        val events = mutableListOf<MeetingTelemetryEvent>()

        override fun record(event: MeetingTelemetryEvent) {
            events += event
        }
    }
}
