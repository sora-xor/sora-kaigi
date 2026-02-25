package io.sora.kaigi.android

interface ProtocolReducer {
    fun reduce(state: ProtocolSessionState, event: ProtocolEvent, nowMs: Long = System.currentTimeMillis()): ProtocolSessionState
}

class DefaultProtocolReducer : ProtocolReducer {
    override fun reduce(state: ProtocolSessionState, event: ProtocolEvent, nowMs: Long): ProtocolSessionState {
        return when (event) {
            ProtocolEvent.ConnectRequested -> state.copy(
                connectionPhase = ConnectionPhase.Connecting,
                handshakeComplete = false,
                lastError = null
            )

            ProtocolEvent.TransportConnected -> state.copy(
                connectionPhase = ConnectionPhase.Connecting,
                lastError = null
            )

            is ProtocolEvent.TransportDisconnected ->
                if (state.fallback.active) {
                    state.copy(
                        connectionPhase = ConnectionPhase.FallbackActive,
                        handshakeComplete = false
                    )
                } else {
                    state.copy(
                        connectionPhase = ConnectionPhase.Degraded,
                        handshakeComplete = false,
                        lastError = SessionError(
                            category = SessionErrorCategory.TransportFailure,
                            code = "transport_disconnected",
                            message = event.reason,
                            atMs = nowMs
                        )
                    )
                }

            is ProtocolEvent.TransportFailure ->
                if (state.fallback.active) {
                    state.copy(
                        connectionPhase = ConnectionPhase.FallbackActive,
                        handshakeComplete = false
                    )
                } else {
                    state.copy(
                        connectionPhase = ConnectionPhase.Degraded,
                        handshakeComplete = false,
                        lastError = SessionError(
                            category = SessionErrorCategory.TransportFailure,
                            code = "transport_failure",
                            message = event.message,
                            atMs = nowMs
                        )
                    )
                }

            is ProtocolEvent.FrameSendFailed ->
                if (state.fallback.active) {
                    state.copy(connectionPhase = ConnectionPhase.FallbackActive)
                } else {
                    state.copy(
                        connectionPhase = ConnectionPhase.Degraded,
                        lastError = SessionError(
                            category = SessionErrorCategory.TransportFailure,
                            code = "send_failed",
                            message = event.message,
                            atMs = nowMs
                        )
                    )
                }

            ProtocolEvent.ManualDisconnected -> state.copy(
                connectionPhase = ConnectionPhase.Disconnected,
                handshakeComplete = false,
                lastError = null
            )

            is ProtocolEvent.FallbackActivated -> state.copy(
                connectionPhase = ConnectionPhase.FallbackActive,
                fallback = state.fallback.copy(
                    active = true,
                    reason = event.reason,
                    activatedAtMs = nowMs
                ),
                lastError = SessionError(
                    category = SessionErrorCategory.TransportFailure,
                    code = "fallback_activated",
                    message = event.reason,
                    atMs = nowMs
                )
            )

            ProtocolEvent.FallbackRecovered -> {
                val rto = if (state.fallback.activatedAtMs != null) {
                    (nowMs - state.fallback.activatedAtMs).coerceAtLeast(0)
                } else {
                    null
                }
                state.copy(
                    connectionPhase = ConnectionPhase.Disconnected,
                    fallback = state.fallback.copy(
                        active = false,
                        reason = null,
                        recoveredAtMs = nowMs,
                        lastRtoMs = rto
                    )
                )
            }

            is ProtocolEvent.ConfigUpdated -> enforcePaymentSettlementPolicy(
                state.copy(config = event.config),
                nowMs
            )

            is ProtocolEvent.FrameReceived -> reduceFrame(state, event.frame, nowMs)
        }
    }

    private fun reduceFrame(state: ProtocolSessionState, frame: ProtocolFrame, nowMs: Long): ProtocolSessionState {
        return when (frame) {
            is ProtocolFrame.HandshakeAck -> state.copy(
                handshakeComplete = true,
                resumeToken = frame.resumeToken,
                connectionPhase = if (state.fallback.active) ConnectionPhase.FallbackActive else ConnectionPhase.Connected,
                lastError = null
            )

            is ProtocolFrame.PresenceDelta -> {
                if (frame.sequence <= state.presenceSequence) return state
                val nextParticipants = state.participants.toMutableMap()
                frame.joined.forEach { nextParticipants[it.id] = it }
                frame.left.forEach { nextParticipants.remove(it) }
                frame.roleChanges.forEach { change ->
                    val participant = nextParticipants[change.participantId]
                    if (participant != null) {
                        nextParticipants[change.participantId] = participant.copy(role = change.role)
                    }
                }
                state.copy(participants = nextParticipants, presenceSequence = frame.sequence)
            }

            is ProtocolFrame.RoleGrant -> {
                if (!hasRequiredSignature(frame.signature, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "role_grant_signature_missing",
                        message = "RoleGrant signature is required"
                    )
                }
                if (!actorIsAuthorized(frame.grantedBy, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "role_grant_not_authorized",
                        message = "RoleGrant issuer is not host/co-host"
                    )
                }
                val participant = state.participants[frame.targetParticipantId]
                if (participant == null) state else {
                    state.copy(participants = state.participants + (frame.targetParticipantId to participant.copy(role = frame.role)))
                }
            }

            is ProtocolFrame.RoleRevoke -> {
                if (!hasRequiredSignature(frame.signature, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "role_revoke_signature_missing",
                        message = "RoleRevoke signature is required"
                    )
                }
                if (!actorIsAuthorized(frame.revokedBy, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "role_revoke_not_authorized",
                        message = "RoleRevoke issuer is not host/co-host"
                    )
                }
                val participant = state.participants[frame.targetParticipantId]
                if (participant == null || participant.role != frame.role) state else {
                    state.copy(participants = state.participants + (frame.targetParticipantId to participant.copy(role = ParticipantRole.Participant)))
                }
            }

            is ProtocolFrame.PermissionsSnapshot -> {
                val prior = state.permissionSnapshots[frame.participantId]
                if (prior != null && frame.epoch <= prior.epoch) {
                    state
                } else {
                    state.copy(
                        permissionSnapshots = state.permissionSnapshots + (
                            frame.participantId to PermissionSnapshot(
                                effectivePermissions = frame.effectivePermissions,
                                epoch = frame.epoch
                            )
                        )
                    )
                }
            }

            is ProtocolFrame.ModerationSigned -> {
                if (!hasRequiredSignature(frame.signature, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "moderation_signature_missing",
                        message = "Moderation signature is required"
                    )
                }
                if (!actorIsAuthorized(frame.issuedBy, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "moderation_not_authorized",
                        message = "Moderation issuer is not host/co-host"
                    )
                }
                val participant = state.participants[frame.targetParticipantId] ?: return state
                when (frame.action) {
                    ModerationAction.Mute -> state.copy(
                        participants = state.participants + (frame.targetParticipantId to participant.copy(muted = true))
                    )

                    ModerationAction.VideoOff -> state.copy(
                        participants = state.participants + (frame.targetParticipantId to participant.copy(videoEnabled = false))
                    )

                    ModerationAction.StopShare -> state.copy(
                        participants = state.participants + (frame.targetParticipantId to participant.copy(shareEnabled = false))
                    )

                    ModerationAction.Kick,
                    ModerationAction.DenyFromWaiting -> state.copy(
                        participants = state.participants - frame.targetParticipantId
                    )

                    ModerationAction.AdmitFromWaiting -> state.copy(
                        participants = state.participants + (frame.targetParticipantId to participant.copy(waitingRoom = false))
                    )
                }
            }

            is ProtocolFrame.SessionPolicy -> {
                if (!hasRequiredSignature(frame.signature, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "session_policy_signature_missing",
                        message = "SessionPolicy signature is required"
                    )
                }
                if (!actorIsAuthorized(frame.updatedBy, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "session_policy_not_authorized",
                        message = "SessionPolicy issuer is not host/co-host"
                    )
                }
                if (frame.policyEpoch < state.policyEpoch) {
                    return state
                }
                enforceE2eeEpochPolicy(
                    state.copy(
                        roomLocked = frame.roomLock,
                        waitingRoomEnabled = frame.waitingRoomEnabled,
                        guestPolicy = frame.guestPolicy,
                        e2eeRequired = frame.e2eeRequired,
                        maxParticipants = frame.maxParticipants,
                        policyEpoch = frame.policyEpoch,
                        recordingNotice = frame.recordingPolicy
                    ),
                    nowMs
                )
            }

            is ProtocolFrame.MediaProfileNegotiation -> {
                val profile = MediaProfileState(
                    preferredProfile = frame.preferredProfile,
                    negotiatedProfile = frame.negotiatedProfile,
                    colorPrimaries = frame.colorPrimaries,
                    transferFunction = frame.transferFunction,
                    codec = frame.codec
                )
                val phase = if (frame.preferredProfile == MediaProfile.HDR && frame.negotiatedProfile == MediaProfile.SDR) {
                    ConnectionPhase.Degraded
                } else if (state.handshakeComplete && !state.fallback.active) {
                    ConnectionPhase.Connected
                } else {
                    state.connectionPhase
                }
                state.copy(mediaProfile = profile, connectionPhase = phase)
            }

            is ProtocolFrame.RecordingNotice -> state.copy(recordingNotice = frame.state)

            is ProtocolFrame.E2eeKeyEpoch -> {
                if (!hasRequiredSignature(frame.signature, state)) {
                    return policyReject(
                        state = state,
                        nowMs = nowMs,
                        code = "e2ee_signature_missing",
                        message = "E2EE key epoch signature is required"
                    )
                }
                enforceE2eeEpochPolicy(
                    state.copy(
                        e2eeState = state.e2eeState.copy(
                            currentEpoch = maxOf(state.e2eeState.currentEpoch, frame.epoch)
                        )
                    ),
                    nowMs
                )
            }

            is ProtocolFrame.KeyRotationAck -> state.copy(
                e2eeState = state.e2eeState.copy(
                    lastAckEpoch = maxOf(state.e2eeState.lastAckEpoch, frame.ackEpoch)
                )
            )

            is ProtocolFrame.PaymentPolicy -> {
                val settlement = if (frame.required) {
                    PaymentSettlementStatus.Pending
                } else {
                    PaymentSettlementStatus.NotRequired
                }
                enforcePaymentSettlementPolicy(
                    state.copy(
                        paymentState = state.paymentState.copy(
                            required = frame.required,
                            destination = frame.destinationAccount,
                            settlementStatus = settlement
                        )
                    ),
                    nowMs
                )
            }

            is ProtocolFrame.PaymentSettlement -> enforcePaymentSettlementPolicy(
                state.copy(
                    paymentState = state.paymentState.copy(settlementStatus = frame.status)
                ),
                nowMs
            )

            is ProtocolFrame.Error -> {
                val phase = when (frame.category) {
                    SessionErrorCategory.PolicyFailure ->
                        if (state.fallback.active) ConnectionPhase.FallbackActive else ConnectionPhase.Error
                    SessionErrorCategory.ProtocolFailure ->
                        if (state.fallback.active) ConnectionPhase.FallbackActive else ConnectionPhase.Degraded
                    SessionErrorCategory.TransportFailure ->
                        if (state.fallback.active) ConnectionPhase.FallbackActive else ConnectionPhase.Degraded
                }
                state.copy(
                    connectionPhase = phase,
                    lastError = SessionError(
                        category = frame.category,
                        code = frame.code,
                        message = frame.message,
                        atMs = nowMs
                    )
                )
            }

            is ProtocolFrame.Handshake,
            is ProtocolFrame.DeviceCapability,
            is ProtocolFrame.Ping,
            is ProtocolFrame.Pong -> state
        }
    }

    private fun actorIsAuthorized(actorId: String, state: ProtocolSessionState): Boolean {
        if (actorId == "system") return true
        val actor = state.participants[actorId] ?: return false
        return actor.role == ParticipantRole.Host || actor.role == ParticipantRole.CoHost
    }

    private fun hasRequiredSignature(signature: String, state: ProtocolSessionState): Boolean {
        if (!state.config.requireSignedModeration) return true
        return signature.isNotBlank()
    }

    private fun policyReject(
        state: ProtocolSessionState,
        nowMs: Long,
        code: String,
        message: String
    ): ProtocolSessionState {
        val phase = if (state.fallback.active) ConnectionPhase.FallbackActive else ConnectionPhase.Error
        return state.copy(
            connectionPhase = phase,
            lastError = SessionError(
                category = SessionErrorCategory.PolicyFailure,
                code = code,
                message = message,
                atMs = nowMs
            )
        )
    }

    private fun enforcePaymentSettlementPolicy(state: ProtocolSessionState, nowMs: Long): ProtocolSessionState {
        if (!state.config.requirePaymentSettlement || !state.paymentState.required) {
            return clearPaymentPolicyErrorIfNeeded(state)
        }

        return when (state.paymentState.settlementStatus) {
            PaymentSettlementStatus.Settled,
            PaymentSettlementStatus.NotRequired -> clearPaymentPolicyErrorIfNeeded(state)
            PaymentSettlementStatus.Blocked -> policyReject(
                state = state,
                nowMs = nowMs,
                code = "payment_settlement_blocked",
                message = "Payment settlement is blocked by policy"
            )
            PaymentSettlementStatus.Pending -> policyReject(
                state = state,
                nowMs = nowMs,
                code = "payment_settlement_required",
                message = "Payment settlement is required before continuing"
            )
        }
    }

    private fun clearPaymentPolicyErrorIfNeeded(state: ProtocolSessionState): ProtocolSessionState {
        val error = state.lastError ?: return state
        if (error.category != SessionErrorCategory.PolicyFailure) return state
        if (error.code != "payment_settlement_required" && error.code != "payment_settlement_blocked") {
            return state
        }

        val phase = when {
            state.fallback.active -> ConnectionPhase.FallbackActive
            state.connectionPhase == ConnectionPhase.Error && state.handshakeComplete -> ConnectionPhase.Connected
            else -> state.connectionPhase
        }

        return state.copy(connectionPhase = phase, lastError = null)
    }

    private fun enforceE2eeEpochPolicy(state: ProtocolSessionState, nowMs: Long): ProtocolSessionState {
        if (!state.e2eeRequired || state.e2eeState.currentEpoch >= 1) {
            return clearE2eeEpochPolicyErrorIfNeeded(state)
        }

        return policyReject(
            state = state,
            nowMs = nowMs,
            code = "e2ee_epoch_required",
            message = "E2EE key epoch is required before continuing"
        )
    }

    private fun clearE2eeEpochPolicyErrorIfNeeded(state: ProtocolSessionState): ProtocolSessionState {
        val error = state.lastError ?: return state
        if (error.category != SessionErrorCategory.PolicyFailure || error.code != "e2ee_epoch_required") {
            return state
        }

        val phase = when {
            state.fallback.active -> ConnectionPhase.FallbackActive
            state.connectionPhase == ConnectionPhase.Error && state.handshakeComplete -> ConnectionPhase.Connected
            else -> state.connectionPhase
        }

        return state.copy(connectionPhase = phase, lastError = null)
    }
}
