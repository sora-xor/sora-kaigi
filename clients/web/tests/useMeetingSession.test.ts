import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  MeetingTelemetryCategory,
  type MeetingTelemetryEvent,
  type MeetingTelemetrySink
} from '../src/session/telemetry';
import { useMeetingSession } from '../src/session/useMeetingSession';
import type { ProtocolTransport, TransportEvent } from '../src/session/transport';
import {
  ConnectionPhase,
  type MeetingConfig,
  type ProtocolFrame
} from '../src/session/types';

class StubTransport implements ProtocolTransport {
  sentFrames: ProtocolFrame[] = [];
  connectCalls: string[] = [];
  disconnectCalls = 0;
  private handler?: (event: TransportEvent) => void;

  connect(url: string): void {
    this.connectCalls.push(url);
  }

  disconnect(): void {
    this.disconnectCalls += 1;
  }

  send(frame: ProtocolFrame): void {
    this.sentFrames.push(frame);
  }

  onEvent(handler: (event: TransportEvent) => void): void {
    this.handler = handler;
  }

  emit(event: TransportEvent): void {
    this.handler?.(event);
  }
}

class InMemoryTelemetrySink implements MeetingTelemetrySink {
  events: MeetingTelemetryEvent[] = [];

  record(event: MeetingTelemetryEvent): void {
    this.events.push(event);
  }
}

const baseConfig: MeetingConfig = {
  signalingUrl: 'ws://127.0.0.1:9000',
  fallbackUrl: 'https://fallback.example',
  roomId: 'room-web',
  participantId: 'web-1',
  participantName: 'web-user',
  requireSignedModeration: true,
  requirePaymentSettlement: false,
  preferWebFallbackOnPolicyFailure: true,
  supportsHdrCapture: true,
  supportsHdrRender: true
};

describe('web meeting runtime', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('sends handshake, capability, and payment policy frames after connect', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(
      () => ({ ...baseConfig, requirePaymentSettlement: true }),
      transport
    );

    api.connect();
    transport.emit({ kind: 'connected' });

    expect(transport.connectCalls).toHaveLength(1);
    expect(transport.sentFrames.map((frame) => frame.kind)).toEqual([
      'handshake',
      'deviceCapability',
      'paymentPolicy'
    ]);
    const handshake = transport.sentFrames[0];
    if (!handshake || handshake.kind !== 'handshake') {
      throw new Error('expected handshake frame');
    }
    expect(handshake.participantId).toBe('web-1');
    expect(handshake.preferredProfile).toBe('hdr');
    expect(handshake.hdrCapture).toBe(true);
    expect(handshake.hdrRender).toBe(true);

    const capability = transport.sentFrames[1];
    if (!capability || capability.kind !== 'deviceCapability') {
      throw new Error('expected device capability frame');
    }
    expect(capability.participantId).toBe('web-1');
    expect(capability.hdrCapture).toBe(true);
    expect(capability.hdrRender).toBe(true);
  });

  it('uses SDR handshake profile when HDR capabilities are disabled', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(
      () => ({
        ...baseConfig,
        participantId: 'web-sdr-1',
        supportsHdrCapture: false,
        supportsHdrRender: false
      }),
      transport
    );

    api.connect();
    transport.emit({ kind: 'connected' });

    expect(transport.sentFrames.map((frame) => frame.kind)).toEqual([
      'handshake',
      'deviceCapability'
    ]);
    const handshake = transport.sentFrames[0];
    if (!handshake || handshake.kind !== 'handshake') {
      throw new Error('expected handshake frame');
    }
    expect(handshake.participantId).toBe('web-sdr-1');
    expect(handshake.preferredProfile).toBe('sdr');
    expect(handshake.hdrCapture).toBe(false);
    expect(handshake.hdrRender).toBe(false);

    const capability = transport.sentFrames[1];
    if (!capability || capability.kind !== 'deviceCapability') {
      throw new Error('expected device capability frame');
    }
    expect(capability.participantId).toBe('web-sdr-1');
    expect(capability.hdrCapture).toBe(false);
    expect(capability.hdrRender).toBe(false);
  });

  it('resolves participant id from participant name when participant id is blank', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(
      () => ({
        ...baseConfig,
        participantId: '   ',
        participantName: 'Web QA 42'
      }),
      transport
    );

    api.connect();
    transport.emit({ kind: 'connected' });

    const handshake = transport.sentFrames[0];
    if (!handshake || handshake.kind !== 'handshake') {
      throw new Error('expected handshake frame');
    }
    expect(handshake.participantId).toBe('web-qa-42');

    const capability = transport.sentFrames[1];
    if (!capability || capability.kind !== 'deviceCapability') {
      throw new Error('expected device capability frame');
    }
    expect(capability.participantId).toBe('web-qa-42');

    transport.sentFrames = [];
    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 5,
        issuedBy: 'host',
        signature: 'sig-5',
        sentAtMs: 500
      }
    });

    const ack = transport.sentFrames[transport.sentFrames.length - 1];
    if (!ack || ack.kind !== 'keyRotationAck') {
      throw new Error('expected keyRotationAck frame');
    }
    expect(ack.participantId).toBe('web-qa-42');
    expect(ack.ackEpoch).toBe(5);
  });

  it('falls back to participant when explicit participant id normalizes empty', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(
      () => ({
        ...baseConfig,
        participantId: '###@@@',
        participantName: 'Web QA 42'
      }),
      transport
    );

    api.connect();
    transport.emit({ kind: 'connected' });

    const handshake = transport.sentFrames[0];
    if (!handshake || handshake.kind !== 'handshake') {
      throw new Error('expected handshake frame');
    }
    expect(handshake.participantId).toBe('participant');

    const capability = transport.sentFrames[1];
    if (!capability || capability.kind !== 'deviceCapability') {
      throw new Error('expected device capability frame');
    }
    expect(capability.participantId).toBe('participant');

    transport.sentFrames = [];
    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 5,
        issuedBy: 'host',
        signature: 'sig-5',
        sentAtMs: 500
      }
    });

    const ack = transport.sentFrames[transport.sentFrames.length - 1];
    if (!ack || ack.kind !== 'keyRotationAck') {
      throw new Error('expected keyRotationAck frame');
    }
    expect(ack.participantId).toBe('participant');
    expect(ack.ackEpoch).toBe(5);
  });

  it('responds to ping with pong', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(() => baseConfig, transport);
    api.connect();
    transport.emit({ kind: 'connected' });

    transport.emit({ kind: 'frame', frame: { kind: 'ping', sentAtMs: 1 } });

    const last = transport.sentFrames[transport.sentFrames.length - 1];
    expect(last?.kind).toBe('pong');
  });

  it('responds to e2ee key epoch with key rotation ack', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(() => baseConfig, transport);
    api.connect();
    transport.emit({ kind: 'connected' });
    transport.sentFrames = [];

    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 3,
        issuedBy: 'host',
        signature: 'sig-3',
        sentAtMs: 300
      }
    });

    const last = transport.sentFrames[transport.sentFrames.length - 1];
    expect(last?.kind).toBe('keyRotationAck');
    expect(api.state.value.e2eeState.lastAckEpoch).toBe(3);
  });

  it('does not acknowledge unsigned e2ee key epoch when signatures are required', () => {
    const transport = new StubTransport();
    const api = useMeetingSession(() => ({ ...baseConfig, requireSignedModeration: true }), transport);
    api.connect();
    transport.emit({ kind: 'connected' });
    transport.sentFrames = [];

    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 3,
        issuedBy: 'host',
        signature: 'sig-3',
        sentAtMs: 300
      }
    });
    expect(transport.sentFrames.at(-1)?.kind).toBe('keyRotationAck');
    expect(api.state.value.e2eeState.lastAckEpoch).toBe(3);

    transport.sentFrames = [];
    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'e2eeKeyEpoch',
        epoch: 2,
        issuedBy: 'host',
        signature: '',
        sentAtMs: 400
      }
    });

    expect(transport.sentFrames).toHaveLength(0);
    expect(api.state.value.e2eeState.lastAckEpoch).toBe(3);
    expect(api.state.value.lastError?.code).toBe('e2ee_signature_missing');
  });

  it('activates fallback when reconnect attempts are exhausted', () => {
    vi.useFakeTimers();
    const transport = new StubTransport();
    const api = useMeetingSession(() => baseConfig, transport);
    api.connect();

    const backoffMs = [1000, 2000, 4000, 8000];
    for (const delayMs of backoffMs) {
      transport.emit({ kind: 'failure', message: `socket_timeout_${delayMs}` });
      expect(api.state.value.fallback.active).toBe(false);
      vi.advanceTimersByTime(delayMs);
    }

    transport.emit({ kind: 'failure', message: 'socket_timeout_final' });

    expect(api.state.value.connectionPhase).toBe(ConnectionPhase.FallbackActive);
    expect(api.state.value.fallback.active).toBe(true);
    expect(api.state.value.fallback.reason).toContain('Reconnect exhausted');
    expect(transport.connectCalls).toHaveLength(1 + backoffMs.length);
    expect(transport.disconnectCalls).toBe(1);
  });

  it('records policy-failure and fallback telemetry events', () => {
    const transport = new StubTransport();
    const telemetry = new InMemoryTelemetrySink();
    const api = useMeetingSession(
      () => ({
        ...baseConfig,
        requirePaymentSettlement: true,
        preferWebFallbackOnPolicyFailure: true
      }),
      transport,
      telemetry
    );

    api.connect();
    transport.emit({ kind: 'connected' });
    transport.emit({
      kind: 'frame',
      frame: {
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'nexus://dest'
      }
    });

    const policyEvent = telemetry.events.find(
      (event) =>
        event.category === MeetingTelemetryCategory.PolicyFailure &&
        event.name === 'payment_settlement_required'
    );
    expect(policyEvent?.attributes.code).toBe('payment_settlement_required');

    const fallbackEvent = telemetry.events.find(
      (event) =>
        event.category === MeetingTelemetryCategory.FallbackLifecycle &&
        event.name === 'fallback_activated'
    );
    expect(fallbackEvent?.attributes.reason).toBe('policy:payment_settlement_required');
  });

  it('records fallback recovery telemetry with rto_ms', () => {
    const transport = new StubTransport();
    const telemetry = new InMemoryTelemetrySink();
    const nowSpy = vi.spyOn(Date, 'now');
    let nowMs = 1_000;
    nowSpy.mockImplementation(() => nowMs);

    const api = useMeetingSession(() => baseConfig, transport, telemetry);

    api.triggerFallback('native_degraded');
    nowMs = 1_375;
    api.recoverFallback();

    const recoveredEvent = telemetry.events.find(
      (event) =>
        event.category === MeetingTelemetryCategory.FallbackLifecycle &&
        event.name === 'fallback_recovered'
    );
    expect(recoveredEvent?.attributes.rto_ms).toBe('375');

    nowSpy.mockRestore();
  });
});
