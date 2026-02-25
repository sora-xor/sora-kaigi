import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame } from '../src/session/codec';
import {
  GuestPolicy,
  MediaProfile,
  ParticipantRole,
  RecordingState,
  SessionErrorCategory
} from '../src/session/types';

describe('web codec', () => {
  it('decodes snake case handshake ack aliases', () => {
    const frame = decodeFrame(JSON.stringify({
      kind: 'handshake_ack',
      session_id: 'session-1',
      resume_token: 'resume-2',
      accepted_at_ms: 42
    }));

    expect(frame.kind).toBe('handshakeAck');
    if (frame.kind !== 'handshakeAck') return;
    expect(frame.resumeToken).toBe('resume-2');
    expect(frame.sessionId).toBe('session-1');
  });

  it('decodes presence delta aliases and role aliases', () => {
    const frame = decodeFrame(JSON.stringify({
      kind: 'participant_presence_delta',
      joined: [
        {
          id: 'p1',
          display_name: 'beta',
          role: 'coHost',
          muted: false,
          video_enabled: true,
          share_enabled: true,
          waiting_room: false
        }
      ],
      left: [],
      role_changes: [{ participant_id: 'p1', role: 'host' }],
      sequence: 18
    }));

    expect(frame.kind).toBe('participantPresenceDelta');
    if (frame.kind !== 'participantPresenceDelta') return;
    expect(frame.joined[0]?.role).toBe(ParticipantRole.CoHost);
    expect(frame.roleChanges[0]?.participantId).toBe('p1');
    expect(frame.roleChanges[0]?.role).toBe(ParticipantRole.Host);
  });

  it('decodes session policy aliases', () => {
    const frame = decodeFrame(JSON.stringify({
      kind: 'session_policy',
      room_lock: true,
      waiting_room_enabled: true,
      recording_policy: 'started',
      guest_policy: 'inviteOnly',
      e2ee_required: true,
      max_participants: 40,
      policy_epoch: 3,
      updated_by: 'host',
      signature: 'sig'
    }));

    expect(frame.kind).toBe('sessionPolicy');
    if (frame.kind !== 'sessionPolicy') return;
    expect(frame.recordingPolicy).toBe(RecordingState.Started);
    expect(frame.guestPolicy).toBe(GuestPolicy.InviteOnly);
  });

  it('decodes nested device capability payload aliases', () => {
    const frame = decodeFrame(JSON.stringify({
      kind: 'device_capability',
      device_capability: {
        participant_id: 'p-cap',
        codecs: ['h264', 'vp9'],
        hdr_capture: true,
        hdr_render: false,
        max_streams: 3,
        updated_at_ms: 9001
      }
    }));

    expect(frame.kind).toBe('deviceCapability');
    if (frame.kind !== 'deviceCapability') return;
    expect(frame.participantId).toBe('p-cap');
    expect(frame.codecs).toEqual(['h264', 'vp9']);
    expect(frame.hdrCapture).toBe(true);
    expect(frame.hdrRender).toBe(false);
    expect(frame.maxStreams).toBe(3);
    expect(frame.updatedAtMs).toBe(9001);
  });

  it('decodes nested pong payload', () => {
    const frame = decodeFrame(JSON.stringify({
      kind: 'pong',
      pong: {
        sent_at_ms: 444
      }
    }));

    expect(frame.kind).toBe('pong');
    if (frame.kind !== 'pong') return;
    expect(frame.sentAtMs).toBe(444);
  });

  it('encodes and decodes media profile frames', () => {
    const payload = encodeFrame({
      kind: 'mediaProfileNegotiation',
      preferredProfile: MediaProfile.HDR,
      negotiatedProfile: MediaProfile.SDR,
      colorPrimaries: 'bt709',
      transferFunction: 'gamma',
      codec: 'h264'
    });

    const decoded = decodeFrame(payload);
    expect(decoded.kind).toBe('mediaProfileNegotiation');
    if (decoded.kind !== 'mediaProfileNegotiation') return;
    expect(decoded.preferredProfile).toBe(MediaProfile.HDR);
    expect(decoded.negotiatedProfile).toBe(MediaProfile.SDR);
  });

  it('maps unknown frame kind to error frame', () => {
    const frame = decodeFrame(JSON.stringify({ kind: 'unexpected', category: 'policyFailure', code: 'x', message: 'y' }));
    expect(frame.kind).toBe('error');
    if (frame.kind !== 'error') return;
    expect(frame.category).toBe(SessionErrorCategory.PolicyFailure);
  });

  it('rejects legacy raw join frames', () => {
    expect(() => decodeFrame('JOIN room=daily participant=alice')).toThrow();
  });
});
