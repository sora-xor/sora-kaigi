import {
  ConnectionPhase,
  GuestPolicy,
  MediaProfile,
  Participant,
  ParticipantRole,
  PaymentSettlementStatus,
  RecordingState,
  SessionErrorCategory,
  type PaymentState,
  type ProtocolEvent,
  type ProtocolFrame,
  type ProtocolSessionState,
  type SessionError
} from './types';

function hasRequiredSignature(signature: string | undefined, state: ProtocolSessionState): boolean {
  if (!state.config.requireSignedModeration) return true;
  return Boolean(signature && signature.trim());
}

function actorIsAuthorized(issuer: string, state: ProtocolSessionState): boolean {
  if (issuer === 'system') return true;
  const participant = state.participants[issuer];
  return participant?.role === ParticipantRole.Host || participant?.role === ParticipantRole.CoHost;
}

function clearErrorIfMatching(
  state: ProtocolSessionState,
  predicate: (error: SessionError | undefined) => boolean
): ProtocolSessionState {
  if (!predicate(state.lastError)) return state;
  return {
    ...state,
    connectionPhase: state.fallback.active
      ? ConnectionPhase.FallbackActive
      : state.handshakeComplete
        ? ConnectionPhase.Connected
        : ConnectionPhase.Connecting,
    lastError: undefined
  };
}

function clearPaymentPolicyErrorIfNeeded(state: ProtocolSessionState): ProtocolSessionState {
  return clearErrorIfMatching(
    state,
    (error) =>
      error?.category === SessionErrorCategory.PolicyFailure &&
      (error.code.startsWith('payment_settlement_') || error.code === 'payment_unsettled')
  );
}

function clearE2eePolicyErrorIfNeeded(state: ProtocolSessionState): ProtocolSessionState {
  return clearErrorIfMatching(
    state,
    (error) => error?.category === SessionErrorCategory.PolicyFailure && error.code.startsWith('e2ee_')
  );
}

function policyReject(
  state: ProtocolSessionState,
  nowMs: number,
  code: string,
  message: string
): ProtocolSessionState {
  const fallbackRequested = state.config.preferWebFallbackOnPolicyFailure;
  const fallbackActive = fallbackRequested || state.fallback.active;
  return {
    ...state,
    connectionPhase: fallbackActive ? ConnectionPhase.FallbackActive : ConnectionPhase.Error,
    fallback: fallbackActive
      ? {
          ...state.fallback,
          active: true,
          reason: state.fallback.reason ?? `policy:${code}`,
          activatedAtMs: state.fallback.activatedAtMs ?? nowMs
        }
      : state.fallback,
    lastError: {
      category: SessionErrorCategory.PolicyFailure,
      code,
      message,
      atMs: nowMs
    }
  };
}

function enforcePaymentSettlementPolicy(state: ProtocolSessionState, nowMs: number): ProtocolSessionState {
  if (!state.config.requirePaymentSettlement || !state.paymentState.required) {
    return clearPaymentPolicyErrorIfNeeded(state);
  }

  switch (state.paymentState.settlementStatus) {
    case PaymentSettlementStatus.Settled:
    case PaymentSettlementStatus.NotRequired:
      return clearPaymentPolicyErrorIfNeeded(state);
    case PaymentSettlementStatus.Blocked:
      return policyReject(state, nowMs, 'payment_settlement_blocked', 'Payment settlement blocked by policy');
    case PaymentSettlementStatus.Pending:
    default:
      return policyReject(
        state,
        nowMs,
        'payment_settlement_required',
        'Payment settlement required before media/session actions can continue'
      );
  }
}

function enforceE2eeEpochPolicy(state: ProtocolSessionState, nowMs: number): ProtocolSessionState {
  if (!state.e2eeRequired) {
    return clearE2eePolicyErrorIfNeeded(state);
  }

  if (state.e2eeState.currentEpoch > 0) {
    return clearE2eePolicyErrorIfNeeded(state);
  }

  return policyReject(state, nowMs, 'e2ee_epoch_required', 'E2EE key epoch is required by session policy');
}

function reduceFrame(state: ProtocolSessionState, frame: ProtocolFrame, nowMs: number): ProtocolSessionState {
  switch (frame.kind) {
    case 'handshakeAck':
      return {
        ...state,
        handshakeComplete: true,
        resumeToken: frame.resumeToken,
        connectionPhase: state.fallback.active ? ConnectionPhase.FallbackActive : ConnectionPhase.Connected,
        lastError: undefined
      };

    case 'participantPresenceDelta': {
      if (frame.sequence <= state.presenceSequence) return state;
      const nextParticipants: Record<string, Participant> = { ...state.participants };
      for (const participant of frame.joined) {
        nextParticipants[participant.id] = participant;
      }
      for (const participantId of frame.left) {
        delete nextParticipants[participantId];
      }
      for (const change of frame.roleChanges) {
        const participant = nextParticipants[change.participantId];
        if (participant) {
          nextParticipants[change.participantId] = { ...participant, role: change.role };
        }
      }
      return {
        ...state,
        participants: nextParticipants,
        presenceSequence: frame.sequence
      };
    }

    case 'roleGrant': {
      if (!hasRequiredSignature(frame.signature, state)) {
        return policyReject(state, nowMs, 'role_grant_signature_missing', 'RoleGrant signature is required');
      }
      if (!actorIsAuthorized(frame.grantedBy, state)) {
        return policyReject(state, nowMs, 'role_grant_not_authorized', 'RoleGrant issuer is not host/co-host');
      }
      const participant = state.participants[frame.targetParticipantId];
      if (!participant) return state;
      return {
        ...state,
        participants: {
          ...state.participants,
          [frame.targetParticipantId]: { ...participant, role: frame.role }
        }
      };
    }

    case 'roleRevoke': {
      if (!hasRequiredSignature(frame.signature, state)) {
        return policyReject(state, nowMs, 'role_revoke_signature_missing', 'RoleRevoke signature is required');
      }
      if (!actorIsAuthorized(frame.revokedBy, state)) {
        return policyReject(state, nowMs, 'role_revoke_not_authorized', 'RoleRevoke issuer is not host/co-host');
      }
      const participant = state.participants[frame.targetParticipantId];
      if (!participant || participant.role !== frame.role) return state;
      return {
        ...state,
        participants: {
          ...state.participants,
          [frame.targetParticipantId]: { ...participant, role: ParticipantRole.Participant }
        }
      };
    }

    case 'permissionsSnapshot': {
      const previous = state.permissionSnapshots[frame.participantId];
      if (previous && frame.epoch <= previous.epoch) return state;
      return {
        ...state,
        permissionSnapshots: {
          ...state.permissionSnapshots,
          [frame.participantId]: {
            effectivePermissions: frame.effectivePermissions,
            epoch: frame.epoch
          }
        }
      };
    }

    case 'moderationSigned': {
      if (!hasRequiredSignature(frame.signature, state)) {
        return policyReject(state, nowMs, 'moderation_signature_missing', 'Moderation signature is required');
      }
      if (!actorIsAuthorized(frame.issuedBy, state)) {
        return policyReject(state, nowMs, 'moderation_not_authorized', 'Moderation issuer is not host/co-host');
      }
      const participant = state.participants[frame.targetParticipantId];
      if (!participant) return state;
      if (frame.action === 'mute') {
        return {
          ...state,
          participants: {
            ...state.participants,
            [frame.targetParticipantId]: { ...participant, muted: true }
          }
        };
      }
      if (frame.action === 'video_off') {
        return {
          ...state,
          participants: {
            ...state.participants,
            [frame.targetParticipantId]: { ...participant, videoEnabled: false }
          }
        };
      }
      if (frame.action === 'stop_share') {
        return {
          ...state,
          participants: {
            ...state.participants,
            [frame.targetParticipantId]: { ...participant, shareEnabled: false }
          }
        };
      }
      if (frame.action === 'admit_from_waiting') {
        return {
          ...state,
          participants: {
            ...state.participants,
            [frame.targetParticipantId]: { ...participant, waitingRoom: false }
          }
        };
      }
      const nextParticipants = { ...state.participants };
      delete nextParticipants[frame.targetParticipantId];
      return { ...state, participants: nextParticipants };
    }

    case 'sessionPolicy': {
      if (!hasRequiredSignature(frame.signature, state)) {
        return policyReject(state, nowMs, 'session_policy_signature_missing', 'SessionPolicy signature is required');
      }
      if (!actorIsAuthorized(frame.updatedBy, state)) {
        return policyReject(state, nowMs, 'session_policy_not_authorized', 'SessionPolicy issuer is not host/co-host');
      }
      if (frame.policyEpoch < state.policyEpoch) {
        return state;
      }
      return enforceE2eeEpochPolicy(
        {
          ...state,
          roomLocked: frame.roomLock,
          waitingRoomEnabled: frame.waitingRoomEnabled,
          guestPolicy: frame.guestPolicy,
          e2eeRequired: frame.e2eeRequired,
          maxParticipants: frame.maxParticipants,
          policyEpoch: frame.policyEpoch,
          recordingNotice: frame.recordingPolicy
        },
        nowMs
      );
    }

    case 'mediaProfileNegotiation': {
      const phase =
        frame.preferredProfile === MediaProfile.HDR && frame.negotiatedProfile === MediaProfile.SDR
          ? ConnectionPhase.Degraded
          : state.handshakeComplete && !state.fallback.active
            ? ConnectionPhase.Connected
            : state.connectionPhase;
      return {
        ...state,
        mediaProfile: {
          preferredProfile: frame.preferredProfile,
          negotiatedProfile: frame.negotiatedProfile,
          colorPrimaries: frame.colorPrimaries,
          transferFunction: frame.transferFunction,
          codec: frame.codec
        },
        connectionPhase: phase
      };
    }

    case 'recordingNotice':
      return { ...state, recordingNotice: frame.state };

    case 'e2eeKeyEpoch': {
      if (!hasRequiredSignature(frame.signature, state)) {
        return policyReject(state, nowMs, 'e2ee_signature_missing', 'E2EE key epoch signature is required');
      }
      return enforceE2eeEpochPolicy(
        {
          ...state,
          e2eeState: {
            ...state.e2eeState,
            currentEpoch: Math.max(state.e2eeState.currentEpoch, frame.epoch)
          }
        },
        nowMs
      );
    }

    case 'keyRotationAck':
      return {
        ...state,
        e2eeState: {
          ...state.e2eeState,
          lastAckEpoch: Math.max(state.e2eeState.lastAckEpoch, frame.ackEpoch)
        }
      };

    case 'paymentPolicy': {
      const paymentState: PaymentState = {
        required: frame.required,
        destination: frame.destinationAccount,
        settlementStatus: frame.required ? PaymentSettlementStatus.Pending : PaymentSettlementStatus.NotRequired
      };
      return enforcePaymentSettlementPolicy({ ...state, paymentState }, nowMs);
    }

    case 'paymentSettlement': {
      const paymentState: PaymentState = {
        ...state.paymentState,
        settlementStatus: frame.status
      };
      return enforcePaymentSettlementPolicy({ ...state, paymentState }, nowMs);
    }

    case 'error': {
      const phase =
        state.fallback.active
          ? ConnectionPhase.FallbackActive
          : frame.category === SessionErrorCategory.PolicyFailure
            ? ConnectionPhase.Error
            : ConnectionPhase.Degraded;
      return {
        ...state,
        connectionPhase: phase,
        lastError: {
          category: frame.category,
          code: frame.code,
          message: frame.message,
          atMs: nowMs
        }
      };
    }

    case 'handshake':
    case 'deviceCapability':
    case 'ping':
    case 'pong':
      return state;
  }
}

export function reduceSession(
  state: ProtocolSessionState,
  event: ProtocolEvent,
  nowMs = Date.now()
): ProtocolSessionState {
  switch (event.kind) {
    case 'connectRequested':
      return {
        ...state,
        connectionPhase: ConnectionPhase.Connecting,
        handshakeComplete: false,
        lastError: undefined
      };

    case 'transportConnected':
      return {
        ...state,
        connectionPhase: ConnectionPhase.Connecting,
        lastError: undefined
      };

    case 'transportDisconnected':
      return state.fallback.active
        ? {
            ...state,
            connectionPhase: ConnectionPhase.FallbackActive,
            handshakeComplete: false
          }
        : {
            ...state,
            connectionPhase: ConnectionPhase.Degraded,
            handshakeComplete: false,
            lastError: {
              category: SessionErrorCategory.TransportFailure,
              code: 'transport_disconnected',
              message: event.reason,
              atMs: nowMs
            }
          };

    case 'transportFailure':
      return state.fallback.active
        ? {
            ...state,
            connectionPhase: ConnectionPhase.FallbackActive,
            handshakeComplete: false
          }
        : {
            ...state,
            connectionPhase: ConnectionPhase.Degraded,
            handshakeComplete: false,
            lastError: {
              category: SessionErrorCategory.TransportFailure,
              code: 'transport_failure',
              message: event.message,
              atMs: nowMs
            }
          };

    case 'frameSendFailed':
      return state.fallback.active
        ? { ...state, connectionPhase: ConnectionPhase.FallbackActive }
        : {
            ...state,
            connectionPhase: ConnectionPhase.Degraded,
            lastError: {
              category: SessionErrorCategory.TransportFailure,
              code: 'send_failed',
              message: event.message,
              atMs: nowMs
            }
          };

    case 'manualDisconnected':
      return {
        ...state,
        connectionPhase: ConnectionPhase.Disconnected,
        handshakeComplete: false,
        lastError: undefined
      };

    case 'fallbackActivated':
      return {
        ...state,
        connectionPhase: ConnectionPhase.FallbackActive,
        fallback: {
          ...state.fallback,
          active: true,
          reason: event.reason,
          activatedAtMs: nowMs
        },
        lastError: {
          category: SessionErrorCategory.TransportFailure,
          code: 'fallback_activated',
          message: event.reason,
          atMs: nowMs
        }
      };

    case 'fallbackRecovered': {
      const rto = state.fallback.activatedAtMs ? Math.max(0, nowMs - state.fallback.activatedAtMs) : undefined;
      return {
        ...state,
        connectionPhase: ConnectionPhase.Disconnected,
        fallback: {
          ...state.fallback,
          active: false,
          reason: undefined,
          recoveredAtMs: nowMs,
          lastRtoMs: rto
        }
      };
    }

    case 'configUpdated':
      return enforcePaymentSettlementPolicy({ ...state, config: event.config }, nowMs);

    case 'frameReceived':
      return reduceFrame(state, event.frame, nowMs);
  }
}
