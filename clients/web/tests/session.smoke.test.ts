import { describe, expect, it } from 'vitest';
import { reduceSession } from '../src/session/reducer';
import {
  ConnectionPhase,
  MediaProfile,
  initialState,
  type MeetingConfig
} from '../src/session/types';

describe('web session smoke', () => {
  it('tracks fallback activation and recovery RTO', () => {
    const config: MeetingConfig = {
      signalingUrl: 'ws://127.0.0.1:9000',
      fallbackUrl: 'https://fallback.example',
      roomId: 'smoke-room',
      participantId: 'web-smoke',
      participantName: 'smoke',
      requireSignedModeration: true,
      requirePaymentSettlement: false,
      preferWebFallbackOnPolicyFailure: true,
      supportsHdrCapture: true,
      supportsHdrRender: true
    };

    const start = initialState(config);
    const activated = reduceSession(start, { kind: 'fallbackActivated', reason: 'drill' }, 1_000);
    const recovered = reduceSession(activated, { kind: 'fallbackRecovered' }, 1_850);

    expect(activated.connectionPhase).toBe(ConnectionPhase.FallbackActive);
    expect(recovered.fallback.lastRtoMs).toBe(850);
  });

  it('marks degraded on hdr downgrade', () => {
    const config: MeetingConfig = {
      signalingUrl: 'ws://127.0.0.1:9000',
      fallbackUrl: 'https://fallback.example',
      roomId: 'smoke-room',
      participantId: 'web-smoke',
      participantName: 'smoke',
      requireSignedModeration: true,
      requirePaymentSettlement: false,
      preferWebFallbackOnPolicyFailure: true,
      supportsHdrCapture: true,
      supportsHdrRender: true
    };

    const connected = reduceSession(initialState(config), {
      kind: 'frameReceived',
      frame: {
        kind: 'handshakeAck',
        sessionId: 'session',
        resumeToken: 'token',
        acceptedAtMs: 100
      }
    });

    const degraded = reduceSession(connected, {
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

    expect(degraded.connectionPhase).toBe(ConnectionPhase.Degraded);
  });
});
