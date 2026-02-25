import { describe, expect, it } from 'vitest';
import { reduceSession } from '../src/session/reducer';
import {
  ConnectionPhase,
  GuestPolicy,
  MediaProfile,
  ParticipantRole,
  PaymentSettlementStatus,
  RecordingState,
  SessionErrorCategory,
  initialState,
  type MeetingConfig,
  type ProtocolSessionState
} from '../src/session/types';

const baseConfig: MeetingConfig = {
  signalingUrl: 'ws://127.0.0.1:9000',
  fallbackUrl: 'https://fallback.example',
  roomId: 'room-a',
  participantId: 'p1',
  participantName: 'alpha',
  requireSignedModeration: true,
  requirePaymentSettlement: true,
  preferWebFallbackOnPolicyFailure: false,
  supportsHdrCapture: true,
  supportsHdrRender: true
};

function withHost(state: ProtocolSessionState): ProtocolSessionState {
  return {
    ...state,
    participants: {
      host: {
        id: 'host',
        displayName: 'host',
        role: ParticipantRole.Host,
        muted: false,
        videoEnabled: true,
        shareEnabled: true,
        waitingRoom: false
      }
    }
  };
}

describe('web reducer', () => {
  it('marks connected on handshake ack', () => {
    const state = reduceSession(initialState(baseConfig), { kind: 'frameReceived', frame: {
      kind: 'handshakeAck',
      sessionId: 's1',
      resumeToken: 'rt-1',
      acceptedAtMs: 100
    }});

    expect(state.connectionPhase).toBe(ConnectionPhase.Connected);
    expect(state.handshakeComplete).toBe(true);
    expect(state.resumeToken).toBe('rt-1');
  });

  it('keeps presence delta monotonic', () => {
    const start = initialState(baseConfig);
    const s1 = reduceSession(start, {
      kind: 'frameReceived',
      frame: {
        kind: 'participantPresenceDelta',
        joined: [
          {
            id: 'p2',
            displayName: 'beta',
            role: ParticipantRole.Participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
          }
        ],
        left: [],
        roleChanges: [],
        sequence: 4
      }
    });

    const stale = reduceSession(s1, {
      kind: 'frameReceived',
      frame: {
        kind: 'participantPresenceDelta',
        joined: [],
        left: ['p2'],
        roleChanges: [],
        sequence: 3
      }
    });

    expect(stale.presenceSequence).toBe(4);
    expect(stale.participants.p2).toBeDefined();
  });

  it('rejects unsigned role grant when signatures are required', () => {
    const state = withHost(initialState(baseConfig));
    const next = reduceSession(state, {
      kind: 'frameReceived',
      frame: {
        kind: 'roleGrant',
        targetParticipantId: 'host',
        role: ParticipantRole.CoHost,
        grantedBy: 'host',
        issuedAtMs: 10
      }
    });

    expect(next.connectionPhase).toBe(ConnectionPhase.Error);
    expect(next.lastError?.category).toBe(SessionErrorCategory.PolicyFailure);
    expect(next.lastError?.code).toBe('role_grant_signature_missing');
  });

  it('transitions degraded on hdr downgrade and recovers on hdr negotiation', () => {
    const connected = reduceSession(initialState(baseConfig), {
      kind: 'frameReceived',
      frame: {
        kind: 'handshakeAck',
        sessionId: 's1',
        resumeToken: 'rt-2',
        acceptedAtMs: 10
      }
    });

    const downgraded = reduceSession(connected, {
      kind: 'frameReceived',
      frame: {
        kind: 'mediaProfileNegotiation',
        preferredProfile: MediaProfile.HDR,
        negotiatedProfile: MediaProfile.SDR,
        colorPrimaries: 'bt709',
        transferFunction: 'gamma',
        codec: 'h264'
      }
    });

    expect(downgraded.connectionPhase).toBe(ConnectionPhase.Degraded);

    const recovered = reduceSession(downgraded, {
      kind: 'frameReceived',
      frame: {
        kind: 'mediaProfileNegotiation',
        preferredProfile: MediaProfile.HDR,
        negotiatedProfile: MediaProfile.HDR,
        colorPrimaries: 'bt2020',
        transferFunction: 'pq',
        codec: 'h265'
      }
    });

    expect(recovered.connectionPhase).toBe(ConnectionPhase.Connected);
  });

  it('clears payment policy error when settlement no longer required by config', () => {
    const unsettled = reduceSession(initialState(baseConfig), {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'wallet:abc'
      }
    });

    expect(unsettled.lastError?.code).toBe('payment_settlement_required');

    const clear = reduceSession(unsettled, {
      kind: 'configUpdated',
      config: {
        ...baseConfig,
        requirePaymentSettlement: false
      }
    });

    expect(clear.lastError).toBeUndefined();
  });

  it('uses blocked payment settlement policy code when settlement is blocked', () => {
    const state = reduceSession(initialState(baseConfig), {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'wallet:abc'
      }
    });

    const blocked = reduceSession(state, {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentSettlement',
        status: PaymentSettlementStatus.Blocked
      }
    });

    expect(blocked.lastError?.code).toBe('payment_settlement_blocked');
  });

  it('clears payment policy error when settlement becomes not required', () => {
    const pending = reduceSession(initialState(baseConfig), {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'wallet:abc'
      }
    });

    expect(pending.lastError?.code).toBe('payment_settlement_required');

    const notRequired = reduceSession(pending, {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentSettlement',
        status: PaymentSettlementStatus.NotRequired
      }
    });

    expect(notRequired.lastError).toBeUndefined();
    expect(notRequired.connectionPhase).toBe(ConnectionPhase.Connecting);
  });

  it('rejects unauthorized moderation issuer even when signatures are optional', () => {
    const config: MeetingConfig = {
      ...baseConfig,
      requireSignedModeration: false,
      preferWebFallbackOnPolicyFailure: false
    };
    const state = reduceSession(withHost(initialState(config)), {
      kind: 'frameReceived',
      frame: {
        kind: 'participantPresenceDelta',
        joined: [
          {
            id: 'participant-1',
            displayName: 'Participant 1',
            role: ParticipantRole.Participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
          }
        ],
        left: [],
        roleChanges: [],
        sequence: 1
      }
    });

    const next = reduceSession(state, {
      kind: 'frameReceived',
      frame: {
        kind: 'moderationSigned',
        targetParticipantId: 'participant-1',
        action: 'mute',
        issuedBy: 'participant-1',
        issuedAtMs: 42
      }
    });

    expect(next.connectionPhase).toBe(ConnectionPhase.Error);
    expect(next.lastError?.code).toBe('moderation_not_authorized');
  });

  it('updates session policy and enforces e2ee epoch', () => {
    const state = withHost(initialState(baseConfig));
    const next = reduceSession(state, {
      kind: 'frameReceived',
      frame: {
        kind: 'sessionPolicy',
        roomLock: true,
        waitingRoomEnabled: true,
        recordingPolicy: RecordingState.Started,
        guestPolicy: GuestPolicy.InviteOnly,
        e2eeRequired: true,
        maxParticipants: 64,
        policyEpoch: 2,
        updatedBy: 'host',
        signature: 'sig',
        updatedAtMs: 22
      }
    });

    expect(next.roomLocked).toBe(true);
    expect(next.waitingRoomEnabled).toBe(true);
    expect(next.policyEpoch).toBe(2);
    expect(next.lastError?.code).toBe('e2ee_epoch_required');

    const clear = reduceSession(next, {
      kind: 'frameReceived',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 2,
        issuedBy: 'host',
        signature: 'sig',
        sentAtMs: 30
      }
    });

    expect(clear.lastError).toBeUndefined();
  });

  it('keeps fallback active on protocol and transport errors', () => {
    const active = reduceSession(initialState(baseConfig), {
      kind: 'fallbackActivated',
      reason: 'network-drop'
    });
    const protocolError = reduceSession(active, {
      kind: 'frameReceived',
      frame: {
        kind: 'error',
        category: SessionErrorCategory.ProtocolFailure,
        code: 'decode_failed',
        message: 'bad frame'
      }
    });
    const transportError = reduceSession(protocolError, {
      kind: 'transportFailure',
      message: 'socket timeout'
    });

    expect(protocolError.connectionPhase).toBe(ConnectionPhase.FallbackActive);
    expect(transportError.connectionPhase).toBe(ConnectionPhase.FallbackActive);
  });

  it('clears payment policy reject when settlement becomes settled', () => {
    const unsettled = reduceSession(initialState(baseConfig), {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'wallet:settle'
      }
    });

    const settled = reduceSession(unsettled, {
      kind: 'frameReceived',
      frame: {
        kind: 'paymentSettlement',
        status: PaymentSettlementStatus.Settled
      }
    });

    expect(settled.lastError).toBeUndefined();
  });
});
