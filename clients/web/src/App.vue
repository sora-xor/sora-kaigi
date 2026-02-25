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
  <div class="shell">
    <h1>Kaigi Web Client</h1>
    <div class="cards">
      <section class="card">
        <h2>Session Config</h2>
        <label>
          Signaling URL
          <input v-model="config.signalingUrl" type="url" />
        </label>
        <label>
          Fallback URL
          <input v-model="config.fallbackUrl" type="url" />
        </label>
        <label>
          Room ID
          <input v-model="config.roomId" type="text" />
        </label>
        <label>
          Participant ID
          <input v-model="config.participantId" type="text" />
        </label>
        <label>
          Display Name
          <input v-model="config.participantName" type="text" />
        </label>
        <label>
          Wallet Identity
          <input v-model="config.walletIdentity" type="text" />
        </label>
        <div class="row">
          <label><input v-model="config.requireSignedModeration" type="checkbox" /> Signed moderation</label>
          <label><input v-model="config.requirePaymentSettlement" type="checkbox" /> Require settlement</label>
          <label><input v-model="config.preferWebFallbackOnPolicyFailure" type="checkbox" /> Fallback on policy fail</label>
        </div>
      </section>

      <section class="card">
        <h2>Controls</h2>
        <div class="row">
          <button @click="session.connect">Connect</button>
          <button class="secondary" @click="session.ping" :disabled="!session.canPing.value">Ping</button>
          <button class="warn" @click="session.disconnect">Disconnect</button>
          <button class="secondary" @click="session.triggerFallback('manual-drill')">Fallback Drill</button>
          <button class="secondary" @click="session.recoverFallback">Recover</button>
        </div>

        <div class="state" style="margin-top: 0.9rem">
          <div>
            Phase:
            <span class="badge" :class="phaseClass">{{ session.state.value.connectionPhase }}</span>
          </div>
          <div>Handshake: {{ session.state.value.handshakeComplete ? 'complete' : 'pending' }}</div>
          <div>Participants: {{ Object.keys(session.state.value.participants).length }}</div>
          <div>Presence Seq: {{ session.state.value.presenceSequence }}</div>
          <div>Media: {{ session.state.value.mediaProfile.negotiatedProfile }} (pref={{ session.state.value.mediaProfile.preferredProfile }})</div>
          <div>Recording: {{ session.state.value.recordingNotice }}</div>
          <div>E2EE epoch/ack: {{ session.state.value.e2eeState.currentEpoch }}/{{ session.state.value.e2eeState.lastAckEpoch }}</div>
          <div>Payment: {{ session.state.value.paymentState.settlementStatus }}</div>
          <div>Fallback RTO: {{ session.state.value.fallback.lastRtoMs ?? 'n/a' }} ms</div>
          <div v-if="session.state.value.lastError">
            Last error: {{ session.state.value.lastError.category }} / {{ session.state.value.lastError.code }}
          </div>
        </div>
      </section>

      <section class="card">
        <h2>Event Log</h2>
        <ol class="log">
          <li v-for="entry in session.log.value" :key="entry.id">
            {{ entry.at }} {{ entry.message }}
          </li>
        </ol>
      </section>
    </div>
  </div>
</template>
