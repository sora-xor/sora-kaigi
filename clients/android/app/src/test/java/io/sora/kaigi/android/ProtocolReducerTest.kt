package io.sora.kaigi.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ProtocolReducerTest {
    private val reducer = DefaultProtocolReducer()

    @Test
    fun handshakeAckTransitionsToConnected() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(state, ProtocolEvent.ConnectRequested, nowMs = 0)
        state = reducer.reduce(state, ProtocolEvent.TransportConnected, nowMs = 1)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.HandshakeAck(
                    sessionId = "s1",
                    resumeToken = "resume-1",
                    acceptedAtMs = 2
                )
            ),
            nowMs = 2
        )

        assertTrue(state.handshakeComplete)
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
        assertEquals("resume-1", state.resumeToken)
    }

    @Test
    fun presenceSequenceIsMonotonic() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        val participant = Participant(
            id = "p1",
            displayName = "Alice",
            role = ParticipantRole.Participant,
            muted = false,
            videoEnabled = true,
            shareEnabled = false,
            waitingRoom = false
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PresenceDelta(
                    joined = listOf(participant),
                    left = emptyList(),
                    roleChanges = emptyList(),
                    sequence = 7
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PresenceDelta(
                    joined = emptyList(),
                    left = listOf("p1"),
                    roleChanges = emptyList(),
                    sequence = 6
                )
            )
        )

        assertEquals(7L, state.presenceSequence)
        assertNotNull(state.participants["p1"])
    }

    @Test
    fun permissionsSnapshotEpochIsMonotonic() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PermissionsSnapshot(
                    participantId = "alice",
                    effectivePermissions = listOf("moderate", "share"),
                    epoch = 3
                )
            ),
            nowMs = 1
        )
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PermissionsSnapshot(
                    participantId = "alice",
                    effectivePermissions = listOf("view"),
                    epoch = 2
                )
            ),
            nowMs = 2
        )

        assertEquals(3, state.permissionSnapshots["alice"]?.epoch)
        assertEquals(listOf("moderate", "share"), state.permissionSnapshots["alice"]?.effectivePermissions)
    }

    @Test
    fun fallbackRecoveryCapturesRto() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(state, ProtocolEvent.FallbackActivated("degraded"), nowMs = 10_000)
        state = reducer.reduce(state, ProtocolEvent.FallbackRecovered, nowMs = 13_250)

        assertFalse(state.fallback.active)
        assertEquals(3_250L, state.fallback.lastRtoMs)
    }

    @Test
    fun policyErrorMovesToErrorPhase() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.PolicyFailure,
                    code = "policy_reject",
                    message = "payment required"
                )
            ),
            nowMs = 100
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("policy_reject", state.lastError?.code)
    }

    @Test
    fun mediaProfileDowngradeMovesToDegradedAndRecoversToConnected() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.MediaProfileNegotiation(
                    participantId = "alice",
                    preferredProfile = MediaProfile.HDR,
                    negotiatedProfile = MediaProfile.SDR,
                    colorPrimaries = "bt2020",
                    transferFunction = "pq",
                    codec = "h265",
                    epoch = 4
                )
            ),
            nowMs = 4
        )
        assertEquals(ConnectionPhase.Degraded, state.connectionPhase)
        assertEquals(MediaProfile.SDR, state.mediaProfile.negotiatedProfile)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.MediaProfileNegotiation(
                    participantId = "alice",
                    preferredProfile = MediaProfile.SDR,
                    negotiatedProfile = MediaProfile.SDR,
                    colorPrimaries = "bt709",
                    transferFunction = "gamma",
                    codec = "h264",
                    epoch = 5
                )
            ),
            nowMs = 5
        )
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
        assertEquals(MediaProfile.SDR, state.mediaProfile.preferredProfile)
    }

    @Test
    fun rejectsUnauthorizedModerationIssuer() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = state.copy(
            participants = mapOf(
                "target" to Participant(
                    id = "target",
                    displayName = "Target",
                    role = ParticipantRole.Participant,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.ModerationSigned(
                    sentAtMs = 1,
                    targetParticipantId = "target",
                    action = ModerationAction.Mute,
                    issuedBy = "unknown",
                    signature = "sig-1"
                )
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("moderation_not_authorized", state.lastError?.code)
        assertFalse(state.participants["target"]?.muted ?: true)
    }

    @Test
    fun rejectsUnauthorizedModerationIssuerEvenWhenSignaturesAreOptional() {
        var state = ProtocolSessionState.initial(
            MeetingConfig(requireSignedModeration = false)
        )
        state = state.copy(
            participants = mapOf(
                "target" to Participant(
                    id = "target",
                    displayName = "Target",
                    role = ParticipantRole.Participant,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.ModerationSigned(
                    sentAtMs = 1,
                    targetParticipantId = "target",
                    action = ModerationAction.Mute,
                    issuedBy = "unknown",
                    signature = ""
                )
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("moderation_not_authorized", state.lastError?.code)
        assertFalse(state.participants["target"]?.muted ?: true)
    }

    @Test
    fun authorizedModerationMuteUpdatesTargetParticipant() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                ),
                "target" to Participant(
                    id = "target",
                    displayName = "Target",
                    role = ParticipantRole.Participant,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.ModerationSigned(
                    sentAtMs = 1,
                    targetParticipantId = "target",
                    action = ModerationAction.Mute,
                    issuedBy = "host",
                    signature = "sig-2"
                )
            ),
            nowMs = 2
        )

        assertEquals(true, state.participants["target"]?.muted)
        assertEquals(null, state.lastError)
    }

    @Test
    fun unsignedModerationIsRejectedWhenSignedModerationRequired() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                ),
                "target" to Participant(
                    id = "target",
                    displayName = "Target",
                    role = ParticipantRole.Participant,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.ModerationSigned(
                    sentAtMs = 1,
                    targetParticipantId = "target",
                    action = ModerationAction.Mute,
                    issuedBy = "host",
                    signature = ""
                )
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("moderation_signature_missing", state.lastError?.code)
        assertEquals(false, state.participants["target"]?.muted)
    }

    @Test
    fun unsignedRoleGrantIsRejectedWhenSignedModerationRequired() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                ),
                "target" to Participant(
                    id = "target",
                    displayName = "Target",
                    role = ParticipantRole.Participant,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.RoleGrant(
                    targetParticipantId = "target",
                    role = ParticipantRole.CoHost,
                    grantedBy = "host",
                    signature = "",
                    issuedAtMs = 10
                )
            ),
            nowMs = 11
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("role_grant_signature_missing", state.lastError?.code)
        assertEquals(ParticipantRole.Participant, state.participants["target"]?.role)
    }

    @Test
    fun unsignedSessionPolicyIsRejectedWhenSignedModerationRequired() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.SessionPolicy(
                    roomLock = true,
                    waitingRoomEnabled = true,
                    recordingPolicy = RecordingState.Started,
                    guestPolicy = GuestPolicy.InviteOnly,
                    e2eeRequired = true,
                    maxParticipants = 300,
                    policyEpoch = 4,
                    updatedBy = "host",
                    signature = ""
                )
            ),
            nowMs = 12
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("session_policy_signature_missing", state.lastError?.code)
        assertFalse(state.roomLocked)
    }

    @Test
    fun sessionPolicyUpdatesMetadataAndIgnoresStaleEpoch() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.SessionPolicy(
                    roomLock = true,
                    waitingRoomEnabled = true,
                    recordingPolicy = RecordingState.Started,
                    guestPolicy = GuestPolicy.InviteOnly,
                    e2eeRequired = false,
                    maxParticipants = 120,
                    policyEpoch = 6,
                    updatedBy = "host",
                    signature = "sig-6"
                )
            ),
            nowMs = 100
        )

        assertTrue(state.roomLocked)
        assertTrue(state.waitingRoomEnabled)
        assertEquals(GuestPolicy.InviteOnly, state.guestPolicy)
        assertFalse(state.e2eeRequired)
        assertEquals(120, state.maxParticipants)
        assertEquals(6, state.policyEpoch)
        assertEquals(RecordingState.Started, state.recordingNotice)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.SessionPolicy(
                    roomLock = false,
                    waitingRoomEnabled = false,
                    recordingPolicy = RecordingState.Stopped,
                    guestPolicy = GuestPolicy.Open,
                    e2eeRequired = true,
                    maxParticipants = 400,
                    policyEpoch = 5,
                    updatedBy = "host",
                    signature = "sig-5"
                )
            ),
            nowMs = 101
        )

        assertTrue(state.roomLocked)
        assertTrue(state.waitingRoomEnabled)
        assertEquals(GuestPolicy.InviteOnly, state.guestPolicy)
        assertFalse(state.e2eeRequired)
        assertEquals(120, state.maxParticipants)
        assertEquals(6, state.policyEpoch)
        assertEquals(RecordingState.Started, state.recordingNotice)
    }

    @Test
    fun e2eeRequiredPolicyRejectsUntilEpochPublishedThenClears() {
        var state = ProtocolSessionState.initial(MeetingConfig()).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected,
            participants = mapOf(
                "host" to Participant(
                    id = "host",
                    displayName = "Host",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = true,
                    waitingRoom = false
                )
            )
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.SessionPolicy(
                    roomLock = false,
                    waitingRoomEnabled = false,
                    recordingPolicy = RecordingState.Stopped,
                    guestPolicy = GuestPolicy.Open,
                    e2eeRequired = true,
                    maxParticipants = 300,
                    policyEpoch = 10,
                    updatedBy = "host",
                    signature = "sig-policy-10"
                )
            ),
            nowMs = 200
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("e2ee_epoch_required", state.lastError?.code)
        assertEquals(0, state.e2eeState.currentEpoch)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "host",
                    epoch = 1,
                    publicKey = "pk-host-1",
                    signature = "sig-epoch-1",
                    issuedAtMs = 201
                )
            ),
            nowMs = 201
        )

        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
        assertEquals(1, state.e2eeState.currentEpoch)
        assertEquals(null, state.lastError)
    }

    @Test
    fun unsignedE2eeEpochIsRejectedWhenSignedModerationRequired() {
        var state = ProtocolSessionState.initial(MeetingConfig())

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "alice",
                    epoch = 2,
                    publicKey = "pk-2",
                    signature = "",
                    issuedAtMs = 20
                )
            ),
            nowMs = 20
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("e2ee_signature_missing", state.lastError?.code)
        assertEquals(0, state.e2eeState.currentEpoch)
    }

    @Test
    fun recordingNoticeFrameUpdatesSessionState() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.RecordingNotice(
                    participantId = "system",
                    state = RecordingState.Started,
                    mode = "room",
                    policyBasis = "recording_required",
                    issuedAtMs = 10,
                    issuedBy = "system"
                )
            ),
            nowMs = 10
        )

        assertEquals(RecordingState.Started, state.recordingNotice)
    }

    @Test
    fun e2eeEpochAndAckAreMonotonic() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "alice",
                    epoch = 7,
                    publicKey = "key-7",
                    signature = "sig-7",
                    issuedAtMs = 1
                )
            ),
            nowMs = 1
        )
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.E2eeKeyEpoch(
                    participantId = "alice",
                    epoch = 5,
                    publicKey = "key-5",
                    signature = "sig-5",
                    issuedAtMs = 2
                )
            ),
            nowMs = 2
        )
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.KeyRotationAck(
                    participantId = "alice",
                    ackEpoch = 6,
                    receivedAtMs = 3
                )
            ),
            nowMs = 3
        )
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.KeyRotationAck(
                    participantId = "alice",
                    ackEpoch = 4,
                    receivedAtMs = 4
                )
            ),
            nowMs = 4
        )

        assertEquals(7, state.e2eeState.currentEpoch)
        assertEquals(6, state.e2eeState.lastAckEpoch)
    }

    @Test
    fun protocolAndTransportErrorsMapToDegradedPhase() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.ProtocolFailure,
                    code = "decode_failed",
                    message = "malformed frame"
                )
            ),
            nowMs = 1
        )
        assertEquals(ConnectionPhase.Degraded, state.connectionPhase)

        state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.TransportFailure,
                    code = "socket_reset",
                    message = "connection reset"
                )
            ),
            nowMs = 2
        )
        assertEquals(ConnectionPhase.Degraded, state.connectionPhase)
    }

    @Test
    fun fallbackStaysActiveOnTransportEvents() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(state, ProtocolEvent.FallbackActivated("native_degraded"), nowMs = 1)
        state = reducer.reduce(state, ProtocolEvent.TransportDisconnected("fallback_activated"), nowMs = 2)
        state = reducer.reduce(state, ProtocolEvent.TransportFailure("socket_closed"), nowMs = 3)

        assertTrue(state.fallback.active)
        assertEquals(ConnectionPhase.FallbackActive, state.connectionPhase)
        assertEquals("fallback_activated", state.lastError?.code)
    }

    @Test
    fun paymentSettlementIsRequiredWhenConfigured() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("payment_settlement_required", state.lastError?.code)
        assertEquals(PaymentSettlementStatus.Pending, state.paymentState.settlementStatus)
    }

    @Test
    fun blockedPaymentSettlementIsPolicyFailure() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.Blocked)
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("payment_settlement_blocked", state.lastError?.code)
        assertEquals(PaymentSettlementStatus.Blocked, state.paymentState.settlementStatus)
    }

    @Test
    fun configUpdateCanEnforcePaymentSettlement() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )

        assertEquals(null, state.lastError)

        state = reducer.reduce(
            state,
            ProtocolEvent.ConfigUpdated(
                state.config.copy(requirePaymentSettlement = true)
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.Error, state.connectionPhase)
        assertEquals("payment_settlement_required", state.lastError?.code)
    }

    @Test
    fun configUpdateCanClearPaymentSettlementRequirement() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected
        )
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )
        assertEquals("payment_settlement_required", state.lastError?.code)
        assertEquals(ConnectionPhase.Error, state.connectionPhase)

        state = reducer.reduce(
            state,
            ProtocolEvent.ConfigUpdated(
                state.config.copy(requirePaymentSettlement = false)
            ),
            nowMs = 2
        )

        assertEquals(null, state.lastError)
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
    }

    @Test
    fun settledPaymentClearsPaymentPolicyError() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )
        assertEquals("payment_settlement_required", state.lastError?.code)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.Settled)
            ),
            nowMs = 2
        )

        assertEquals(null, state.lastError)
        assertEquals(PaymentSettlementStatus.Settled, state.paymentState.settlementStatus)
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
    }

    @Test
    fun notRequiredPaymentSettlementClearsPaymentPolicyError() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )
        assertEquals("payment_settlement_required", state.lastError?.code)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.NotRequired)
            ),
            nowMs = 2
        )

        assertEquals(null, state.lastError)
        assertEquals(PaymentSettlementStatus.NotRequired, state.paymentState.settlementStatus)
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
    }

    @Test
    fun paymentPolicyDisableClearsPaymentPolicyError() {
        val config = MeetingConfig(requirePaymentSettlement = true)
        var state = ProtocolSessionState.initial(config).copy(
            handshakeComplete = true,
            connectionPhase = ConnectionPhase.Connected
        )

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = true,
                    destinationAccount = "nexus://dest"
                )
            ),
            nowMs = 1
        )
        assertEquals("payment_settlement_required", state.lastError?.code)
        assertEquals(ConnectionPhase.Error, state.connectionPhase)

        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.PaymentPolicy(
                    required = false,
                    destinationAccount = null
                )
            ),
            nowMs = 2
        )

        assertEquals(null, state.lastError)
        assertEquals(PaymentSettlementStatus.NotRequired, state.paymentState.settlementStatus)
        assertEquals(ConnectionPhase.Connected, state.connectionPhase)
    }

    @Test
    fun fallbackStaysActiveOnProtocolErrorFrame() {
        var state = ProtocolSessionState.initial(MeetingConfig())
        state = reducer.reduce(state, ProtocolEvent.FallbackActivated("native_degraded"), nowMs = 1)
        state = reducer.reduce(
            state,
            ProtocolEvent.FrameReceived(
                ProtocolFrame.Error(
                    category = SessionErrorCategory.ProtocolFailure,
                    code = "parse_error",
                    message = "decode failed"
                )
            ),
            nowMs = 2
        )

        assertEquals(ConnectionPhase.FallbackActive, state.connectionPhase)
    }
}
