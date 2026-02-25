import { computed, ref } from 'vue';
import { reduceSession } from './reducer';
import {
  MeetingTelemetryCategory,
  NoOpMeetingTelemetrySink,
  type MeetingTelemetrySink
} from './telemetry';
import { BrowserWebSocketTransport, type ProtocolTransport } from './transport';
import {
  ConnectionPhase,
  MediaProfile,
  SessionErrorCategory,
  type HandshakeFrame,
  type MeetingConfig,
  type ProtocolEvent,
  type ProtocolFrame,
  type ProtocolSessionState,
  initialState
} from './types';

interface LogEntry {
  id: number;
  at: string;
  message: string;
}

interface UseMeetingSessionApi {
  state: ReturnType<typeof ref<ProtocolSessionState>>;
  log: ReturnType<typeof ref<LogEntry[]>>;
  canPing: ReturnType<typeof computed<boolean>>;
  connect: () => void;
  disconnect: () => void;
  ping: () => void;
  triggerFallback: (reason: string) => void;
  recoverFallback: () => void;
  updateConfig: () => void;
}

const RETRY_DELAYS_MS = [1000, 2000, 4000, 8000];

function hasRequiredSignature(signature: string | undefined, requireSignedModeration: boolean): boolean {
  if (!requireSignedModeration) return true;
  return Boolean(signature && signature.trim());
}

function normalizeParticipantId(raw: string): string {
  const source = raw.trim();
  const base = source.length > 0 ? source : 'participant';
  const normalized = base
    .toLowerCase()
    .split('')
    .map((ch) => {
      if ((ch >= 'a' && ch <= 'z') || (ch >= '0' && ch <= '9') || ch === '-' || ch === '_') return ch;
      if (ch.trim() === '') return '-';
      return '';
    })
    .join('');

  return normalized.length > 0 ? normalized : 'participant';
}

function resolveParticipantId(config: MeetingConfig): string {
  if (config.participantId.trim().length > 0) {
    return normalizeParticipantId(config.participantId);
  }
  return normalizeParticipantId(config.participantName);
}

export function useMeetingSession(
  resolveConfig: () => MeetingConfig,
  transport: ProtocolTransport = new BrowserWebSocketTransport(),
  telemetrySink: MeetingTelemetrySink = NoOpMeetingTelemetrySink
): UseMeetingSessionApi {
  const state = ref(initialState(resolveConfig()));
  const log = ref<LogEntry[]>([]);

  let reconnectAttempt = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let manuallyDisconnected = false;

  const appendLog = (message: string): void => {
    const entry: LogEntry = {
      id: Date.now() + Math.floor(Math.random() * 10_000),
      at: new Date().toISOString(),
      message
    };
    log.value = [entry, ...log.value].slice(0, 100);
  };

  const recordTelemetry = (
    category: MeetingTelemetryCategory,
    name: string,
    attributes: Record<string, string> = {},
    atMs: number = Date.now()
  ): void => {
    telemetrySink.record({
      category,
      name,
      attributes,
      atMs
    });
  };

  const applyEvent = (event: ProtocolEvent): void => {
    const nowMs = Date.now();
    const previousState = state.value;
    state.value = reduceSession(state.value, event, nowMs);
    const nextState = state.value;

    if (previousState.connectionPhase !== nextState.connectionPhase) {
      recordTelemetry(
        MeetingTelemetryCategory.ConnectionLifecycle,
        'phase_changed',
        {
          from: previousState.connectionPhase,
          to: nextState.connectionPhase
        },
        nowMs
      );
    }

    if (!previousState.fallback.active && nextState.fallback.active) {
      recordTelemetry(
        MeetingTelemetryCategory.FallbackLifecycle,
        'fallback_activated',
        {
          reason: nextState.fallback.reason ?? 'unknown'
        },
        nowMs
      );
    }

    if (previousState.fallback.active && !nextState.fallback.active) {
      const attributes: Record<string, string> = {};
      if (nextState.fallback.lastRtoMs !== undefined) {
        attributes.rto_ms = String(nextState.fallback.lastRtoMs);
      }
      recordTelemetry(MeetingTelemetryCategory.FallbackLifecycle, 'fallback_recovered', attributes, nowMs);
    }

    if (previousState.lastError !== nextState.lastError && nextState.lastError?.category === SessionErrorCategory.PolicyFailure) {
      recordTelemetry(
        MeetingTelemetryCategory.PolicyFailure,
        nextState.lastError.code,
        {
          code: nextState.lastError.code,
          message: nextState.lastError.message
        },
        nowMs
      );
    }
  };

  const preferredProfile = (): MediaProfile => {
    const config = resolveConfig();
    return config.supportsHdrCapture && config.supportsHdrRender ? MediaProfile.HDR : MediaProfile.SDR;
  };

  const handshakeFrame = (): HandshakeFrame => {
    const config = resolveConfig();
    const participantId = resolveParticipantId(config);
    return {
      kind: 'handshake',
      roomId: config.roomId,
      participantId,
      participantName: config.participantName,
      walletIdentity: config.walletIdentity,
      resumeToken: state.value.resumeToken,
      preferredProfile: preferredProfile(),
      hdrCapture: config.supportsHdrCapture,
      hdrRender: config.supportsHdrRender,
      sentAtMs: Date.now()
    };
  };

  const sendHandshakeFrames = (): void => {
    const config = resolveConfig();
    const participantId = resolveParticipantId(config);
    const nowMs = Date.now();
    transport.send(handshakeFrame());
    transport.send({
      kind: 'deviceCapability',
      participantId,
      codecs: ['h264', 'vp9'],
      hdrCapture: config.supportsHdrCapture,
      hdrRender: config.supportsHdrRender,
      maxStreams: 4,
      updatedAtMs: nowMs
    });
    if (config.requirePaymentSettlement) {
      transport.send({
        kind: 'paymentPolicy',
        required: true,
        destinationAccount: 'nexus://payment-policy'
      });
    }
  };

  const clearReconnectTimer = (): void => {
    if (!reconnectTimer) return;
    clearTimeout(reconnectTimer);
    reconnectTimer = undefined;
  };

  const scheduleReconnect = (reason: string): void => {
    if (state.value.fallback.active) return;
    if (reconnectTimer) return;
    if (reconnectAttempt >= RETRY_DELAYS_MS.length) {
      triggerFallback(`Reconnect exhausted after ${reconnectAttempt} attempts: ${reason}`);
      return;
    }
    const delay = RETRY_DELAYS_MS[reconnectAttempt];
    reconnectAttempt += 1;
    appendLog(`Reconnect scheduled in ${delay}ms (attempt ${reconnectAttempt})`);
    recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'reconnect_scheduled', {
      attempt: String(reconnectAttempt),
      due_in_ms: String(delay),
      trigger: reason
    });
    reconnectTimer = setTimeout(() => {
      recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'reconnect_attempt');
      connectInternal(false, 'reconnect');
    }, delay);
  };

  transport.onEvent((event) => {
    switch (event.kind) {
      case 'connected':
        reconnectAttempt = 0;
        appendLog('Transport connected');
        recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'transport_connected');
        applyEvent({ kind: 'transportConnected' });
        sendHandshakeFrames();
        return;

      case 'disconnected':
        appendLog(`Transport disconnected: ${event.reason}`);
        recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'transport_disconnected', {
          reason: event.reason
        });
        applyEvent({ kind: 'transportDisconnected', reason: event.reason });
        if (!manuallyDisconnected && !state.value.fallback.active) {
          scheduleReconnect(event.reason);
        }
        return;

      case 'failure':
        appendLog(`Transport failure: ${event.message}`);
        recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'transport_failure', {
          message: event.message
        });
        applyEvent({ kind: 'transportFailure', message: event.message });
        if (!manuallyDisconnected && state.value.connectionPhase !== ConnectionPhase.FallbackActive) {
          scheduleReconnect(event.message);
        }
        return;

      case 'frame':
        appendLog(`Frame received: ${event.frame.kind}`);
        applyEvent({ kind: 'frameReceived', frame: event.frame });
        if (event.frame.kind === 'ping') {
          transport.send({ kind: 'pong', sentAtMs: Date.now() });
        }
        if (
          event.frame.kind === 'e2eeKeyEpoch' &&
          hasRequiredSignature(event.frame.signature, state.value.config.requireSignedModeration) &&
          state.value.e2eeState.currentEpoch >= event.frame.epoch
        ) {
          const participantId = resolveParticipantId(resolveConfig());
          const ack: ProtocolFrame = {
            kind: 'keyRotationAck',
            ackEpoch: event.frame.epoch,
            participantId,
            sentAtMs: Date.now()
          };
          transport.send(ack);
          applyEvent({ kind: 'frameReceived', frame: ack });
        }
        if (
          event.frame.kind === 'error' &&
          event.frame.category === SessionErrorCategory.PolicyFailure &&
          resolveConfig().preferWebFallbackOnPolicyFailure
        ) {
          triggerFallback(`Policy rejection: ${event.frame.message}`);
        }
        return;
    }
  });

  const updateConfig = (): void => {
    applyEvent({ kind: 'configUpdated', config: resolveConfig() });
  };

  const connectInternal = (resetBackoff: boolean, source: 'manual' | 'reconnect' | 'fallback_recovery'): void => {
    manuallyDisconnected = false;
    if (resetBackoff) {
      reconnectAttempt = 0;
    }
    clearReconnectTimer();
    updateConfig();
    recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'connect_requested', { source });
    applyEvent({ kind: 'connectRequested' });
    appendLog(
      source === 'manual'
        ? `Connecting to ${resolveConfig().signalingUrl}`
        : `Connecting (${source}) to ${resolveConfig().signalingUrl}`
    );
    transport.connect(resolveConfig().signalingUrl);
  };

  const connect = (): void => {
    connectInternal(true, 'manual');
  };

  const disconnect = (): void => {
    manuallyDisconnected = true;
    clearReconnectTimer();
    transport.disconnect();
    recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'manual_disconnect');
    applyEvent({ kind: 'manualDisconnected' });
    appendLog('Manual disconnect');
  };

  const ping = (): void => {
    const frame: ProtocolFrame = { kind: 'ping', sentAtMs: Date.now() };
    transport.send(frame);
    appendLog('Ping sent');
  };

  function triggerFallback(reason: string): void {
    if (state.value.fallback.active) return;
    clearReconnectTimer();
    applyEvent({ kind: 'fallbackActivated', reason });
    appendLog(`Fallback activated: ${reason}`);
    transport.disconnect();
  }

  const recoverFallback = (): void => {
    recordTelemetry(MeetingTelemetryCategory.ConnectionLifecycle, 'fallback_recovery_requested');
    applyEvent({ kind: 'fallbackRecovered' });
    appendLog('Fallback recovered; reconnecting');
    connectInternal(true, 'fallback_recovery');
  };

  return {
    state,
    log,
    canPing: computed(
      () =>
        state.value.connectionPhase === ConnectionPhase.Connected ||
        state.value.connectionPhase === ConnectionPhase.Degraded
    ),
    connect,
    disconnect,
    ping,
    triggerFallback,
    recoverFallback,
    updateConfig
  };
}
