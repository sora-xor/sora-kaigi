<script setup lang="ts">
import { computed, reactive } from 'vue';
import { ConnectionPhase, type MeetingConfig } from './session/types';
import { useMeetingSession } from './session/useMeetingSession';

const config = reactive<MeetingConfig>({
  signalingUrl: 'ws://127.0.0.1:9000',
  fallbackUrl: 'https://example.com/kaigi-fallback',
  roomId: 'ga-room',
  participantId: 'web-guest-1',
  participantName: 'Web Guest',
  walletIdentity: '',
  requireSignedModeration: true,
  requirePaymentSettlement: false,
  preferWebFallbackOnPolicyFailure: true,
  supportsHdrCapture: true,
  supportsHdrRender: true
});

const session = useMeetingSession(() => ({
  ...config,
  walletIdentity: config.walletIdentity || undefined
}));

const phaseClass = computed(() => {
  switch (session.state.value.connectionPhase) {
    case ConnectionPhase.Connected:
      return 'connected';
    case ConnectionPhase.Degraded:
    case ConnectionPhase.FallbackActive:
      return 'degraded';
    case ConnectionPhase.Error:
      return 'error';
    default:
      return '';
  }
});
</script>

<template>
  <div class="shell" data-testid="kaigi.app.shell">
    <h1 data-testid="kaigi.header.title">Kaigi Web Client</h1>
    <div class="cards">
      <section class="card">
        <h2>Session Config</h2>
        <label>
          Signaling URL
          <input v-model="config.signalingUrl" type="url" data-testid="kaigi.config.signaling_url" />
        </label>
        <label>
          Fallback URL
          <input v-model="config.fallbackUrl" type="url" data-testid="kaigi.config.fallback_url" />
        </label>
        <label>
          Room ID
          <input v-model="config.roomId" type="text" data-testid="kaigi.config.room_id" />
        </label>
        <label>
          Participant ID
          <input v-model="config.participantId" type="text" data-testid="kaigi.config.participant_id" />
        </label>
        <label>
          Display Name
          <input v-model="config.participantName" type="text" data-testid="kaigi.config.participant_name" />
        </label>
        <label>
          Wallet Identity
          <input v-model="config.walletIdentity" type="text" data-testid="kaigi.config.wallet_identity" />
        </label>
        <div class="row">
          <label><input v-model="config.requireSignedModeration" type="checkbox" data-testid="kaigi.config.require_signed_moderation" /> Signed moderation</label>
          <label><input v-model="config.requirePaymentSettlement" type="checkbox" data-testid="kaigi.config.require_payment_settlement" /> Require settlement</label>
          <label><input v-model="config.preferWebFallbackOnPolicyFailure" type="checkbox" data-testid="kaigi.config.prefer_web_fallback" /> Fallback on policy fail</label>
        </div>
      </section>

      <section class="card">
        <h2>Controls</h2>
        <div class="row">
          <button data-testid="kaigi.controls.connect" @click="session.connect">Connect</button>
          <button data-testid="kaigi.controls.ping" class="secondary" @click="session.ping" :disabled="!session.canPing.value">Ping</button>
          <button data-testid="kaigi.controls.disconnect" class="warn" @click="session.disconnect">Disconnect</button>
          <button data-testid="kaigi.controls.trigger_fallback" class="secondary" @click="session.triggerFallback('manual-drill')">Fallback Drill</button>
          <button data-testid="kaigi.controls.recover" class="secondary" @click="session.recoverFallback">Recover</button>
        </div>

        <div class="state" style="margin-top: 0.9rem" data-testid="kaigi.state.card">
          <div data-testid="kaigi.state.phase_row">
            Phase:
            <span class="badge" :class="phaseClass" data-testid="kaigi.state.phase">{{ session.state.value.connectionPhase }}</span>
          </div>
          <div data-testid="kaigi.state.handshake">Handshake: {{ session.state.value.handshakeComplete ? 'complete' : 'pending' }}</div>
          <div data-testid="kaigi.state.participants">Participants: {{ Object.keys(session.state.value.participants).length }}</div>
          <div data-testid="kaigi.state.presence_seq">Presence Seq: {{ session.state.value.presenceSequence }}</div>
          <div data-testid="kaigi.state.media">Media: {{ session.state.value.mediaProfile.negotiatedProfile }} (pref={{ session.state.value.mediaProfile.preferredProfile }})</div>
          <div data-testid="kaigi.state.recording">Recording: {{ session.state.value.recordingNotice }}</div>
          <div data-testid="kaigi.state.e2ee">E2EE epoch/ack: {{ session.state.value.e2eeState.currentEpoch }}/{{ session.state.value.e2eeState.lastAckEpoch }}</div>
          <div data-testid="kaigi.state.payment">Payment: {{ session.state.value.paymentState.settlementStatus }}</div>
          <div data-testid="kaigi.state.fallback_rto">Fallback RTO: {{ session.state.value.fallback.lastRtoMs ?? 'n/a' }} ms</div>
          <div v-if="session.state.value.lastError" data-testid="kaigi.state.last_error">
            Last error: {{ session.state.value.lastError.category }} / {{ session.state.value.lastError.code }}
          </div>
        </div>
      </section>

      <section class="card">
        <h2>Event Log</h2>
        <ol class="log" data-testid="kaigi.log.list">
          <li v-for="entry in session.log.value" :key="entry.id">
            {{ entry.at }} {{ entry.message }}
          </li>
        </ol>
      </section>
    </div>
  </div>
</template>
