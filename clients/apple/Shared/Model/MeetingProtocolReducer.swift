import Foundation

protocol MeetingStateReducer {
    func reduce(state: MeetingSessionState, event: MeetingSessionEvent, now: Date) -> MeetingSessionState
}

struct DefaultMeetingStateReducer: MeetingStateReducer {
    func reduce(state: MeetingSessionState, event: MeetingSessionEvent, now: Date) -> MeetingSessionState {
        var next = state
        let nowMs = Int64(now.timeIntervalSince1970 * 1000)

        switch event {
        case .connectRequested:
            next.connectionState = .connecting
            next.handshakeComplete = false
            next.lastError = nil

        case .transportConnected:
            next.connectionState = .connecting
            next.lastError = nil

        case .transportDisconnected(let reason):
            next.handshakeComplete = false
            if next.fallback.active {
                next.connectionState = .fallbackActive
            } else {
                next.connectionState = .degraded
                next.lastError = SessionError(
                    category: .transportFailure,
                    code: "transport_disconnected",
                    message: reason,
                    atMs: nowMs
                )
            }

        case .transportFailure(let message):
            next.handshakeComplete = false
            if next.fallback.active {
                next.connectionState = .fallbackActive
            } else {
                next.connectionState = .degraded
                next.lastError = SessionError(
                    category: .transportFailure,
                    code: "transport_failure",
                    message: message,
                    atMs: nowMs
                )
            }

        case .frameSendFailed(let message):
            if next.fallback.active {
                next.connectionState = .fallbackActive
            } else {
                next.connectionState = .degraded
                next.lastError = SessionError(
                    category: .transportFailure,
                    code: "send_failed",
                    message: message,
                    atMs: nowMs
                )
            }

        case .manualDisconnected:
            next.connectionState = .disconnected
            next.handshakeComplete = false
            next.lastError = nil

        case .fallbackActivated(let reason):
            next.connectionState = .fallbackActive
            next.fallback.active = true
            next.fallback.reason = reason
            next.fallback.activatedAtMs = nowMs
            next.lastError = SessionError(
                category: .transportFailure,
                code: "fallback_activated",
                message: reason,
                atMs: nowMs
            )

        case .fallbackRecovered:
            next.fallback.active = false
            next.fallback.reason = nil
            next.fallback.recoveredAtMs = nowMs
            if let activatedAt = next.fallback.activatedAtMs {
                next.fallback.lastRtoMs = max(0, nowMs - activatedAt)
            }
            next.connectionState = .disconnected

        case .configUpdated(let config):
            next.config = config
            enforcePaymentSettlementPolicy(state: &next, nowMs: nowMs)

        case .frameReceived(let frame):
            apply(frame: frame, to: &next, nowMs: nowMs)
        }

        return next
    }

    private func apply(frame: MeetingProtocolFrame, to state: inout MeetingSessionState, nowMs: Int64) {
        switch frame.kind {
        case .handshakeAck:
            guard let ack = frame.handshakeAck else { return }
            state.handshakeComplete = true
            state.resumeToken = ack.resumeToken
            state.connectionState = state.fallback.active ? .fallbackActive : .connected
            state.lastError = nil

        case .participantPresenceDelta:
            guard let delta = frame.presenceDelta else { return }
            if delta.sequence <= state.presenceSequence {
                return
            }
            state.presenceSequence = delta.sequence
            for participant in delta.joined {
                state.participants[participant.id] = participant
            }
            for leaving in delta.left {
                state.participants.removeValue(forKey: leaving)
            }
            for change in delta.roleChanges {
                guard var participant = state.participants[change.participantID] else { continue }
                participant.role = change.role
                state.participants[change.participantID] = participant
            }

        case .roleGrant:
            guard let frame = frame.roleGrant else { return }
            guard hasRequiredSignature(frame.signature, state: state) else {
                rejectPolicy(
                    code: "role_grant_signature_missing",
                    message: "RoleGrant signature is required",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            guard actorIsAuthorized(frame.grantedBy, state: state) else {
                rejectPolicy(
                    code: "role_grant_not_authorized",
                    message: "RoleGrant issuer is not host/co-host",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            if var participant = state.participants[frame.targetParticipantID] {
                participant.role = frame.role
                state.participants[frame.targetParticipantID] = participant
            }

        case .roleRevoke:
            guard let frame = frame.roleRevoke else { return }
            guard hasRequiredSignature(frame.signature, state: state) else {
                rejectPolicy(
                    code: "role_revoke_signature_missing",
                    message: "RoleRevoke signature is required",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            guard actorIsAuthorized(frame.revokedBy, state: state) else {
                rejectPolicy(
                    code: "role_revoke_not_authorized",
                    message: "RoleRevoke issuer is not host/co-host",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            if var participant = state.participants[frame.targetParticipantID], participant.role == frame.role {
                participant.role = .participant
                state.participants[frame.targetParticipantID] = participant
            }

        case .permissionsSnapshot:
            guard let frame = frame.permissionsSnapshot else { return }
            if let prior = state.permissionSnapshots[frame.participantID], frame.epoch <= prior.epoch {
                return
            }
            state.permissionSnapshots[frame.participantID] = frame

        case .moderationSigned:
            guard let frame = frame.moderationSigned else { return }
            guard hasRequiredSignature(frame.signature, state: state) else {
                rejectPolicy(
                    code: "moderation_signature_missing",
                    message: "Moderation signature is required",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            guard actorIsAuthorized(frame.issuedBy, state: state) else {
                rejectPolicy(
                    code: "moderation_not_authorized",
                    message: "Moderation issuer is not host/co-host",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            applyModeration(frame: frame, to: &state)

        case .sessionPolicy:
            guard let frame = frame.sessionPolicy else { return }
            guard hasRequiredSignature(frame.signature, state: state) else {
                rejectPolicy(
                    code: "session_policy_signature_missing",
                    message: "SessionPolicy signature is required",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            guard actorIsAuthorized(frame.updatedBy, state: state) else {
                rejectPolicy(
                    code: "session_policy_not_authorized",
                    message: "SessionPolicy issuer is not host/co-host",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            guard frame.policyEpoch >= state.policyEpoch else {
                return
            }
            state.roomLocked = frame.roomLock
            state.waitingRoomEnabled = frame.waitingRoomEnabled
            state.guestPolicy = frame.guestPolicy
            state.e2eeRequired = frame.e2eeRequired
            state.maxParticipants = frame.maxParticipants
            state.policyEpoch = frame.policyEpoch
            state.recordingNotice.state = frame.recordingPolicy
            enforceE2EEEpochPolicy(state: &state, nowMs: nowMs)

        case .mediaProfileNegotiation:
            guard let frame = frame.mediaProfileNegotiation else { return }
            state.mediaProfile = MediaProfileState(
                preferredProfile: frame.preferredProfile,
                negotiatedProfile: frame.negotiatedProfile,
                colorPrimaries: frame.colorPrimaries,
                transferFunction: frame.transferFunction,
                codec: frame.codec
            )
            if frame.preferredProfile == .hdr && frame.negotiatedProfile == .sdr {
                state.connectionState = .degraded
            } else if state.handshakeComplete && !state.fallback.active {
                state.connectionState = .connected
            }

        case .recordingNotice:
            guard let frame = frame.recordingNotice else { return }
            state.recordingNotice = RecordingNoticeState(
                state: frame.state,
                policyBasis: frame.policyBasis,
                issuedBy: frame.issuedBy
            )

        case .e2eeKeyEpoch:
            guard let frame = frame.e2eeKeyEpoch else { return }
            guard hasRequiredSignature(frame.signature, state: state) else {
                rejectPolicy(
                    code: "e2ee_signature_missing",
                    message: "E2EE key epoch signature is required",
                    state: &state,
                    nowMs: nowMs
                )
                return
            }
            state.e2ee.currentEpoch = max(state.e2ee.currentEpoch, frame.epoch)
            enforceE2EEEpochPolicy(state: &state, nowMs: nowMs)

        case .keyRotationAck:
            guard let frame = frame.keyRotationAck else { return }
            state.e2ee.lastAckEpoch = max(state.e2ee.lastAckEpoch, frame.ackEpoch)

        case .paymentPolicy:
            guard let frame = frame.paymentPolicy else { return }
            state.payment.required = frame.required
            state.payment.destination = frame.destinationAccount
            if frame.required {
                state.payment.settlementStatus = .pending
            } else {
                state.payment.settlementStatus = .notRequired
            }
            enforcePaymentSettlementPolicy(state: &state, nowMs: nowMs)

        case .paymentSettlement:
            guard let frame = frame.paymentSettlement else { return }
            state.payment.settlementStatus = frame.status
            enforcePaymentSettlementPolicy(state: &state, nowMs: nowMs)

        case .error:
            guard let frame = frame.error else { return }
            state.lastError = SessionError(
                category: frame.category,
                code: frame.code,
                message: frame.message,
                atMs: nowMs
            )
            switch frame.category {
            case .policyFailure:
                state.connectionState = state.fallback.active ? .fallbackActive : .error
            case .protocolFailure:
                state.connectionState = state.fallback.active ? .fallbackActive : .degraded
            case .transportFailure:
                state.connectionState = state.fallback.active ? .fallbackActive : .degraded
            }

        case .ping, .pong, .handshake, .deviceCapability:
            break
        }
    }

    private func applyModeration(frame: ModerationSignedFrame, to state: inout MeetingSessionState) {
        guard var participant = state.participants[frame.targetParticipantID] else { return }

        switch frame.action {
        case .mute:
            participant.muted = true
        case .videoOff:
            participant.videoEnabled = false
        case .stopShare:
            participant.shareEnabled = false
        case .kick:
            state.participants.removeValue(forKey: frame.targetParticipantID)
            return
        case .admitFromWaiting:
            participant.waitingRoom = false
        case .denyFromWaiting:
            state.participants.removeValue(forKey: frame.targetParticipantID)
            return
        }

        state.participants[frame.targetParticipantID] = participant
    }

    private func actorIsAuthorized(_ actorID: String, state: MeetingSessionState) -> Bool {
        if actorID == "system" {
            return true
        }
        guard let actor = state.participants[actorID] else {
            return false
        }
        return actor.role == .host || actor.role == .coHost
    }

    private func hasRequiredSignature(_ signature: String, state: MeetingSessionState) -> Bool {
        if !state.config.requireSignedModeration {
            return true
        }
        return !signature.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func rejectPolicy(code: String, message: String, state: inout MeetingSessionState, nowMs: Int64) {
        state.lastError = SessionError(
            category: .policyFailure,
            code: code,
            message: message,
            atMs: nowMs
        )
        state.connectionState = state.fallback.active ? .fallbackActive : .error
    }

    private func enforceE2EEEpochPolicy(state: inout MeetingSessionState, nowMs: Int64) {
        if !state.e2eeRequired || state.e2ee.currentEpoch >= 1 {
            clearE2EEEpochPolicyErrorIfNeeded(state: &state)
            return
        }

        rejectPolicy(
            code: "e2ee_epoch_required",
            message: "E2EE key epoch is required before continuing",
            state: &state,
            nowMs: nowMs
        )
    }

    private func clearE2EEEpochPolicyErrorIfNeeded(state: inout MeetingSessionState) {
        guard let error = state.lastError else { return }
        guard error.category == .policyFailure else { return }
        guard error.code == "e2ee_epoch_required" else { return }

        state.lastError = nil
        if state.fallback.active {
            state.connectionState = .fallbackActive
        } else if state.connectionState == .error && state.handshakeComplete {
            state.connectionState = .connected
        }
    }

    private func enforcePaymentSettlementPolicy(state: inout MeetingSessionState, nowMs: Int64) {
        guard state.config.requirePaymentSettlement, state.payment.required else {
            clearPaymentPolicyErrorIfNeeded(state: &state)
            return
        }

        switch state.payment.settlementStatus {
        case .settled, .notRequired:
            clearPaymentPolicyErrorIfNeeded(state: &state)
        case .blocked:
            rejectPolicy(
                code: "payment_settlement_blocked",
                message: "Payment settlement is blocked by policy",
                state: &state,
                nowMs: nowMs
            )
        case .pending:
            rejectPolicy(
                code: "payment_settlement_required",
                message: "Payment settlement is required before continuing",
                state: &state,
                nowMs: nowMs
            )
        }
    }

    private func clearPaymentPolicyErrorIfNeeded(state: inout MeetingSessionState) {
        guard let error = state.lastError else { return }
        guard error.category == .policyFailure else { return }
        guard error.code == "payment_settlement_required" || error.code == "payment_settlement_blocked" else { return }

        state.lastError = nil
        if state.fallback.active {
            state.connectionState = .fallbackActive
        } else if state.connectionState == .error && state.handshakeComplete {
            state.connectionState = .connected
        }
    }
}
