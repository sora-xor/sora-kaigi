import XCTest
@testable import KaigiIOS

final class KaigiIOSTests: XCTestCase {
    func testMeetingConfigRejectsInvalidScheme() {
        let config = MeetingConfig(
            signalingURLText: "ftp://example.com",
            fallbackURLText: "https://example.com",
            roomID: "room-a",
            participantName: "Alice"
        )
        XCTAssertNil(config.signalingURL)
        XCTAssertFalse(config.isJoinable)
    }

    func testProtocolFrameRoundTripHandshake() throws {
        let original = MeetingProtocolFrame.handshake(
            HandshakeFrame(
                roomID: "room-a",
                participantID: "alice",
                participantName: "Alice",
                walletIdentity: "nexus://wallet/alice",
                resumeToken: "resume-1",
                preferredProfile: .hdr,
                hdrCapture: true,
                hdrRender: true,
                sentAtMs: 100
            )
        )

        let encoded = try MeetingProtocolCodec.encode(original)
        let decoded = try MeetingProtocolCodec.decode(encoded)
        XCTAssertEqual(decoded, original)
    }

    func testProtocolCodecDecodesSnakeCaseHandshakeAck() throws {
        let raw = """
        {"kind":"handshake_ack","handshake_ack":{"session_id":"s1","resume_token":"resume-2","accepted_at_ms":1700000000000}}
        """

        let frame = try MeetingProtocolCodec.decode(raw)
        XCTAssertEqual(frame.kind, .handshakeAck)
        XCTAssertEqual(frame.handshakeAck?.sessionID, "s1")
        XCTAssertEqual(frame.handshakeAck?.resumeToken, "resume-2")
        XCTAssertEqual(frame.handshakeAck?.acceptedAtMs, 1_700_000_000_000)
    }

    func testProtocolCodecDecodesPresenceDeltaPayloadAliases() throws {
        let raw = """
        {"kind":"participant_presence_delta","participant_presence_delta":{"joined":[{"id":"alice","display_name":"Alice","role":"coHost","muted":false,"video_enabled":true,"share_enabled":true,"waiting_room":true}],"left":["bob"],"role_changes":[{"participant_id":"alice","role":"co_host"}],"sequence":9}}
        """

        let frame = try MeetingProtocolCodec.decode(raw)
        XCTAssertEqual(frame.kind, .participantPresenceDelta)
        XCTAssertEqual(frame.presenceDelta?.sequence, 9)
        XCTAssertEqual(frame.presenceDelta?.joined.first?.displayName, "Alice")
        XCTAssertEqual(frame.presenceDelta?.joined.first?.role, .coHost)
        XCTAssertEqual(frame.presenceDelta?.joined.first?.waitingRoom, true)
        XCTAssertEqual(frame.presenceDelta?.roleChanges.first?.participantID, "alice")
        XCTAssertEqual(frame.presenceDelta?.roleChanges.first?.role, .coHost)
    }

    func testProtocolCodecDecodesSessionPolicyAliases() throws {
        let raw = """
        {"kind":"session_policy","session_policy":{"room_lock":true,"waiting_room_enabled":true,"recording_policy":"started","guest_policy":"inviteOnly","e2ee_required":true,"max_participants":300,"policy_epoch":8,"updated_by":"host","signature":"sig-policy"}}
        """

        let frame = try MeetingProtocolCodec.decode(raw)
        XCTAssertEqual(frame.kind, .sessionPolicy)
        XCTAssertEqual(frame.sessionPolicy?.roomLock, true)
        XCTAssertEqual(frame.sessionPolicy?.waitingRoomEnabled, true)
        XCTAssertEqual(frame.sessionPolicy?.recordingPolicy, .started)
        XCTAssertEqual(frame.sessionPolicy?.guestPolicy, .inviteOnly)
        XCTAssertEqual(frame.sessionPolicy?.maxParticipants, 300)
        XCTAssertEqual(frame.sessionPolicy?.policyEpoch, 8)
        XCTAssertEqual(frame.sessionPolicy?.updatedBy, "host")
        XCTAssertEqual(frame.sessionPolicy?.signature, "sig-policy")
    }

    func testProtocolCodecDecodesPolicyFailureErrorAlias() throws {
        let raw = """
        {"kind":"error","error":{"category":"policy_failure","code":"policy_reject","message":"blocked"}}
        """

        let frame = try MeetingProtocolCodec.decode(raw)
        XCTAssertEqual(frame.kind, .error)
        XCTAssertEqual(frame.error?.category, .policyFailure)
        XCTAssertEqual(frame.error?.code, "policy_reject")
    }

    func testProtocolCodecDecodesNotRequiredPaymentSettlementAlias() throws {
        let raw = """
        {"kind":"payment_settlement","payment_settlement":{"status":"not_required"}}
        """

        let frame = try MeetingProtocolCodec.decode(raw)
        XCTAssertEqual(frame.kind, .paymentSettlement)
        XCTAssertEqual(frame.paymentSettlement?.status, .notRequired)
    }

    func testProtocolCodecRejectsLegacyRawJoinFrame() {
        XCTAssertThrowsError(try MeetingProtocolCodec.decode("JOIN room=daily participant=Alice"))
    }

    func testReducerMarksConnectedOnHandshakeAck() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(state: state, event: .connectRequested, now: Date(timeIntervalSince1970: 0))
        state = reducer.reduce(state: state, event: .transportConnected, now: Date(timeIntervalSince1970: 1))

        let ack = MeetingProtocolFrame.handshakeAck(
            HandshakeAckFrame(sessionID: "s1", resumeToken: "resume-2", acceptedAtMs: 2_000)
        )

        state = reducer.reduce(state: state, event: .frameReceived(ack), now: Date(timeIntervalSince1970: 2))

        XCTAssertTrue(state.handshakeComplete)
        XCTAssertEqual(state.resumeToken, "resume-2")
        XCTAssertEqual(state.connectionState, .connected)
    }

    func testReducerRejectsUnauthorizedModerationIssuer() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["target"] = MeetingParticipant(
            id: "target",
            displayName: "Target",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        let moderation = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: 1,
                targetParticipantID: "target",
                action: .mute,
                issuedBy: "unknown",
                signature: "sig-1"
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(moderation), now: Date(timeIntervalSince1970: 2))
        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "moderation_not_authorized")
        XCTAssertEqual(state.participants["target"]?.muted, false)
    }

    func testReducerRejectsUnauthorizedModerationIssuerWhenSignaturesOptional() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requireSignedModeration = false
        config.preferWebFallbackOnPolicyFailure = false

        var state = MeetingSessionState.initial(config: config)
        state.participants["target"] = MeetingParticipant(
            id: "target",
            displayName: "Target",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        let moderation = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: 1,
                targetParticipantID: "target",
                action: .mute,
                issuedBy: "unknown",
                signature: ""
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(moderation), now: Date(timeIntervalSince1970: 2))
        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "moderation_not_authorized")
        XCTAssertEqual(state.participants["target"]?.muted, false)
    }

    func testReducerAppliesAuthorizedModerationMute() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )
        state.participants["target"] = MeetingParticipant(
            id: "target",
            displayName: "Target",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        let moderation = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: 1,
                targetParticipantID: "target",
                action: .mute,
                issuedBy: "host",
                signature: "sig-2"
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(moderation), now: Date(timeIntervalSince1970: 2))
        XCTAssertEqual(state.participants["target"]?.muted, true)
        XCTAssertNil(state.lastError)
    }

    func testReducerUpdatesRecordingNoticeState() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        let notice = MeetingProtocolFrame(
            kind: .recordingNotice,
            recordingNotice: RecordingNoticeFrame(
                participantID: "system",
                state: .started,
                mode: "room",
                policyBasis: "recording_required",
                issuedAtMs: 10,
                issuedBy: "system"
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(notice), now: Date(timeIntervalSince1970: 1))
        XCTAssertEqual(state.recordingNotice.state, .started)
        XCTAssertEqual(state.recordingNotice.policyBasis, "recording_required")
    }

    func testReducerRejectsUnsignedModerationWhenRequired() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )
        state.participants["target"] = MeetingParticipant(
            id: "target",
            displayName: "Target",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        let moderation = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: 1,
                targetParticipantID: "target",
                action: .mute,
                issuedBy: "host",
                signature: ""
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(moderation), now: Date(timeIntervalSince1970: 2))
        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "moderation_signature_missing")
        XCTAssertEqual(state.participants["target"]?.muted, false)
    }

    func testReducerRejectsUnsignedRoleGrantWhenRequired() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )
        state.participants["target"] = MeetingParticipant(
            id: "target",
            displayName: "Target",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .roleGrant,
                    roleGrant: RoleGrantFrame(
                        targetParticipantID: "target",
                        role: .coHost,
                        grantedBy: "host",
                        signature: "",
                        issuedAtMs: 10
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "role_grant_signature_missing")
        XCTAssertEqual(state.participants["target"]?.role, .participant)
    }

    func testReducerRejectsUnsignedSessionPolicyWhenRequired() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .sessionPolicy,
                    sessionPolicy: SessionPolicyFrame(
                        roomLock: true,
                        waitingRoomEnabled: true,
                        recordingPolicy: .started,
                        guestPolicy: .inviteOnly,
                        e2eeRequired: true,
                        maxParticipants: 200,
                        policyEpoch: 4,
                        updatedBy: "host",
                        signature: ""
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "session_policy_signature_missing")
        XCTAssertFalse(state.roomLocked)
    }

    func testReducerSessionPolicyUpdatesMetadataAndIgnoresStaleEpoch() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .sessionPolicy,
                    sessionPolicy: SessionPolicyFrame(
                        roomLock: true,
                        waitingRoomEnabled: true,
                        recordingPolicy: .started,
                        guestPolicy: .inviteOnly,
                        e2eeRequired: false,
                        maxParticipants: 120,
                        policyEpoch: 6,
                        updatedBy: "host",
                        signature: "sig-6"
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 10)
        )

        XCTAssertTrue(state.roomLocked)
        XCTAssertTrue(state.waitingRoomEnabled)
        XCTAssertEqual(state.guestPolicy, .inviteOnly)
        XCTAssertFalse(state.e2eeRequired)
        XCTAssertEqual(state.maxParticipants, 120)
        XCTAssertEqual(state.policyEpoch, 6)
        XCTAssertEqual(state.recordingNotice.state, .started)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .sessionPolicy,
                    sessionPolicy: SessionPolicyFrame(
                        roomLock: false,
                        waitingRoomEnabled: false,
                        recordingPolicy: .stopped,
                        guestPolicy: .open,
                        e2eeRequired: true,
                        maxParticipants: 400,
                        policyEpoch: 5,
                        updatedBy: "host",
                        signature: "sig-5"
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 11)
        )

        XCTAssertTrue(state.roomLocked)
        XCTAssertTrue(state.waitingRoomEnabled)
        XCTAssertEqual(state.guestPolicy, .inviteOnly)
        XCTAssertFalse(state.e2eeRequired)
        XCTAssertEqual(state.maxParticipants, 120)
        XCTAssertEqual(state.policyEpoch, 6)
        XCTAssertEqual(state.recordingNotice.state, .started)
    }

    func testReducerE2EERequiredPolicyRejectsUntilEpochPublishedThenClears() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.handshakeComplete = true
        state.connectionState = .connected
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .sessionPolicy,
                    sessionPolicy: SessionPolicyFrame(
                        roomLock: false,
                        waitingRoomEnabled: false,
                        recordingPolicy: .stopped,
                        guestPolicy: .open,
                        e2eeRequired: true,
                        maxParticipants: 300,
                        policyEpoch: 10,
                        updatedBy: "host",
                        signature: "sig-policy-10"
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 20)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "e2ee_epoch_required")
        XCTAssertEqual(state.e2ee.currentEpoch, 0)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "host",
                        epoch: 1,
                        publicKey: "pk-host-1",
                        signature: "sig-epoch-1",
                        issuedAtMs: 21
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 21)
        )

        XCTAssertEqual(state.connectionState, .connected)
        XCTAssertEqual(state.e2ee.currentEpoch, 1)
        XCTAssertNil(state.lastError)
    }

    func testReducerRejectsUnsignedE2EEEpochWhenRequired() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "alice",
                        epoch: 1,
                        publicKey: "pk-1",
                        signature: "",
                        issuedAtMs: 10
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "e2ee_signature_missing")
        XCTAssertEqual(state.e2ee.currentEpoch, 0)
    }

    func testReducerTracksE2EEEpochAndAckMonotonicity() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(participantID: "alice", epoch: 7, publicKey: "k7", signature: "sig-7", issuedAtMs: 1)
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(participantID: "alice", epoch: 5, publicKey: "k5", signature: "sig-5", issuedAtMs: 2)
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )
        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .keyRotationAck,
                    keyRotationAck: KeyRotationAckFrame(participantID: "alice", ackEpoch: 6, receivedAtMs: 3)
                )
            ),
            now: Date(timeIntervalSince1970: 3)
        )
        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .keyRotationAck,
                    keyRotationAck: KeyRotationAckFrame(participantID: "alice", ackEpoch: 4, receivedAtMs: 4)
                )
            ),
            now: Date(timeIntervalSince1970: 4)
        )

        XCTAssertEqual(state.e2ee.currentEpoch, 7)
        XCTAssertEqual(state.e2ee.lastAckEpoch, 6)
    }

    func testReducerAppliesPermissionsSnapshotMonotonically() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .permissionsSnapshot,
                    permissionsSnapshot: PermissionsSnapshotFrame(
                        participantID: "alice",
                        effectivePermissions: ["moderate", "share"],
                        epoch: 3
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .permissionsSnapshot,
                    permissionsSnapshot: PermissionsSnapshotFrame(
                        participantID: "alice",
                        effectivePermissions: ["view"],
                        epoch: 2
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.permissionSnapshots["alice"]?.epoch, 3)
        XCTAssertEqual(state.permissionSnapshots["alice"]?.effectivePermissions, ["moderate", "share"])
    }

    func testReducerMapsProtocolAndTransportErrorFramesToDegraded() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(.error(category: .protocolFailure, code: "decode_failed", message: "malformed frame")),
            now: Date(timeIntervalSince1970: 1)
        )
        XCTAssertEqual(state.connectionState, .degraded)

        state = MeetingSessionState.initial(config: .default)
        state = reducer.reduce(
            state: state,
            event: .frameReceived(.error(category: .transportFailure, code: "socket_reset", message: "connection reset")),
            now: Date(timeIntervalSince1970: 2)
        )
        XCTAssertEqual(state.connectionState, .degraded)
    }

    func testReducerKeepsFallbackActiveOnTransportEvents() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .fallbackActivated(reason: "native_degraded"),
            now: Date(timeIntervalSince1970: 1)
        )
        state = reducer.reduce(
            state: state,
            event: .transportDisconnected(reason: "fallback_activated"),
            now: Date(timeIntervalSince1970: 2)
        )
        state = reducer.reduce(
            state: state,
            event: .transportFailure(message: "socket_closed"),
            now: Date(timeIntervalSince1970: 3)
        )

        XCTAssertTrue(state.fallback.active)
        XCTAssertEqual(state.connectionState, .fallbackActive)
        XCTAssertEqual(state.lastError?.code, "fallback_activated")
    }

    func testReducerRequiresPaymentSettlementWhenConfigured() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")
        XCTAssertEqual(state.payment.settlementStatus, .pending)
    }

    func testReducerFlagsBlockedPaymentSettlement() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentSettlement,
                    paymentSettlement: PaymentSettlementFrame(status: .blocked)
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "payment_settlement_blocked")
        XCTAssertEqual(state.payment.settlementStatus, .blocked)
    }

    func testReducerConfigUpdateEnforcesPaymentSettlement() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )

        XCTAssertNil(state.lastError)

        var updated = state.config
        updated.requirePaymentSettlement = true
        state = reducer.reduce(
            state: state,
            event: .configUpdated(updated),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.connectionState, .error)
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")
    }

    func testReducerConfigUpdateCanClearPaymentSettlementRequirement() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)
        state.handshakeComplete = true
        state.connectionState = .connected

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")
        XCTAssertEqual(state.connectionState, .error)

        var updated = state.config
        updated.requirePaymentSettlement = false
        state = reducer.reduce(
            state: state,
            event: .configUpdated(updated),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertNil(state.lastError)
        XCTAssertEqual(state.connectionState, .connected)
    }

    func testReducerClearsPaymentErrorWhenSettlementSucceeds() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)
        state.handshakeComplete = true
        state.connectionState = .connected

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentSettlement,
                    paymentSettlement: PaymentSettlementFrame(status: .settled)
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertNil(state.lastError)
        XCTAssertEqual(state.payment.settlementStatus, .settled)
        XCTAssertEqual(state.connectionState, .connected)
    }

    func testReducerClearsPaymentErrorWhenSettlementBecomesNotRequired() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)
        state.handshakeComplete = true
        state.connectionState = .connected

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentSettlement,
                    paymentSettlement: PaymentSettlementFrame(status: .notRequired)
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertNil(state.lastError)
        XCTAssertEqual(state.payment.settlementStatus, .notRequired)
        XCTAssertEqual(state.connectionState, .connected)
    }

    func testReducerClearsPaymentErrorWhenPolicyNoLongerRequiresSettlement() {
        let reducer = DefaultMeetingStateReducer()
        var config = MeetingConfig.default
        config.requirePaymentSettlement = true
        var state = MeetingSessionState.initial(config: config)
        state.handshakeComplete = true
        state.connectionState = .connected

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: true, destinationAccount: "nexus://dest")
                )
            ),
            now: Date(timeIntervalSince1970: 1)
        )
        XCTAssertEqual(state.lastError?.code, "payment_settlement_required")
        XCTAssertEqual(state.connectionState, .error)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .paymentPolicy,
                    paymentPolicy: PaymentPolicyFrame(required: false, destinationAccount: nil)
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertNil(state.lastError)
        XCTAssertEqual(state.payment.settlementStatus, .notRequired)
        XCTAssertEqual(state.connectionState, .connected)
    }

    func testReducerKeepsFallbackActiveOnProtocolErrorFrame() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .fallbackActivated(reason: "native_degraded"),
            now: Date(timeIntervalSince1970: 1)
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                .error(category: .protocolFailure, code: "parse_error", message: "decode failed")
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.connectionState, .fallbackActive)
    }

    func testReducerMediaProfileDowngradeTransitionsDegradedAndRecoversConnected() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.handshakeComplete = true
        state.connectionState = .connected

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .mediaProfileNegotiation,
                    mediaProfileNegotiation: MediaProfileNegotiationFrame(
                        participantID: "alice",
                        preferredProfile: .hdr,
                        negotiatedProfile: .sdr,
                        colorPrimaries: "bt2020",
                        transferFunction: "pq",
                        codec: "h265",
                        epoch: 4
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.connectionState, .degraded)
        XCTAssertEqual(state.mediaProfile.negotiatedProfile, .sdr)

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .mediaProfileNegotiation,
                    mediaProfileNegotiation: MediaProfileNegotiationFrame(
                        participantID: "alice",
                        preferredProfile: .sdr,
                        negotiatedProfile: .sdr,
                        colorPrimaries: "bt709",
                        transferFunction: "gamma",
                        codec: "h264",
                        epoch: 5
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 3)
        )

        XCTAssertEqual(state.connectionState, .connected)
        XCTAssertEqual(state.mediaProfile.preferredProfile, .sdr)
    }

    @MainActor
    func testMeetingSessionPolicyFailureFrameActivatesFallbackWhenPreferred() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.preferWebFallbackOnPolicyFailure = true
        session.config = config

        client.emit(
            .frameReceived(
                .error(category: .policyFailure, code: "policy_reject", message: "blocked by policy")
            )
        )
        await Task.yield()

        XCTAssertTrue(session.shouldShowFallback)
        XCTAssertTrue(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.fallback.reason, "blocked by policy")
        XCTAssertEqual(session.sessionState.connectionState, .fallbackActive)
        XCTAssertEqual(session.sessionState.lastError?.code, "fallback_activated")
        XCTAssertEqual(client.disconnectReasons.last, "fallback_activated")
    }

    @MainActor
    func testMeetingSessionPolicyFailureAndFallbackEmitTelemetryEvents() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let telemetry = InMemoryMeetingTelemetrySink()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            telemetrySink: telemetry,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.preferWebFallbackOnPolicyFailure = true
        session.config = config

        client.emit(
            .frameReceived(
                .error(category: .policyFailure, code: "policy_reject", message: "blocked by policy")
            )
        )
        await Task.yield()

        let policyEvent = telemetry.events.first {
            $0.category == .policyFailure && $0.attributes["code"] == "policy_reject"
        }
        XCTAssertNotNil(policyEvent)

        let fallbackEvent = telemetry.events.first {
            $0.category == .fallbackLifecycle && $0.name == "fallback_activated"
        }
        XCTAssertEqual(fallbackEvent?.attributes["reason"], "blocked by policy")
    }

    @MainActor
    func testMeetingSessionFallbackRecoveryEmitsRecoveryRtoTelemetry() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let telemetry = InMemoryMeetingTelemetrySink()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            telemetrySink: telemetry,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.preferWebFallbackOnPolicyFailure = true
        session.config = config

        client.emit(
            .frameReceived(
                .error(category: .policyFailure, code: "policy_reject", message: "blocked by policy")
            )
        )
        await Task.yield()

        session.recoverFromFallback()
        await Task.yield()

        let recoveryEvent = telemetry.events.first {
            $0.category == .fallbackLifecycle && $0.name == "fallback_recovered"
        }
        XCTAssertNotNil(recoveryEvent)

        let rtoValue = Int64(recoveryEvent?.attributes["rto_ms"] ?? "")
        XCTAssertNotNil(rtoValue)
        XCTAssertGreaterThanOrEqual(rtoValue ?? -1, 0)
    }

    @MainActor
    func testMeetingSessionReconnectExhaustionActivatesFallback() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        session.connect()
        client.emit(.transportFailed("network_down"))
        await Task.yield()

        XCTAssertTrue(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.connectionState, .fallbackActive)
        XCTAssertEqual(client.disconnectReasons.last, "fallback_activated")
    }

    @MainActor
    func testMeetingSessionRestoresResumeTokenForHandshake() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let firstClient = FakeMeetingProtocolClient()
        let firstSession = MeetingSession(
            protocolClient: firstClient,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        firstSession.connect()
        firstClient.emit(.connected)
        await Task.yield()
        firstClient.emit(
            .frameReceived(
                .handshakeAck(
                    HandshakeAckFrame(
                        sessionID: "s1",
                        resumeToken: "resume-7",
                        acceptedAtMs: 1_000
                    )
                )
            )
        )
        await Task.yield()

        XCTAssertEqual(firstSession.sessionState.resumeToken, "resume-7")
        XCTAssertEqual(persistence.resumeToken, "resume-7")

        let secondClient = FakeMeetingProtocolClient()
        let secondSession = MeetingSession(
            protocolClient: secondClient,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        XCTAssertEqual(secondSession.sessionState.resumeToken, "resume-7")

        secondSession.connect()
        secondClient.emit(.connected)
        await Task.yield()

        let handshakeFrame = secondClient.sentFrames.first { $0.kind == .handshake }
        XCTAssertEqual(handshakeFrame?.handshake?.resumeToken, "resume-7")
    }

    @MainActor
    func testMeetingSessionHandshakeIncludesWalletIdentity() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.walletIdentity = "nexus://wallet/tester"
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let handshakeFrame = client.sentFrames.first { $0.kind == .handshake }
        XCTAssertEqual(handshakeFrame?.handshake?.walletIdentity, "nexus://wallet/tester")
    }

    @MainActor
    func testMeetingSessionHandshakeUsesExplicitParticipantID() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.participantName = "Alice Runner"
        config.participantID = "Primary Host"
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let handshakeFrame = client.sentFrames.first { $0.kind == .handshake }
        XCTAssertEqual(handshakeFrame?.handshake?.participantID, "primary-host")
        XCTAssertEqual(handshakeFrame?.handshake?.participantName, "Alice Runner")
    }

    @MainActor
    func testMeetingSessionHandshakeFallsBackToParticipantWhenExplicitIDNormalizesEmpty() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.participantName = "Alice Runner"
        config.participantID = "###@@@"
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let handshakeFrame = client.sentFrames.first { $0.kind == .handshake }
        XCTAssertEqual(handshakeFrame?.handshake?.participantID, "participant")
        XCTAssertEqual(handshakeFrame?.handshake?.participantName, "Alice Runner")
    }

    @MainActor
    func testMeetingSessionHandshakeFallsBackToParticipantNameWhenParticipantIDMissing() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.participantName = "Alice Runner"
        config.participantID = nil
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let handshakeFrame = client.sentFrames.first { $0.kind == .handshake }
        XCTAssertEqual(handshakeFrame?.handshake?.participantID, "alice-runner")
        XCTAssertEqual(handshakeFrame?.handshake?.participantName, "Alice Runner")
    }

    @MainActor
    func testMeetingSessionKeyRotationAckUsesResolvedParticipantIDWhenParticipantIDMissing() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.participantName = "Alice Runner"
        config.participantID = nil
        session.config = config

        client.emit(
            .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "host",
                        epoch: 3,
                        publicKey: "pk-3",
                        signature: "sig-3",
                        issuedAtMs: 300
                    )
                )
            )
        )
        await Task.yield()

        let ackFrame = client.sentFrames.last { $0.kind == .keyRotationAck }?.keyRotationAck
        XCTAssertEqual(ackFrame?.participantID, "alice-runner")
        XCTAssertEqual(ackFrame?.ackEpoch, 3)
    }

    @MainActor
    func testMeetingSessionHandshakeRespectsHDRCapabilityOverrides() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.supportsHDRCapture = true
        config.supportsHDRRender = false
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let handshakeFrame = client.sentFrames.first { $0.kind == .handshake }?.handshake
        XCTAssertEqual(handshakeFrame?.preferredProfile, .sdr)
        XCTAssertEqual(handshakeFrame?.hdrCapture, true)
        XCTAssertEqual(handshakeFrame?.hdrRender, false)

        let capabilityFrame = client.sentFrames.first { $0.kind == .deviceCapability }?.deviceCapability
        XCTAssertEqual(capabilityFrame?.hdrCapture, true)
        XCTAssertEqual(capabilityFrame?.hdrRender, false)
    }

    @MainActor
    func testMeetingSessionConnectedSendsHandshakeCapabilityAndPaymentPolicyFrames() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.requirePaymentSettlement = true
        session.config = config

        session.connect()
        client.emit(.connected)
        await Task.yield()

        let kinds = client.sentFrames.map(\.kind)
        XCTAssertEqual(kinds, [.handshake, .deviceCapability, .paymentPolicy])
    }

    @MainActor
    func testMeetingSessionModerationRequiresHostRoleEvenWhenSignaturesOptional() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.requireSignedModeration = false
        config.preferWebFallbackOnPolicyFailure = false
        session.config = config

        session.sendModeration(action: .mute, targetParticipantID: "target")
        await Task.yield()

        XCTAssertEqual(session.sessionState.lastError?.code, "moderation_not_authorized")
        XCTAssertTrue(client.sentFrames.filter { $0.kind == .moderationSigned }.isEmpty)
    }

    @MainActor
    func testMeetingSessionRespondsToPingWithPong() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        session.connect()
        client.emit(.connected)
        await Task.yield()
        client.sentFrames.removeAll()

        client.emit(.frameReceived(.ping(at: 123)))
        await Task.yield()

        XCTAssertEqual(client.sentFrames.last?.kind, .pong)
    }

    @MainActor
    func testMeetingSessionAcknowledgesE2EEKeyEpoch() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        session.connect()
        client.emit(.connected)
        await Task.yield()
        client.sentFrames.removeAll()

        client.emit(
            .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "host",
                        epoch: 3,
                        publicKey: "pk-3",
                        signature: "sig-3",
                        issuedAtMs: 300
                    )
                )
            )
        )
        await Task.yield()

        XCTAssertEqual(client.sentFrames.last?.kind, .keyRotationAck)
        XCTAssertEqual(client.sentFrames.last?.keyRotationAck?.ackEpoch, 3)
        XCTAssertEqual(session.sessionState.e2ee.lastAckEpoch, 3)
    }

    @MainActor
    func testMeetingSessionDoesNotAcknowledgeUnsignedE2EEKeyEpochWhenSignaturesRequired() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.requireSignedModeration = true
        config.preferWebFallbackOnPolicyFailure = false
        session.config = config

        client.emit(
            .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "host",
                        epoch: 3,
                        publicKey: "pk-3",
                        signature: "sig-3",
                        issuedAtMs: 300
                    )
                )
            )
        )
        await Task.yield()
        XCTAssertEqual(client.sentFrames.last?.kind, .keyRotationAck)
        XCTAssertEqual(session.sessionState.e2ee.lastAckEpoch, 3)

        client.sentFrames.removeAll()
        client.emit(
            .frameReceived(
                MeetingProtocolFrame(
                    kind: .e2eeKeyEpoch,
                    e2eeKeyEpoch: E2EEKeyEpochFrame(
                        participantID: "host",
                        epoch: 2,
                        publicKey: "pk-2",
                        signature: "",
                        issuedAtMs: 400
                    )
                )
            )
        )
        await Task.yield()

        XCTAssertTrue(client.sentFrames.isEmpty)
        XCTAssertEqual(session.sessionState.e2ee.lastAckEpoch, 3)
        XCTAssertEqual(session.sessionState.lastError?.code, "e2ee_signature_missing")
    }

    @MainActor
    func testMeetingSessionRestoresFallbackPhaseFromPersistence() async {
        let persistence = InMemoryMeetingSessionPersistence()
        persistence.fallbackActive = true
        persistence.fallbackReason = "policy_failure"

        let session = MeetingSession(
            protocolClient: FakeMeetingProtocolClient(),
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        XCTAssertTrue(session.shouldShowFallback)
        XCTAssertTrue(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.fallback.reason, "policy_failure")
        XCTAssertEqual(session.sessionState.connectionState, .fallbackActive)
        XCTAssertEqual(session.transportState, SessionConnectionState.fallbackActive.label)
        await Task.yield()
    }

    @MainActor
    func testMeetingSessionForegroundTriggersImmediateReconnectWhileBackoffPending() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [60.0]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)

        client.emit(.transportFailed("network_down"))
        await Task.yield()

        session.onAppForegrounded()
        await Task.yield()

        XCTAssertEqual(client.connectURLs.count, 2)
        XCTAssertFalse(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.connectionState, .connecting)
    }

    @MainActor
    func testMeetingSessionConnectivityRestoreTriggersSingleReconnect() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [60.0, 60.0]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)

        session.onConnectivityChanged(available: false)
        session.onConnectivityChanged(available: false)
        await Task.yield()

        session.onConnectivityChanged(available: true)
        await Task.yield()

        XCTAssertEqual(client.connectURLs.count, 2)
        XCTAssertFalse(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.connectionState, .connecting)

        session.onConnectivityChanged(available: true)
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 2)
    }

    @MainActor
    func testMeetingSessionManualDisconnectSuppressesLifecycleReconnect() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [0.01]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)

        session.disconnect()
        client.emit(.transportFailed("network_down"))
        session.onAppForegrounded()
        session.onConnectivityChanged(available: true)
        session.onConnectivityChanged(available: false)
        await Task.yield()

        XCTAssertEqual(client.connectURLs.count, 1)
        XCTAssertEqual(client.disconnectReasons.first, "user_requested")
        XCTAssertFalse(session.sessionState.fallback.active)
    }

    @MainActor
    func testMeetingSessionBackgroundDefersReconnectUntilForegrounded() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let telemetry = InMemoryMeetingTelemetrySink()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            telemetrySink: telemetry,
            reconnectBackoffSeconds: [60.0]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)
        client.emit(.connected)
        await Task.yield()

        session.onAppBackgrounded()
        await Task.yield()
        XCTAssertEqual(client.disconnectReasons.last, "app_backgrounded")
        XCTAssertEqual(session.sessionState.connectionState, .degraded)

        client.emit(.disconnected(reason: "app_backgrounded"))
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 1)

        session.onAppForegrounded()
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 2)

        let backgroundEvent = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "app_backgrounded"
        }
        XCTAssertNotNil(backgroundEvent)
        let foregroundEvent = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "app_foregrounded"
        }
        XCTAssertNotNil(foregroundEvent)
    }

    @MainActor
    func testMeetingSessionConnectivityRestoreDefersWhileBackgrounded() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [60.0]
        )

        session.onAppBackgrounded()
        session.onConnectivityChanged(available: true)
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 0)

        session.onAppForegrounded()
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 1)
    }

    @MainActor
    func testMeetingSessionAudioInterruptionHooksEmitTelemetryAndReconnect() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let telemetry = InMemoryMeetingTelemetrySink()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            telemetrySink: telemetry,
            reconnectBackoffSeconds: [60.0]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)
        client.emit(.connected)
        await Task.yield()

        session.onAudioInterruptionBegan()
        await Task.yield()
        XCTAssertEqual(session.sessionState.connectionState, .degraded)

        session.onAudioInterruptionEnded()
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 2)

        let interruptionBegan = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "audio_interruption_began"
        }
        XCTAssertNotNil(interruptionBegan)

        let interruptionEnded = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "audio_interruption_ended"
        }
        XCTAssertEqual(interruptionEnded?.attributes["should_reconnect"], "true")
    }

    @MainActor
    func testMeetingSessionRouteAndScreenCaptureHooksEmitTelemetry() async {
        let telemetry = InMemoryMeetingTelemetrySink()
        let session = MeetingSession(
            protocolClient: FakeMeetingProtocolClient(),
            persistence: InMemoryMeetingSessionPersistence(),
            telemetrySink: telemetry,
            reconnectBackoffSeconds: []
        )

        session.onAudioRouteChanged(reason: "new_device_available")
        session.onScreenCaptureCapabilityChanged(available: false, source: "macos_preflight")
        await Task.yield()

        let routeEvent = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "audio_route_changed"
        }
        XCTAssertEqual(routeEvent?.attributes["reason"], "new_device_available")

        let screenCaptureEvent = telemetry.events.first {
            $0.category == .connectionLifecycle && $0.name == "screen_capture_capability"
        }
        XCTAssertEqual(screenCaptureEvent?.attributes["available"], "false")
        XCTAssertEqual(screenCaptureEvent?.attributes["source"], "macos_preflight")
    }
}

private final class FakeMeetingProtocolClient: MeetingProtocolClient {
    var eventHandler: ((MeetingProtocolClientEvent) -> Void)?
    var sentFrames: [MeetingProtocolFrame] = []
    var connectURLs: [URL] = []
    var disconnectReasons: [String] = []

    func connect(to url: URL) {
        connectURLs.append(url)
    }

    func disconnect(reason: String) {
        disconnectReasons.append(reason)
    }

    func send(frame: MeetingProtocolFrame) {
        sentFrames.append(frame)
    }

    func emit(_ event: MeetingProtocolClientEvent) {
        eventHandler?(event)
    }
}

private final class InMemoryMeetingSessionPersistence: MeetingSessionPersistence {
    var config: MeetingConfig?
    var resumeToken: String?
    var fallbackActive = false
    var fallbackReason: String?

    func loadConfig() -> MeetingConfig? {
        config
    }

    func saveConfig(_ config: MeetingConfig) {
        self.config = config
    }

    func loadResumeToken() -> String? {
        resumeToken
    }

    func saveResumeToken(_ token: String?) {
        resumeToken = token
    }

    func loadFallbackActive() -> Bool {
        fallbackActive
    }

    func saveFallbackActive(_ active: Bool) {
        fallbackActive = active
    }

    func loadFallbackReason() -> String? {
        fallbackReason
    }

    func saveFallbackReason(_ reason: String?) {
        fallbackReason = reason
    }
}

private final class InMemoryMeetingTelemetrySink: MeetingTelemetrySink {
    private(set) var events: [MeetingTelemetryEvent] = []

    func record(_ event: MeetingTelemetryEvent) {
        events.append(event)
    }
}
