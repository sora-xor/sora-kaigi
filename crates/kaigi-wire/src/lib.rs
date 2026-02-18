use anyhow::{Context as _, Result, anyhow};
use norito::derive::{NoritoDeserialize, NoritoSerialize};

/// A framed message carried over a Kaigi QUIC stream.
///
/// Kaigi streams are byte streams; do not assume WebSocket or QUIC chunk boundaries
/// align to message boundaries. Frames are therefore length-prefixed on the wire.
#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum KaigiFrame {
    /// Anonymous-mode hello frame. Carries an opaque participant handle and DH public key.
    AnonHello(AnonHelloFrame),
    /// Anonymous-mode roster with opaque handles and keying material only.
    AnonRoster(AnonRosterFrame),
    /// Anonymous-mode key update (initial announce + rekey).
    GroupKeyUpdate(GroupKeyUpdateFrame),
    /// Anonymous-mode encrypted control payload fan-out.
    EncryptedControl(EncryptedControlFrame),
    /// Anonymous-mode escrow proof heartbeat (amount-hidden).
    EscrowProof(EscrowProofFrame),
    /// Hub acknowledgement for `EscrowProof`.
    EscrowAck(EscrowAckFrame),
    Hello(HelloFrame),
    /// Snapshot of the currently-known participant roster.
    Roster(RosterFrame),
    /// A room-scoped event such as join/leave/state change.
    Event(RoomEventFrame),
    /// Room-scoped configuration (rate, host, etc).
    RoomConfig(RoomConfigFrame),
    /// Host-to-hub request to update room configuration.
    RoomConfigUpdate(RoomConfigUpdateFrame),
    /// Host moderation actions (mute/stop video/stop share/kick).
    Moderation(ModerationFrame),
    /// Signed host/co-host moderation action (vNext).
    ModerationSigned(ModerationSignedFrame),
    /// Role assignment and policy elevation (vNext).
    RoleGrant(RoleGrantFrame),
    /// Role revocation and policy downgrade (vNext).
    RoleRevoke(RoleRevokeFrame),
    /// Effective permission snapshot for a participant (vNext).
    PermissionsSnapshot(PermissionsSnapshotFrame),
    /// Authoritative room policy state (vNext).
    SessionPolicy(SessionPolicyFrame),
    /// Declared endpoint device/media capabilities (vNext).
    DeviceCapability(DeviceCapabilityFrame),
    /// Negotiated media profile between SDR/HDR paths (vNext).
    MediaProfileNegotiation(MediaProfileNegotiationFrame),
    /// Recording state transition notice (vNext).
    RecordingNotice(RecordingNoticeFrame),
    /// E2EE key epoch announcement (vNext).
    E2EEKeyEpoch(E2EEKeyEpochFrame),
    /// Acknowledgement of applied key epoch (vNext).
    KeyRotationAck(KeyRotationAckFrame),
    /// Delta roster updates for large rooms (vNext).
    ParticipantPresenceDelta(ParticipantPresenceDeltaFrame),
    Chat(ChatFrame),
    ParticipantState(ParticipantStateFrame),
    /// Client-to-hub payment signal (dev harness).
    ///
    /// In production this should be backed by XOR transfers or Nexus micropayments.
    Payment(PaymentFrame),
    /// Hub-to-client acknowledgement for a `Payment` frame.
    PaymentAck(PaymentAckFrame),
    Ping(PingFrame),
    Pong(PongFrame),
    Error(ErrorFrame),
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct AnonHelloFrame {
    pub protocol_version: u16,
    /// Opaque participant handle (never an account id).
    pub participant_handle: String,
    /// X25519 public key encoded as lower-case hex (32 bytes).
    pub x25519_pubkey_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct AnonRosterFrame {
    pub at_ms: u64,
    pub participants: Vec<AnonRosterEntry>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct AnonRosterEntry {
    pub participant_handle: String,
    pub x25519_pubkey_hex: String,
    pub joined_at_ms: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct GroupKeyUpdateFrame {
    pub sent_at_ms: u64,
    pub participant_handle: String,
    pub x25519_pubkey_hex: String,
    pub epoch: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum EncryptedControlKind {
    Chat,
    ParticipantState,
    Moderation,
    Command,
    EscrowHeartbeat,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct EncryptedRecipientPayload {
    pub recipient_handle: String,
    /// XChaCha20-Poly1305 nonce encoded as lower-case hex (24 bytes).
    pub nonce_hex: String,
    /// Ciphertext + tag encoded as lower-case hex.
    pub ciphertext_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct EncryptedControlFrame {
    pub sent_at_ms: u64,
    pub sender_handle: String,
    pub epoch: u64,
    pub kind: EncryptedControlKind,
    pub payloads: Vec<EncryptedRecipientPayload>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct EscrowProofFrame {
    pub sent_at_ms: u64,
    pub payer_handle: String,
    pub escrow_id: String,
    /// Opaque proof bytes encoded as lower-case hex.
    pub proof_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct EscrowAckFrame {
    pub received_at_ms: u64,
    pub escrow_id: String,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct HelloFrame {
    pub protocol_version: u16,
    pub participant_id: String,
    pub display_name: Option<String>,
    /// MUST default to `false` when joining a call.
    pub mic_enabled: bool,
    /// MUST default to `false` when joining a call.
    pub video_enabled: bool,
    pub screen_share_enabled: bool,
    /// Hint from the client: the display can render HDR content.
    pub hdr_display: bool,
    /// Hint from the client: the camera/capture pipeline can produce HDR frames.
    pub hdr_capture: bool,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ChatFrame {
    pub sent_at_ms: u64,
    pub from_participant_id: String,
    pub from_display_name: Option<String>,
    pub text: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RosterFrame {
    pub at_ms: u64,
    pub participants: Vec<RosterEntry>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RosterEntry {
    pub participant_id: String,
    pub display_name: Option<String>,
    pub mic_enabled: bool,
    pub video_enabled: bool,
    pub screen_share_enabled: bool,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum RoomEventFrame {
    Joined(ParticipantSnapshot),
    Left(ParticipantLeftFrame),
    StateUpdated(ParticipantSnapshot),
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ParticipantSnapshot {
    pub at_ms: u64,
    pub participant_id: String,
    pub display_name: Option<String>,
    pub mic_enabled: bool,
    pub video_enabled: bool,
    pub screen_share_enabled: bool,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ParticipantLeftFrame {
    pub at_ms: u64,
    pub participant_id: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ParticipantStateFrame {
    pub updated_at_ms: u64,
    pub mic_enabled: Option<bool>,
    pub video_enabled: Option<bool>,
    pub screen_share_enabled: Option<bool>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RoomConfigFrame {
    pub updated_at_ms: u64,
    /// Current host participant id (if known).
    pub host_participant_id: Option<String>,
    /// Call rate in nano-XOR per minute.
    pub rate_per_minute_nano: u64,
    /// Billing grace window (seconds).
    pub billing_grace_secs: u64,
    /// Maximum simultaneous screen shares allowed.
    pub max_screen_shares: u8,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RoomConfigUpdateFrame {
    pub updated_at_ms: u64,
    pub rate_per_minute_nano: Option<u64>,
    pub max_screen_shares: Option<u8>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ModerationFrame {
    pub sent_at_ms: u64,
    pub target: ModerationTarget,
    pub action: ModerationAction,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ModerationSignedFrame {
    pub sent_at_ms: u64,
    pub target: ModerationTarget,
    pub action: ModerationAction,
    pub issued_by: String,
    pub signature_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum RoleKind {
    Host,
    CoHost,
    Participant,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RoleGrantFrame {
    pub issued_at_ms: u64,
    pub target_participant_id: String,
    pub role: RoleKind,
    pub granted_by: String,
    pub signature_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RoleRevokeFrame {
    pub issued_at_ms: u64,
    pub target_participant_id: String,
    pub role: RoleKind,
    pub revoked_by: String,
    pub signature_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct PermissionsSnapshotFrame {
    pub at_ms: u64,
    pub participant_id: String,
    pub host: bool,
    pub co_host: bool,
    pub can_moderate: bool,
    pub can_record_local: bool,
    pub epoch: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct SessionPolicyFrame {
    pub updated_at_ms: u64,
    pub room_lock: bool,
    pub waiting_room_enabled: bool,
    pub guest_join_allowed: bool,
    pub local_recording_allowed: bool,
    pub e2ee_required: bool,
    pub max_participants: u32,
    pub policy_epoch: u64,
    pub updated_by: String,
    pub signature_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct DeviceCapabilityFrame {
    pub reported_at_ms: u64,
    pub participant_id: String,
    pub codecs: Vec<String>,
    pub hdr_capture: bool,
    pub hdr_render: bool,
    pub max_video_streams: u16,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum MediaProfileKind {
    Sdr,
    Hdr,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct MediaProfileNegotiationFrame {
    pub at_ms: u64,
    pub participant_id: String,
    pub requested_profile: MediaProfileKind,
    pub negotiated_profile: MediaProfileKind,
    pub codec: String,
    pub epoch: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum RecordingState {
    Started,
    Stopped,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RecordingNoticeFrame {
    pub at_ms: u64,
    pub participant_id: String,
    pub state: RecordingState,
    pub local_recording: bool,
    pub policy_basis: Option<String>,
    pub issued_by: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct E2EEKeyEpochFrame {
    pub sent_at_ms: u64,
    pub participant_id: String,
    pub epoch: u64,
    pub public_key_hex: String,
    pub signature_hex: String,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct KeyRotationAckFrame {
    pub received_at_ms: u64,
    pub participant_id: String,
    pub ack_epoch: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct RoleChangeEntry {
    pub participant_id: String,
    pub role: RoleKind,
    pub granted: bool,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ParticipantPresenceDeltaFrame {
    pub at_ms: u64,
    pub sequence: u64,
    pub joined: Vec<ParticipantSnapshot>,
    pub left: Vec<ParticipantLeftFrame>,
    pub role_changes: Vec<RoleChangeEntry>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum ModerationTarget {
    All,
    Participant(String),
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub enum ModerationAction {
    Kick,
    DisableMic,
    DisableVideo,
    DisableScreenShare,
    AdmitFromWaiting,
    DenyFromWaiting,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct PaymentFrame {
    pub sent_at_ms: u64,
    /// Amount paid in nano-XOR (1e-9 XOR units).
    pub amount_nano_xor: u64,
    /// Optional on-ledger transaction hash (hex) for reconciliation.
    pub tx_hash_hex: Option<String>,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct PaymentAckFrame {
    pub received_at_ms: u64,
    pub amount_nano_xor: u64,
    pub total_paid_nano_xor: u64,
    pub total_billed_nano_xor: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct PingFrame {
    pub nonce: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct PongFrame {
    pub nonce: u64,
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, PartialEq, Eq, NoritoSerialize, NoritoDeserialize)]
pub struct ErrorFrame {
    pub message: String,
}

pub const PROTOCOL_VERSION: u16 = 5;
pub const MAX_FRAME_LEN: usize = 256 * 1024;
pub const MAX_ANON_PARTICIPANT_HANDLE_LEN: usize = 128;
pub const MAX_ESCROW_ID_LEN: usize = 128;
pub const MAX_ESCROW_PROOF_HEX_LEN: usize = 64 * 1024;

/// Encode a single framed message: `u32(be) len` + `payload`.
pub fn encode_framed(frame: &KaigiFrame) -> Result<Vec<u8>> {
    let payload = norito::to_bytes(frame).context("norito encode")?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(anyhow!(
            "frame too large: {} bytes (max {})",
            payload.len(),
            MAX_FRAME_LEN
        ));
    }
    let len = u32::try_from(payload.len()).expect("MAX_FRAME_LEN fits in u32");
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Streaming decoder for framed Kaigi messages.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn try_next(&mut self) -> Result<Option<KaigiFrame>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[0..4].try_into().expect("len slice")) as usize;
        if len == 0 {
            self.buf.drain(..4);
            return Ok(Some(KaigiFrame::Error(ErrorFrame {
                message: "zero-length frame".to_string(),
            })));
        }
        if len > MAX_FRAME_LEN {
            return Err(anyhow!(
                "declared frame length {len} exceeds MAX_FRAME_LEN {MAX_FRAME_LEN}"
            ));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }

        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        let frame: KaigiFrame = norito::decode_from_bytes(&payload).context("norito decode")?;
        Ok(Some(frame))
    }

    pub fn buffer_len(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip_single_chunk() {
        let frame = KaigiFrame::Chat(ChatFrame {
            sent_at_ms: 123,
            from_participant_id: "p-1".to_string(),
            from_display_name: Some("Alice".to_string()),
            text: "hello".to_string(),
        });
        let bytes = encode_framed(&frame).expect("encode");
        let mut dec = FrameDecoder::new();
        dec.push(&bytes);
        let out = dec.try_next().expect("decode").expect("some");
        assert_eq!(out, frame);
        assert_eq!(dec.buffer_len(), 0);
    }

    #[test]
    fn decode_handles_partial_chunks() {
        let frame = KaigiFrame::Ping(PingFrame { nonce: 42 });
        let bytes = encode_framed(&frame).expect("encode");
        let split = bytes.len() / 2;
        let mut dec = FrameDecoder::new();
        dec.push(&bytes[..split]);
        assert!(dec.try_next().expect("decode").is_none());
        dec.push(&bytes[split..]);
        let out = dec.try_next().expect("decode").expect("some");
        assert_eq!(out, frame);
        assert_eq!(dec.buffer_len(), 0);
    }

    #[test]
    fn decode_rejects_oversized_declared_length() {
        let mut dec = FrameDecoder::new();
        let len = (MAX_FRAME_LEN as u32 + 1).to_be_bytes();
        dec.push(&len);
        let err = dec.try_next().expect_err("expected error");
        assert!(
            err.to_string().contains("exceeds MAX_FRAME_LEN"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn decode_zero_length_emits_error_frame() {
        let mut dec = FrameDecoder::new();
        dec.push(&0u32.to_be_bytes());
        let out = dec.try_next().expect("decode").expect("some");
        assert_eq!(
            out,
            KaigiFrame::Error(ErrorFrame {
                message: "zero-length frame".to_string()
            })
        );
    }

    #[test]
    fn decode_rejects_malformed_payload_bytes() {
        let mut dec = FrameDecoder::new();
        let payload = vec![0xde, 0xad, 0xbe, 0xef];
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&payload);
        dec.push(&framed);
        let err = dec.try_next().expect_err("expected norito decode failure");
        assert!(
            err.to_string().contains("norito decode"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn legacy_frames_roundtrip_with_vnext_decoder() {
        let frames = vec![
            KaigiFrame::Hello(HelloFrame {
                protocol_version: PROTOCOL_VERSION,
                participant_id: "legacy-host@sora".to_string(),
                display_name: Some("Legacy Host".to_string()),
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                hdr_display: false,
                hdr_capture: false,
            }),
            KaigiFrame::Chat(ChatFrame {
                sent_at_ms: 10,
                from_participant_id: "legacy-host@sora".to_string(),
                from_display_name: Some("Legacy Host".to_string()),
                text: "legacy-chat".to_string(),
            }),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 11,
                mic_enabled: Some(true),
                video_enabled: Some(false),
                screen_share_enabled: Some(false),
            }),
            KaigiFrame::RoomConfigUpdate(RoomConfigUpdateFrame {
                updated_at_ms: 12,
                rate_per_minute_nano: Some(1_000),
                max_screen_shares: Some(2),
            }),
            KaigiFrame::Moderation(ModerationFrame {
                sent_at_ms: 13,
                target: ModerationTarget::All,
                action: ModerationAction::DisableMic,
            }),
            KaigiFrame::Payment(PaymentFrame {
                sent_at_ms: 14,
                amount_nano_xor: 5_000,
                tx_hash_hex: Some("11".repeat(32)),
            }),
            KaigiFrame::Ping(PingFrame { nonce: 99 }),
        ];

        for frame in frames {
            let bytes = encode_framed(&frame).expect("encode");
            let mut dec = FrameDecoder::new();
            dec.push(&bytes);
            let out = dec.try_next().expect("decode").expect("frame");
            assert_eq!(out, frame);
            assert_eq!(dec.buffer_len(), 0);
        }
    }

    #[test]
    fn anon_hello_roundtrip() {
        let frame = KaigiFrame::AnonHello(AnonHelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_handle: "h-001".to_string(),
            x25519_pubkey_hex: "11".repeat(32),
        });
        let bytes = encode_framed(&frame).expect("encode");
        let mut dec = FrameDecoder::new();
        dec.push(&bytes);
        let out = dec.try_next().expect("decode").expect("frame");
        assert_eq!(out, frame);
    }

    #[test]
    fn encrypted_control_roundtrip() {
        let frame = KaigiFrame::EncryptedControl(EncryptedControlFrame {
            sent_at_ms: 42,
            sender_handle: "h-abc".to_string(),
            epoch: 1,
            kind: EncryptedControlKind::Chat,
            payloads: vec![
                EncryptedRecipientPayload {
                    recipient_handle: "h-abc".to_string(),
                    nonce_hex: "22".repeat(24),
                    ciphertext_hex: "33".repeat(48),
                },
                EncryptedRecipientPayload {
                    recipient_handle: "h-def".to_string(),
                    nonce_hex: "44".repeat(24),
                    ciphertext_hex: "55".repeat(48),
                },
            ],
        });
        let bytes = encode_framed(&frame).expect("encode");
        let mut dec = FrameDecoder::new();
        dec.push(&bytes);
        let out = dec.try_next().expect("decode").expect("frame");
        assert_eq!(out, frame);
    }

    #[test]
    fn anon_roster_and_key_update_roundtrip() {
        let roster = KaigiFrame::AnonRoster(AnonRosterFrame {
            at_ms: 100,
            participants: vec![
                AnonRosterEntry {
                    participant_handle: "h-1".to_string(),
                    x25519_pubkey_hex: "aa".repeat(32),
                    joined_at_ms: 90,
                },
                AnonRosterEntry {
                    participant_handle: "h-2".to_string(),
                    x25519_pubkey_hex: "bb".repeat(32),
                    joined_at_ms: 95,
                },
            ],
        });
        let key_update = KaigiFrame::GroupKeyUpdate(GroupKeyUpdateFrame {
            sent_at_ms: 101,
            participant_handle: "h-1".to_string(),
            x25519_pubkey_hex: "cc".repeat(32),
            epoch: 2,
        });

        for frame in [roster, key_update] {
            let bytes = encode_framed(&frame).expect("encode");
            let mut dec = FrameDecoder::new();
            dec.push(&bytes);
            let out = dec.try_next().expect("decode").expect("frame");
            assert_eq!(out, frame);
        }
    }

    #[test]
    fn escrow_frames_roundtrip() {
        let proof = KaigiFrame::EscrowProof(EscrowProofFrame {
            sent_at_ms: 200,
            payer_handle: "h-escrow".to_string(),
            escrow_id: "escrow-42".to_string(),
            proof_hex: "de".repeat(32),
        });
        let ack = KaigiFrame::EscrowAck(EscrowAckFrame {
            received_at_ms: 201,
            escrow_id: "escrow-42".to_string(),
            accepted: false,
            reason: Some("stale proof".to_string()),
        });

        for frame in [proof, ack] {
            let bytes = encode_framed(&frame).expect("encode");
            let mut dec = FrameDecoder::new();
            dec.push(&bytes);
            let out = dec.try_next().expect("decode").expect("frame");
            assert_eq!(out, frame);
        }
    }

    #[test]
    fn vnext_frames_roundtrip() {
        let frames = vec![
            KaigiFrame::ModerationSigned(ModerationSignedFrame {
                sent_at_ms: 0,
                target: ModerationTarget::All,
                action: ModerationAction::DisableMic,
                issued_by: "p-1".to_string(),
                signature_hex: "aa".repeat(32),
            }),
            KaigiFrame::RoleGrant(RoleGrantFrame {
                issued_at_ms: 1,
                target_participant_id: "p-2".to_string(),
                role: RoleKind::CoHost,
                granted_by: "p-1".to_string(),
                signature_hex: "ab".repeat(32),
            }),
            KaigiFrame::RoleRevoke(RoleRevokeFrame {
                issued_at_ms: 2,
                target_participant_id: "p-2".to_string(),
                role: RoleKind::CoHost,
                revoked_by: "p-1".to_string(),
                signature_hex: "cd".repeat(32),
            }),
            KaigiFrame::PermissionsSnapshot(PermissionsSnapshotFrame {
                at_ms: 3,
                participant_id: "p-2".to_string(),
                host: false,
                co_host: true,
                can_moderate: true,
                can_record_local: true,
                epoch: 9,
            }),
            KaigiFrame::SessionPolicy(SessionPolicyFrame {
                updated_at_ms: 4,
                room_lock: false,
                waiting_room_enabled: true,
                guest_join_allowed: true,
                local_recording_allowed: true,
                e2ee_required: true,
                max_participants: 500,
                policy_epoch: 3,
                updated_by: "p-1".to_string(),
                signature_hex: "ef".repeat(32),
            }),
            KaigiFrame::DeviceCapability(DeviceCapabilityFrame {
                reported_at_ms: 5,
                participant_id: "p-2".to_string(),
                codecs: vec!["av1".to_string(), "h265".to_string()],
                hdr_capture: true,
                hdr_render: true,
                max_video_streams: 4,
            }),
            KaigiFrame::MediaProfileNegotiation(MediaProfileNegotiationFrame {
                at_ms: 6,
                participant_id: "p-2".to_string(),
                requested_profile: MediaProfileKind::Hdr,
                negotiated_profile: MediaProfileKind::Sdr,
                codec: "av1".to_string(),
                epoch: 10,
            }),
            KaigiFrame::RecordingNotice(RecordingNoticeFrame {
                at_ms: 7,
                participant_id: "p-3".to_string(),
                state: RecordingState::Started,
                local_recording: true,
                policy_basis: Some("host-allowed".to_string()),
                issued_by: "p-1".to_string(),
            }),
            KaigiFrame::E2EEKeyEpoch(E2EEKeyEpochFrame {
                sent_at_ms: 8,
                participant_id: "p-3".to_string(),
                epoch: 11,
                public_key_hex: "01".repeat(32),
                signature_hex: "23".repeat(32),
            }),
            KaigiFrame::KeyRotationAck(KeyRotationAckFrame {
                received_at_ms: 9,
                participant_id: "p-4".to_string(),
                ack_epoch: 11,
            }),
            KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                at_ms: 10,
                sequence: 12,
                joined: vec![ParticipantSnapshot {
                    at_ms: 10,
                    participant_id: "p-5".to_string(),
                    display_name: Some("Dana".to_string()),
                    mic_enabled: false,
                    video_enabled: false,
                    screen_share_enabled: false,
                }],
                left: vec![ParticipantLeftFrame {
                    at_ms: 10,
                    participant_id: "p-6".to_string(),
                }],
                role_changes: vec![RoleChangeEntry {
                    participant_id: "p-2".to_string(),
                    role: RoleKind::CoHost,
                    granted: true,
                }],
            }),
        ];

        for frame in frames {
            let bytes = encode_framed(&frame).expect("encode");
            let mut dec = FrameDecoder::new();
            dec.push(&bytes);
            let out = dec.try_next().expect("decode").expect("frame");
            assert_eq!(out, frame);
        }
    }
}
