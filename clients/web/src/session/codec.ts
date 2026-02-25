import {
  GuestPolicy,
  MediaProfile,
  ModerationAction,
  ParticipantRole,
  PaymentSettlementStatus,
  RecordingState,
  SessionErrorCategory,
  type ProtocolFrame
} from './types';

function frameKindAlias(rawKind: string): ProtocolFrame['kind'] {
  switch (rawKind) {
    case 'handshake':
      return 'handshake';
    case 'handshake_ack':
    case 'handshakeAck':
      return 'handshakeAck';
    case 'participant_presence_delta':
    case 'participantPresenceDelta':
      return 'participantPresenceDelta';
    case 'role_grant':
    case 'roleGrant':
      return 'roleGrant';
    case 'role_revoke':
    case 'roleRevoke':
      return 'roleRevoke';
    case 'permissions_snapshot':
    case 'permissionsSnapshot':
      return 'permissionsSnapshot';
    case 'moderation_signed':
    case 'moderationSigned':
      return 'moderationSigned';
    case 'session_policy':
    case 'sessionPolicy':
      return 'sessionPolicy';
    case 'device_capability':
    case 'deviceCapability':
      return 'deviceCapability';
    case 'media_profile_negotiation':
    case 'mediaProfileNegotiation':
      return 'mediaProfileNegotiation';
    case 'recording_notice':
    case 'recordingNotice':
      return 'recordingNotice';
    case 'e2ee_key_epoch':
    case 'e2eeKeyEpoch':
      return 'e2eeKeyEpoch';
    case 'key_rotation_ack':
    case 'keyRotationAck':
      return 'keyRotationAck';
    case 'payment_policy':
    case 'paymentPolicy':
      return 'paymentPolicy';
    case 'payment_settlement':
    case 'paymentSettlement':
      return 'paymentSettlement';
    case 'ping':
      return 'ping';
    case 'pong':
      return 'pong';
    default:
      return 'error';
  }
}

function payloadOrRoot(raw: Record<string, unknown>, ...keys: string[]): Record<string, unknown> {
  for (const key of keys) {
    const value = raw[key];
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return value as Record<string, unknown>;
    }
  }
  return raw;
}

function categoryAlias(raw: unknown): SessionErrorCategory {
  if (raw === 'policyFailure' || raw === 'policy_failure') return SessionErrorCategory.PolicyFailure;
  if (raw === 'transportFailure' || raw === 'transport_failure') return SessionErrorCategory.TransportFailure;
  return SessionErrorCategory.ProtocolFailure;
}

function roleAlias(raw: unknown): ParticipantRole {
  if (raw === 'coHost' || raw === 'co_host') return ParticipantRole.CoHost;
  if (raw === 'host') return ParticipantRole.Host;
  if (raw === 'guest') return ParticipantRole.Guest;
  return ParticipantRole.Participant;
}

function moderationAlias(raw: unknown): ModerationAction {
  switch (raw) {
    case 'videoOff':
    case 'video_off':
      return ModerationAction.VideoOff;
    case 'stopShare':
    case 'stop_share':
      return ModerationAction.StopShare;
    case 'kick':
      return ModerationAction.Kick;
    case 'admitFromWaiting':
    case 'admit_from_waiting':
      return ModerationAction.AdmitFromWaiting;
    case 'denyFromWaiting':
    case 'deny_from_waiting':
      return ModerationAction.DenyFromWaiting;
    default:
      return ModerationAction.Mute;
  }
}

function mediaProfileAlias(raw: unknown): MediaProfile {
  return raw === 'hdr' ? MediaProfile.HDR : MediaProfile.SDR;
}

function recordingAlias(raw: unknown): RecordingState {
  return raw === 'started' ? RecordingState.Started : RecordingState.Stopped;
}

function guestPolicyAlias(raw: unknown): GuestPolicy {
  if (raw === 'inviteOnly' || raw === 'invite_only') return GuestPolicy.InviteOnly;
  if (raw === 'blocked') return GuestPolicy.Blocked;
  return GuestPolicy.Open;
}

function paymentAlias(raw: unknown): PaymentSettlementStatus {
  if (raw === 'pending') return PaymentSettlementStatus.Pending;
  if (raw === 'settled') return PaymentSettlementStatus.Settled;
  if (raw === 'blocked') return PaymentSettlementStatus.Blocked;
  return PaymentSettlementStatus.NotRequired;
}

export function decodeFrame(payload: string): ProtocolFrame {
  const raw = JSON.parse(payload) as Record<string, unknown>;
  const kind = frameKindAlias(String(raw.kind ?? 'error'));

  switch (kind) {
    case 'handshakeAck':
      {
        const value = payloadOrRoot(raw, 'handshakeAck', 'handshake_ack');
        return {
        kind,
        sessionId: String(value.sessionId ?? value.session_id ?? 'session'),
        resumeToken: String(value.resumeToken ?? value.resume_token ?? ''),
        acceptedAtMs: Number(value.acceptedAtMs ?? value.accepted_at_ms ?? Date.now())
      };
      }

    case 'participantPresenceDelta':
      {
        const value = payloadOrRoot(raw, 'presenceDelta', 'participantPresenceDelta', 'participant_presence_delta');
        return {
        kind,
        joined: Array.isArray(value.joined)
          ? value.joined.map((p) => {
              const participant = p as Record<string, unknown>;
              return {
                id: String(participant.id ?? ''),
                displayName: String(participant.displayName ?? participant.display_name ?? ''),
                role: roleAlias(participant.role),
                muted: Boolean(participant.muted),
                videoEnabled: participant.videoEnabled !== false && participant.video_enabled !== false,
                shareEnabled: participant.shareEnabled !== false && participant.share_enabled !== false,
                waitingRoom: Boolean(participant.waitingRoom ?? participant.waiting_room)
              };
            })
          : [],
        left: Array.isArray(value.left) ? value.left.map((v) => String(v)) : [],
        roleChanges: Array.isArray(value.roleChanges ?? value.role_changes)
          ? ((value.roleChanges ?? value.role_changes) as unknown[]).map((change) => {
              const value = change as Record<string, unknown>;
              return {
                participantId: String(value.participantId ?? value.participant_id ?? ''),
                role: roleAlias(value.role)
              };
            })
          : [],
        sequence: Number(value.sequence ?? 0)
      };
      }

    case 'roleGrant':
      {
        const value = payloadOrRoot(raw, 'roleGrant', 'role_grant');
        return {
        kind,
        targetParticipantId: String(value.targetParticipantId ?? value.target_participant_id ?? ''),
        role: roleAlias(value.role),
        grantedBy: String(value.grantedBy ?? value.granted_by ?? ''),
        signature: typeof value.signature === 'string' ? value.signature : undefined,
        issuedAtMs: Number(value.issuedAtMs ?? value.issued_at_ms ?? Date.now())
      };
      }

    case 'roleRevoke':
      {
        const value = payloadOrRoot(raw, 'roleRevoke', 'role_revoke');
        return {
        kind,
        targetParticipantId: String(value.targetParticipantId ?? value.target_participant_id ?? ''),
        role: roleAlias(value.role),
        revokedBy: String(value.revokedBy ?? value.revoked_by ?? ''),
        signature: typeof value.signature === 'string' ? value.signature : undefined,
        issuedAtMs: Number(value.issuedAtMs ?? value.issued_at_ms ?? Date.now())
      };
      }

    case 'permissionsSnapshot':
      {
        const value = payloadOrRoot(raw, 'permissionsSnapshot', 'permissions_snapshot');
        return {
        kind,
        participantId: String(value.participantId ?? value.participant_id ?? ''),
        effectivePermissions: Array.isArray(value.effectivePermissions ?? value.effective_permissions)
          ? ((value.effectivePermissions ?? value.effective_permissions) as unknown[]).map((permission) => String(permission))
          : [],
        epoch: Number(value.epoch ?? 0)
      };
      }

    case 'moderationSigned':
      {
        const value = payloadOrRoot(raw, 'moderationSigned', 'moderation_signed');
        return {
        kind,
        targetParticipantId: String(value.targetParticipantId ?? value.target_participant_id ?? ''),
        action: moderationAlias(value.action),
        issuedBy: String(value.issuedBy ?? value.issued_by ?? ''),
        signature: typeof value.signature === 'string' ? value.signature : undefined,
        sentAtMs: Number(value.sentAtMs ?? value.sent_at_ms ?? Date.now())
      };
      }

    case 'sessionPolicy':
      {
        const value = payloadOrRoot(raw, 'sessionPolicy', 'session_policy');
        return {
        kind,
        roomLock: Boolean(value.roomLock ?? value.room_lock),
        waitingRoomEnabled: Boolean(value.waitingRoomEnabled ?? value.waiting_room_enabled),
        recordingPolicy: recordingAlias(value.recordingPolicy ?? value.recording_policy),
        guestPolicy: guestPolicyAlias(value.guestPolicy ?? value.guest_policy),
        e2eeRequired: value.e2eeRequired !== false && value.e2ee_required !== false,
        maxParticipants: Number(value.maxParticipants ?? value.max_participants ?? 300),
        policyEpoch: Number(value.policyEpoch ?? value.policy_epoch ?? 0),
        updatedBy: String(value.updatedBy ?? value.updated_by ?? ''),
        signature: typeof value.signature === 'string' ? value.signature : undefined,
        updatedAtMs: Number(value.updatedAtMs ?? value.updated_at_ms ?? Date.now())
      };
      }

    case 'deviceCapability':
      {
        const value = payloadOrRoot(raw, 'deviceCapability', 'device_capability');
        return {
          kind,
          participantId: String(value.participantId ?? value.participant_id ?? ''),
          codecs: Array.isArray(value.codecs) ? value.codecs.map((codec) => String(codec)) : [],
          hdrCapture: Boolean(value.hdrCapture ?? value.hdr_capture),
          hdrRender: Boolean(value.hdrRender ?? value.hdr_render),
          maxStreams: Number(value.maxStreams ?? value.max_streams ?? 1),
          updatedAtMs: Number(value.updatedAtMs ?? value.updated_at_ms ?? Date.now())
        };
      }

    case 'mediaProfileNegotiation':
      {
        const value = payloadOrRoot(raw, 'mediaProfileNegotiation', 'media_profile_negotiation');
        return {
        kind,
        preferredProfile: mediaProfileAlias(value.preferredProfile ?? value.preferred_profile),
        negotiatedProfile: mediaProfileAlias(value.negotiatedProfile ?? value.negotiated_profile),
        colorPrimaries: String(value.colorPrimaries ?? value.color_primaries ?? 'bt709'),
        transferFunction: String(value.transferFunction ?? value.transfer_function ?? 'gamma'),
        codec: String(value.codec ?? 'h264')
      };
      }

    case 'recordingNotice':
      {
        const value = payloadOrRoot(raw, 'recordingNotice', 'recording_notice');
        return {
        kind,
        state: recordingAlias(value.state),
        issuedAtMs: Number(value.issuedAtMs ?? value.issued_at_ms ?? Date.now())
      };
      }

    case 'e2eeKeyEpoch':
      {
        const value = payloadOrRoot(raw, 'e2eeKeyEpoch', 'e2ee_key_epoch');
        return {
        kind,
        epoch: Number(value.epoch ?? 0),
        issuedBy: String(value.issuedBy ?? value.issued_by ?? ''),
        signature: typeof value.signature === 'string' ? value.signature : undefined,
        sentAtMs: Number(value.sentAtMs ?? value.sent_at_ms ?? Date.now())
      };
      }

    case 'keyRotationAck':
      {
        const value = payloadOrRoot(raw, 'keyRotationAck', 'key_rotation_ack');
        return {
        kind,
        ackEpoch: Number(value.ackEpoch ?? value.ack_epoch ?? 0),
        participantId: String(value.participantId ?? value.participant_id ?? ''),
        sentAtMs: Number(value.sentAtMs ?? value.sent_at_ms ?? Date.now())
      };
      }

    case 'paymentPolicy':
      {
        const value = payloadOrRoot(raw, 'paymentPolicy', 'payment_policy');
        return {
        kind,
        required: Boolean(value.required),
        destinationAccount: typeof value.destinationAccount === 'string'
          ? value.destinationAccount
          : typeof value.destination_account === 'string'
            ? value.destination_account
            : undefined
      };
      }

    case 'paymentSettlement':
      {
        const value = payloadOrRoot(raw, 'paymentSettlement', 'payment_settlement');
        return {
        kind,
        status: paymentAlias(value.status)
      };
      }

    case 'ping':
      {
        const value = payloadOrRoot(raw, 'ping');
        return {
        kind,
        sentAtMs: Number(value.sentAtMs ?? value.sent_at_ms ?? Date.now())
      };
      }

    case 'pong':
      {
        const value = payloadOrRoot(raw, 'pong');
        return {
          kind,
          sentAtMs: Number(value.sentAtMs ?? value.sent_at_ms ?? Date.now())
        };
      }

    case 'handshake':
      {
        const value = payloadOrRoot(raw, 'handshake');
        return {
        kind,
        roomId: String(value.roomId ?? value.room_id ?? ''),
        participantId: String(value.participantId ?? value.participant_id ?? ''),
        participantName: String(value.participantName ?? value.participant_name ?? ''),
        walletIdentity: typeof value.walletIdentity === 'string' ? value.walletIdentity : undefined,
        resumeToken: typeof value.resumeToken === 'string' ? value.resumeToken : undefined,
        preferredProfile: mediaProfileAlias(value.preferredProfile),
        hdrCapture: Boolean(value.hdrCapture),
        hdrRender: Boolean(value.hdrRender),
        sentAtMs: Number(value.sentAtMs ?? Date.now())
      };
      }

    default:
      return {
        kind: 'error',
        category: categoryAlias(raw.category),
        code: String(raw.code ?? 'unknown_error'),
        message: String(raw.message ?? 'Unknown protocol frame')
      };
  }
}

export function encodeFrame(frame: ProtocolFrame): string {
  return JSON.stringify(frame);
}
