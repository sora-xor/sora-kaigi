use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionPhase {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    FallbackActive,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCategory {
    ProtocolFailure,
    PolicyFailure,
    TransportFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionError {
    pub category: SessionErrorCategory,
    pub code: String,
    pub message: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantRole {
    Host,
    CoHost,
    Participant,
    Guest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    pub display_name: String,
    pub role: ParticipantRole,
    pub muted: bool,
    pub video_enabled: bool,
    pub share_enabled: bool,
    pub waiting_room: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleChange {
    pub participant_id: String,
    pub role: ParticipantRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModerationAction {
    Mute,
    VideoOff,
    StopShare,
    Kick,
    AdmitFromWaiting,
    DenyFromWaiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaProfile {
    Sdr,
    Hdr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProfileState {
    pub preferred_profile: MediaProfile,
    pub negotiated_profile: MediaProfile,
    pub color_primaries: String,
    pub transfer_function: String,
    pub codec: String,
}

impl Default for MediaProfileState {
    fn default() -> Self {
        Self {
            preferred_profile: MediaProfile::Sdr,
            negotiated_profile: MediaProfile::Sdr,
            color_primaries: "bt709".to_string(),
            transfer_function: "gamma".to_string(),
            codec: "h264".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Stopped,
    Started,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestPolicy {
    Open,
    InviteOnly,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSnapshot {
    pub effective_permissions: Vec<String>,
    pub epoch: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentSettlementStatus {
    NotRequired,
    Pending,
    Settled,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentState {
    pub required: bool,
    pub destination: Option<String>,
    pub settlement_status: PaymentSettlementStatus,
}

impl Default for PaymentState {
    fn default() -> Self {
        Self {
            required: false,
            destination: None,
            settlement_status: PaymentSettlementStatus::NotRequired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eeState {
    pub current_epoch: i32,
    pub last_ack_epoch: i32,
}

impl Default for E2eeState {
    fn default() -> Self {
        Self {
            current_epoch: 0,
            last_ack_epoch: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackState {
    pub active: bool,
    pub reason: Option<String>,
    pub activated_at_ms: Option<i64>,
    pub recovered_at_ms: Option<i64>,
    pub last_rto_ms: Option<i64>,
}

impl Default for FallbackState {
    fn default() -> Self {
        Self {
            active: false,
            reason: None,
            activated_at_ms: None,
            recovered_at_ms: None,
            last_rto_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingConfig {
    pub signaling_url: String,
    pub fallback_url: String,
    pub room_id: String,
    pub participant_id: String,
    pub participant_name: String,
    pub wallet_identity: Option<String>,
    pub require_signed_moderation: bool,
    pub require_payment_settlement: bool,
    pub prefer_web_fallback_on_policy_failure: bool,
    pub supports_hdr_capture: bool,
    pub supports_hdr_render: bool,
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            signaling_url: "ws://127.0.0.1:9000".to_string(),
            fallback_url: "https://example.com/fallback".to_string(),
            room_id: "ga-room".to_string(),
            participant_id: "linux-guest-1".to_string(),
            participant_name: "Linux Guest".to_string(),
            wallet_identity: None,
            require_signed_moderation: true,
            require_payment_settlement: false,
            prefer_web_fallback_on_policy_failure: true,
            supports_hdr_capture: true,
            supports_hdr_render: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolSessionState {
    pub config: MeetingConfig,
    pub connection_phase: ConnectionPhase,
    pub handshake_complete: bool,
    pub resume_token: Option<String>,
    pub participants: BTreeMap<String, Participant>,
    pub permission_snapshots: BTreeMap<String, PermissionSnapshot>,
    pub presence_sequence: i64,
    pub room_locked: bool,
    pub waiting_room_enabled: bool,
    pub guest_policy: GuestPolicy,
    pub e2ee_required: bool,
    pub max_participants: i32,
    pub policy_epoch: i32,
    pub media_profile: MediaProfileState,
    pub recording_notice: RecordingState,
    pub e2ee_state: E2eeState,
    pub payment_state: PaymentState,
    pub fallback: FallbackState,
    pub last_error: Option<SessionError>,
}

impl ProtocolSessionState {
    pub fn initial(config: MeetingConfig) -> Self {
        Self {
            config,
            connection_phase: ConnectionPhase::Disconnected,
            handshake_complete: false,
            resume_token: None,
            participants: BTreeMap::new(),
            permission_snapshots: BTreeMap::new(),
            presence_sequence: 0,
            room_locked: false,
            waiting_room_enabled: false,
            guest_policy: GuestPolicy::Open,
            e2ee_required: true,
            max_participants: 300,
            policy_epoch: 0,
            media_profile: MediaProfileState::default(),
            recording_notice: RecordingState::Stopped,
            e2ee_state: E2eeState::default(),
            payment_state: PaymentState::default(),
            fallback: FallbackState::default(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolEvent {
    ConnectRequested,
    TransportConnected,
    TransportDisconnected { reason: String },
    TransportFailure { message: String },
    FrameReceived { frame: ProtocolFrame },
    FrameSendFailed { message: String },
    ManualDisconnected,
    FallbackActivated { reason: String },
    FallbackRecovered,
    ConfigUpdated { config: MeetingConfig },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProtocolFrame {
    Handshake {
        room_id: String,
        participant_id: String,
        participant_name: String,
        wallet_identity: Option<String>,
        resume_token: Option<String>,
        preferred_profile: MediaProfile,
        hdr_capture: bool,
        hdr_render: bool,
        sent_at_ms: i64,
    },
    HandshakeAck {
        session_id: String,
        resume_token: String,
        accepted_at_ms: i64,
    },
    ParticipantPresenceDelta {
        joined: Vec<Participant>,
        left: Vec<String>,
        role_changes: Vec<RoleChange>,
        sequence: i64,
    },
    RoleGrant {
        target_participant_id: String,
        role: ParticipantRole,
        granted_by: String,
        signature: Option<String>,
        issued_at_ms: i64,
    },
    RoleRevoke {
        target_participant_id: String,
        role: ParticipantRole,
        revoked_by: String,
        signature: Option<String>,
        issued_at_ms: i64,
    },
    PermissionsSnapshot {
        participant_id: String,
        effective_permissions: Vec<String>,
        epoch: i32,
    },
    ModerationSigned {
        target_participant_id: String,
        action: ModerationAction,
        issued_by: String,
        signature: Option<String>,
        sent_at_ms: i64,
    },
    SessionPolicy {
        room_lock: bool,
        waiting_room_enabled: bool,
        recording_policy: RecordingState,
        guest_policy: GuestPolicy,
        e2ee_required: bool,
        max_participants: i32,
        policy_epoch: i32,
        updated_by: String,
        signature: Option<String>,
        updated_at_ms: i64,
    },
    DeviceCapability {
        participant_id: String,
        codecs: Vec<String>,
        hdr_capture: bool,
        hdr_render: bool,
        max_streams: i32,
        updated_at_ms: i64,
    },
    MediaProfileNegotiation {
        preferred_profile: MediaProfile,
        negotiated_profile: MediaProfile,
        color_primaries: String,
        transfer_function: String,
        codec: String,
    },
    RecordingNotice {
        state: RecordingState,
        issued_at_ms: i64,
    },
    E2eeKeyEpoch {
        epoch: i32,
        issued_by: String,
        signature: Option<String>,
        sent_at_ms: i64,
    },
    KeyRotationAck {
        ack_epoch: i32,
        participant_id: String,
        sent_at_ms: i64,
    },
    PaymentPolicy {
        required: bool,
        destination_account: Option<String>,
    },
    PaymentSettlement {
        status: PaymentSettlementStatus,
    },
    Error {
        category: SessionErrorCategory,
        code: String,
        message: String,
    },
    Ping {
        sent_at_ms: i64,
    },
    Pong {
        sent_at_ms: i64,
    },
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("invalid frame json: {0}")]
    InvalidJson(String),
    #[error("invalid frame shape: {0}")]
    InvalidShape(String),
}

pub fn encode_frame(frame: &ProtocolFrame) -> Result<String, CodecError> {
    serde_json::to_string(frame).map_err(|err| CodecError::InvalidJson(err.to_string()))
}

pub fn decode_frame(payload: &str) -> Result<ProtocolFrame, CodecError> {
    let value: Value =
        serde_json::from_str(payload).map_err(|err| CodecError::InvalidJson(err.to_string()))?;
    let root = value
        .as_object()
        .ok_or_else(|| CodecError::InvalidShape("frame must be an object".to_string()))?;

    let raw_kind = root
        .get("kind")
        .and_then(value_as_str)
        .ok_or_else(|| CodecError::InvalidShape("frame.kind missing".to_string()))?;
    let kind = canonical_kind(raw_kind);

    match kind {
        "handshake" => {
            let obj = payload_or_root(root, &["handshake"]);
            Ok(ProtocolFrame::Handshake {
                room_id: require_string(
                    obj,
                    &["room_id", "roomId", "roomID"],
                    "handshake.room_id",
                )?,
                participant_id: require_string(
                    obj,
                    &["participant_id", "participantId", "participantID"],
                    "handshake.participant_id",
                )?,
                participant_name: require_string(
                    obj,
                    &["participant_name", "participantName"],
                    "handshake.participant_name",
                )?,
                wallet_identity: get_string(obj, &["wallet_identity", "walletIdentity"]),
                resume_token: get_string(obj, &["resume_token", "resumeToken"]),
                preferred_profile: parse_media_profile(get_string_ref(
                    obj,
                    &["preferred_profile", "preferredProfile"],
                )),
                hdr_capture: get_bool(obj, &["hdr_capture", "hdrCapture"]).unwrap_or(false),
                hdr_render: get_bool(obj, &["hdr_render", "hdrRender"]).unwrap_or(false),
                sent_at_ms: get_i64(obj, &["sent_at_ms", "sentAtMs"]).unwrap_or(0),
            })
        }
        "handshakeAck" => {
            let obj = payload_or_root(root, &["handshakeAck", "handshake_ack"]);
            Ok(ProtocolFrame::HandshakeAck {
                session_id: require_string(
                    obj,
                    &["session_id", "sessionId", "sessionID"],
                    "handshakeAck.session_id",
                )?,
                resume_token: require_string(
                    obj,
                    &["resume_token", "resumeToken"],
                    "handshakeAck.resume_token",
                )?,
                accepted_at_ms: get_i64(obj, &["accepted_at_ms", "acceptedAtMs"]).unwrap_or(0),
            })
        }
        "participantPresenceDelta" => {
            let obj = payload_or_root(
                root,
                &[
                    "presenceDelta",
                    "participantPresenceDelta",
                    "participant_presence_delta",
                ],
            );
            let joined = get_array(obj, &["joined"])
                .iter()
                .filter_map(parse_participant)
                .collect::<Vec<_>>();
            let left = get_array(obj, &["left"])
                .iter()
                .filter_map(value_as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let role_changes = get_array(obj, &["role_changes", "roleChanges"])
                .iter()
                .filter_map(parse_role_change)
                .collect::<Vec<_>>();
            Ok(ProtocolFrame::ParticipantPresenceDelta {
                joined,
                left,
                role_changes,
                sequence: get_i64(obj, &["sequence"]).unwrap_or(0),
            })
        }
        "roleGrant" => {
            let obj = payload_or_root(root, &["roleGrant", "role_grant"]);
            Ok(ProtocolFrame::RoleGrant {
                target_participant_id: require_string(
                    obj,
                    &[
                        "target_participant_id",
                        "targetParticipantId",
                        "targetParticipantID",
                    ],
                    "roleGrant.target_participant_id",
                )?,
                role: parse_participant_role(get_string_ref(obj, &["role"])),
                granted_by: get_string(obj, &["granted_by", "grantedBy"])
                    .unwrap_or_else(|| "unknown".to_string()),
                signature: get_string(obj, &["signature"]),
                issued_at_ms: get_i64(obj, &["issued_at_ms", "issuedAtMs"]).unwrap_or(0),
            })
        }
        "roleRevoke" => {
            let obj = payload_or_root(root, &["roleRevoke", "role_revoke"]);
            Ok(ProtocolFrame::RoleRevoke {
                target_participant_id: require_string(
                    obj,
                    &[
                        "target_participant_id",
                        "targetParticipantId",
                        "targetParticipantID",
                    ],
                    "roleRevoke.target_participant_id",
                )?,
                role: parse_participant_role(get_string_ref(obj, &["role"])),
                revoked_by: get_string(obj, &["revoked_by", "revokedBy"])
                    .unwrap_or_else(|| "unknown".to_string()),
                signature: get_string(obj, &["signature"]),
                issued_at_ms: get_i64(obj, &["issued_at_ms", "issuedAtMs"]).unwrap_or(0),
            })
        }
        "permissionsSnapshot" => {
            let obj = payload_or_root(root, &["permissionsSnapshot", "permissions_snapshot"]);
            Ok(ProtocolFrame::PermissionsSnapshot {
                participant_id: require_string(
                    obj,
                    &["participant_id", "participantId", "participantID"],
                    "permissionsSnapshot.participant_id",
                )?,
                effective_permissions: get_array(
                    obj,
                    &["effective_permissions", "effectivePermissions"],
                )
                .iter()
                .filter_map(value_as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
                epoch: get_i32(obj, &["epoch"]).unwrap_or(0),
            })
        }
        "moderationSigned" => {
            let obj = payload_or_root(root, &["moderationSigned", "moderation_signed"]);
            Ok(ProtocolFrame::ModerationSigned {
                target_participant_id: require_string(
                    obj,
                    &[
                        "target_participant_id",
                        "targetParticipantId",
                        "targetParticipantID",
                    ],
                    "moderationSigned.target_participant_id",
                )?,
                action: parse_moderation_action(get_string_ref(obj, &["action"])),
                issued_by: get_string(obj, &["issued_by", "issuedBy"])
                    .unwrap_or_else(|| "unknown".to_string()),
                signature: get_string(obj, &["signature"]),
                sent_at_ms: get_i64(obj, &["sent_at_ms", "sentAtMs"]).unwrap_or(0),
            })
        }
        "sessionPolicy" => {
            let obj = payload_or_root(root, &["sessionPolicy", "session_policy"]);
            Ok(ProtocolFrame::SessionPolicy {
                room_lock: get_bool(obj, &["room_lock", "roomLock"]).unwrap_or(false),
                waiting_room_enabled: get_bool(
                    obj,
                    &["waiting_room_enabled", "waitingRoomEnabled"],
                )
                .unwrap_or(false),
                recording_policy: parse_recording_state(get_string_ref(
                    obj,
                    &["recording_policy", "recordingPolicy"],
                )),
                guest_policy: parse_guest_policy(get_string_ref(
                    obj,
                    &["guest_policy", "guestPolicy"],
                )),
                e2ee_required: get_bool(obj, &["e2ee_required", "e2eeRequired"]).unwrap_or(true),
                max_participants: get_i32(obj, &["max_participants", "maxParticipants"])
                    .unwrap_or(300),
                policy_epoch: get_i32(obj, &["policy_epoch", "policyEpoch"]).unwrap_or(0),
                updated_by: get_string(obj, &["updated_by", "updatedBy"])
                    .unwrap_or_else(|| "system".to_string()),
                signature: get_string(obj, &["signature"]),
                updated_at_ms: get_i64(obj, &["updated_at_ms", "updatedAtMs"]).unwrap_or(0),
            })
        }
        "deviceCapability" => {
            let obj = payload_or_root(root, &["deviceCapability", "device_capability"]);
            Ok(ProtocolFrame::DeviceCapability {
                participant_id: require_string(
                    obj,
                    &["participant_id", "participantId", "participantID"],
                    "deviceCapability.participant_id",
                )?,
                codecs: get_array(obj, &["codecs"])
                    .iter()
                    .filter_map(value_as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>(),
                hdr_capture: get_bool(obj, &["hdr_capture", "hdrCapture"]).unwrap_or(false),
                hdr_render: get_bool(obj, &["hdr_render", "hdrRender"]).unwrap_or(false),
                max_streams: get_i32(obj, &["max_streams", "maxStreams"]).unwrap_or(1),
                updated_at_ms: get_i64(obj, &["updated_at_ms", "updatedAtMs"]).unwrap_or(0),
            })
        }
        "mediaProfileNegotiation" => {
            let obj = payload_or_root(
                root,
                &["mediaProfileNegotiation", "media_profile_negotiation"],
            );
            Ok(ProtocolFrame::MediaProfileNegotiation {
                preferred_profile: parse_media_profile(get_string_ref(
                    obj,
                    &["preferred_profile", "preferredProfile"],
                )),
                negotiated_profile: parse_media_profile(get_string_ref(
                    obj,
                    &["negotiated_profile", "negotiatedProfile"],
                )),
                color_primaries: get_string(obj, &["color_primaries", "colorPrimaries"])
                    .unwrap_or_else(|| "bt709".to_string()),
                transfer_function: get_string(obj, &["transfer_function", "transferFunction"])
                    .unwrap_or_else(|| "gamma".to_string()),
                codec: get_string(obj, &["codec"]).unwrap_or_else(|| "h264".to_string()),
            })
        }
        "recordingNotice" => {
            let obj = payload_or_root(root, &["recordingNotice", "recording_notice"]);
            Ok(ProtocolFrame::RecordingNotice {
                state: parse_recording_state(get_string_ref(obj, &["state"])),
                issued_at_ms: get_i64(obj, &["issued_at_ms", "issuedAtMs"]).unwrap_or(0),
            })
        }
        "e2eeKeyEpoch" => {
            let obj = payload_or_root(root, &["e2eeKeyEpoch", "e2ee_key_epoch"]);
            Ok(ProtocolFrame::E2eeKeyEpoch {
                epoch: get_i32(obj, &["epoch"]).unwrap_or(0),
                issued_by: get_string(obj, &["issued_by", "issuedBy"])
                    .unwrap_or_else(|| "unknown".to_string()),
                signature: get_string(obj, &["signature"]),
                sent_at_ms: get_i64(obj, &["sent_at_ms", "sentAtMs"]).unwrap_or(0),
            })
        }
        "keyRotationAck" => {
            let obj = payload_or_root(root, &["keyRotationAck", "key_rotation_ack"]);
            Ok(ProtocolFrame::KeyRotationAck {
                ack_epoch: get_i32(obj, &["ack_epoch", "ackEpoch"]).unwrap_or(0),
                participant_id: require_string(
                    obj,
                    &["participant_id", "participantId", "participantID"],
                    "keyRotationAck.participant_id",
                )?,
                sent_at_ms: get_i64(
                    obj,
                    &["sent_at_ms", "sentAtMs", "received_at_ms", "receivedAtMs"],
                )
                .unwrap_or(0),
            })
        }
        "paymentPolicy" => {
            let obj = payload_or_root(root, &["paymentPolicy", "payment_policy"]);
            Ok(ProtocolFrame::PaymentPolicy {
                required: get_bool(obj, &["required"]).unwrap_or(false),
                destination_account: get_string(
                    obj,
                    &["destination_account", "destinationAccount"],
                ),
            })
        }
        "paymentSettlement" => {
            let obj = payload_or_root(root, &["paymentSettlement", "payment_settlement"]);
            Ok(ProtocolFrame::PaymentSettlement {
                status: parse_payment_settlement_status(get_string_ref(obj, &["status"])),
            })
        }
        "ping" => {
            let obj = payload_or_root(root, &["ping"]);
            Ok(ProtocolFrame::Ping {
                sent_at_ms: get_i64(obj, &["sent_at_ms", "sentAtMs"]).unwrap_or(0),
            })
        }
        "pong" => {
            let obj = payload_or_root(root, &["pong"]);
            Ok(ProtocolFrame::Pong {
                sent_at_ms: get_i64(obj, &["sent_at_ms", "sentAtMs"]).unwrap_or(0),
            })
        }
        "error" => {
            let obj = payload_or_root(root, &["error"]);
            Ok(ProtocolFrame::Error {
                category: parse_session_error_category(get_string_ref(obj, &["category"])),
                code: get_string(obj, &["code"]).unwrap_or_else(|| "error".to_string()),
                message: get_string(obj, &["message"]).unwrap_or_else(|| "unknown".to_string()),
            })
        }
        other => Err(CodecError::InvalidShape(format!(
            "unsupported frame kind: {other}"
        ))),
    }
}

fn canonical_kind(raw: &str) -> &str {
    match raw {
        "handshake_ack" => "handshakeAck",
        "participant_presence_delta" => "participantPresenceDelta",
        "role_grant" => "roleGrant",
        "role_revoke" => "roleRevoke",
        "permissions_snapshot" => "permissionsSnapshot",
        "moderation_signed" => "moderationSigned",
        "session_policy" => "sessionPolicy",
        "device_capability" => "deviceCapability",
        "media_profile_negotiation" => "mediaProfileNegotiation",
        "recording_notice" => "recordingNotice",
        "e2ee_key_epoch" => "e2eeKeyEpoch",
        "key_rotation_ack" => "keyRotationAck",
        "payment_policy" => "paymentPolicy",
        "payment_settlement" => "paymentSettlement",
        other => other,
    }
}

fn payload_or_root<'a>(root: &'a Map<String, Value>, keys: &[&str]) -> &'a Map<String, Value> {
    for key in keys {
        if let Some(obj) = root.get(*key).and_then(Value::as_object) {
            return obj;
        }
    }
    root
}

fn get_value<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| obj.get(*key))
}

fn get_string(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    get_value(obj, keys)
        .and_then(value_as_str)
        .map(ToOwned::to_owned)
}

fn get_string_ref<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    get_value(obj, keys).and_then(value_as_str)
}

fn require_string(
    obj: &Map<String, Value>,
    keys: &[&str],
    field: &str,
) -> Result<String, CodecError> {
    get_string(obj, keys)
        .ok_or_else(|| CodecError::InvalidShape(format!("missing required field: {field}")))
}

fn get_bool(obj: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    get_value(obj, keys).and_then(|value| {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|number| number != 0))
            .or_else(|| match value_as_str(value) {
                Some("true") => Some(true),
                Some("false") => Some(false),
                _ => None,
            })
    })
}

fn get_i64(obj: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    get_value(obj, keys).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
            .or_else(|| value_as_str(value).and_then(|text| text.parse::<i64>().ok()))
    })
}

fn get_i32(obj: &Map<String, Value>, keys: &[&str]) -> Option<i32> {
    get_i64(obj, keys).and_then(|number| i32::try_from(number).ok())
}

fn get_array<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> &'a [Value] {
    get_value(obj, keys)
        .and_then(Value::as_array)
        .map_or(&[], |items| items.as_slice())
}

fn value_as_str(value: &Value) -> Option<&str> {
    value.as_str()
}

fn parse_participant(value: &Value) -> Option<Participant> {
    let obj = value.as_object()?;
    let id = get_string(obj, &["id"])?;
    let display_name =
        get_string(obj, &["display_name", "displayName"]).unwrap_or_else(|| id.clone());
    let role = parse_participant_role(get_string_ref(obj, &["role"]));
    let muted = get_bool(obj, &["muted"]).unwrap_or(false);
    let video_enabled = get_bool(obj, &["video_enabled", "videoEnabled"]).unwrap_or(true);
    let share_enabled = get_bool(obj, &["share_enabled", "shareEnabled"]).unwrap_or(true);
    let waiting_room = get_bool(obj, &["waiting_room", "waitingRoom"]).unwrap_or(false);

    Some(Participant {
        id,
        display_name,
        role,
        muted,
        video_enabled,
        share_enabled,
        waiting_room,
    })
}

fn parse_role_change(value: &Value) -> Option<RoleChange> {
    let obj = value.as_object()?;
    Some(RoleChange {
        participant_id: get_string(obj, &["participant_id", "participantId", "participantID"])?,
        role: parse_participant_role(get_string_ref(obj, &["role"])),
    })
}

fn parse_session_error_category(raw: Option<&str>) -> SessionErrorCategory {
    match raw.unwrap_or("protocol_failure") {
        "policyFailure" | "policy_failure" => SessionErrorCategory::PolicyFailure,
        "transportFailure" | "transport_failure" => SessionErrorCategory::TransportFailure,
        _ => SessionErrorCategory::ProtocolFailure,
    }
}

fn parse_participant_role(raw: Option<&str>) -> ParticipantRole {
    match raw.unwrap_or("participant") {
        "host" => ParticipantRole::Host,
        "coHost" | "co_host" => ParticipantRole::CoHost,
        "guest" => ParticipantRole::Guest,
        _ => ParticipantRole::Participant,
    }
}

fn parse_moderation_action(raw: Option<&str>) -> ModerationAction {
    match raw.unwrap_or("mute") {
        "videoOff" | "video_off" => ModerationAction::VideoOff,
        "stopShare" | "stop_share" => ModerationAction::StopShare,
        "kick" => ModerationAction::Kick,
        "admitFromWaiting" | "admit_from_waiting" => ModerationAction::AdmitFromWaiting,
        "denyFromWaiting" | "deny_from_waiting" => ModerationAction::DenyFromWaiting,
        _ => ModerationAction::Mute,
    }
}

fn parse_media_profile(raw: Option<&str>) -> MediaProfile {
    match raw.unwrap_or("sdr") {
        "hdr" => MediaProfile::Hdr,
        _ => MediaProfile::Sdr,
    }
}

fn parse_recording_state(raw: Option<&str>) -> RecordingState {
    match raw.unwrap_or("stopped") {
        "started" => RecordingState::Started,
        _ => RecordingState::Stopped,
    }
}

fn parse_guest_policy(raw: Option<&str>) -> GuestPolicy {
    match raw.unwrap_or("open") {
        "inviteOnly" | "invite_only" => GuestPolicy::InviteOnly,
        "blocked" => GuestPolicy::Blocked,
        _ => GuestPolicy::Open,
    }
}

fn parse_payment_settlement_status(raw: Option<&str>) -> PaymentSettlementStatus {
    match raw.unwrap_or("not_required") {
        "pending" => PaymentSettlementStatus::Pending,
        "settled" => PaymentSettlementStatus::Settled,
        "blocked" => PaymentSettlementStatus::Blocked,
        _ => PaymentSettlementStatus::NotRequired,
    }
}

fn has_required_signature(signature: &Option<String>, state: &ProtocolSessionState) -> bool {
    if !state.config.require_signed_moderation {
        return true;
    }
    signature
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn actor_is_authorized(issuer: &str, state: &ProtocolSessionState) -> bool {
    if issuer == "system" {
        return true;
    }
    state.participants.get(issuer).is_some_and(|participant| {
        participant.role == ParticipantRole::Host || participant.role == ParticipantRole::CoHost
    })
}

fn policy_reject(
    mut state: ProtocolSessionState,
    now_ms: i64,
    code: &str,
    message: &str,
) -> ProtocolSessionState {
    let fallback_requested = state.config.prefer_web_fallback_on_policy_failure;
    let fallback_active = fallback_requested || state.fallback.active;
    state.connection_phase = if fallback_active {
        ConnectionPhase::FallbackActive
    } else {
        ConnectionPhase::Error
    };
    if fallback_active {
        state.fallback.active = true;
        state
            .fallback
            .reason
            .get_or_insert_with(|| format!("policy:{code}"));
        state.fallback.activated_at_ms.get_or_insert(now_ms);
    }
    state.last_error = Some(SessionError {
        category: SessionErrorCategory::PolicyFailure,
        code: code.to_string(),
        message: message.to_string(),
        at_ms: now_ms,
    });
    state
}

fn clear_error_if_matching<F>(mut state: ProtocolSessionState, predicate: F) -> ProtocolSessionState
where
    F: Fn(Option<&SessionError>) -> bool,
{
    if !predicate(state.last_error.as_ref()) {
        return state;
    }

    state.connection_phase = if state.fallback.active {
        ConnectionPhase::FallbackActive
    } else if state.handshake_complete {
        ConnectionPhase::Connected
    } else {
        ConnectionPhase::Connecting
    };
    state.last_error = None;
    state
}

fn enforce_payment_settlement_policy(
    state: ProtocolSessionState,
    now_ms: i64,
) -> ProtocolSessionState {
    let is_payment_policy_error = |error: Option<&SessionError>| {
        error.is_some_and(|value| {
            value.category == SessionErrorCategory::PolicyFailure
                && (value.code.starts_with("payment_settlement_")
                    || value.code == "payment_unsettled")
        })
    };

    if !state.config.require_payment_settlement || !state.payment_state.required {
        return clear_error_if_matching(state, is_payment_policy_error);
    }

    if state.payment_state.settlement_status == PaymentSettlementStatus::Settled
        || state.payment_state.settlement_status == PaymentSettlementStatus::NotRequired
    {
        return clear_error_if_matching(state, is_payment_policy_error);
    }

    if state.payment_state.settlement_status == PaymentSettlementStatus::Blocked {
        return policy_reject(
            state,
            now_ms,
            "payment_settlement_blocked",
            "Payment settlement blocked by policy",
        );
    }

    policy_reject(
        state,
        now_ms,
        "payment_settlement_required",
        "Payment settlement required before media/session actions can continue",
    )
}

fn enforce_e2ee_epoch_policy(state: ProtocolSessionState, now_ms: i64) -> ProtocolSessionState {
    if !state.e2ee_required {
        return clear_error_if_matching(state, |error| {
            error.is_some_and(|value| {
                value.category == SessionErrorCategory::PolicyFailure
                    && value.code.starts_with("e2ee_")
            })
        });
    }

    if state.e2ee_state.current_epoch > 0 {
        return clear_error_if_matching(state, |error| {
            error.is_some_and(|value| {
                value.category == SessionErrorCategory::PolicyFailure
                    && value.code.starts_with("e2ee_")
            })
        });
    }

    policy_reject(
        state,
        now_ms,
        "e2ee_epoch_required",
        "E2EE key epoch is required by session policy",
    )
}

fn reduce_frame(
    mut state: ProtocolSessionState,
    frame: ProtocolFrame,
    now_ms: i64,
) -> ProtocolSessionState {
    match frame {
        ProtocolFrame::HandshakeAck { resume_token, .. } => {
            state.handshake_complete = true;
            state.resume_token = Some(resume_token);
            state.connection_phase = if state.fallback.active {
                ConnectionPhase::FallbackActive
            } else {
                ConnectionPhase::Connected
            };
            state.last_error = None;
            state
        }
        ProtocolFrame::ParticipantPresenceDelta {
            joined,
            left,
            role_changes,
            sequence,
        } => {
            if sequence <= state.presence_sequence {
                return state;
            }
            for participant in joined {
                state
                    .participants
                    .insert(participant.id.clone(), participant);
            }
            for participant_id in left {
                state.participants.remove(&participant_id);
            }
            for change in role_changes {
                if let Some(participant) = state.participants.get_mut(&change.participant_id) {
                    participant.role = change.role;
                }
            }
            state.presence_sequence = sequence;
            state
        }
        ProtocolFrame::RoleGrant {
            target_participant_id,
            role,
            granted_by,
            signature,
            ..
        } => {
            if !has_required_signature(&signature, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "role_grant_signature_missing",
                    "RoleGrant signature is required",
                );
            }
            if !actor_is_authorized(&granted_by, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "role_grant_not_authorized",
                    "RoleGrant issuer is not host/co-host",
                );
            }
            if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                participant.role = role;
            }
            state
        }
        ProtocolFrame::RoleRevoke {
            target_participant_id,
            role,
            revoked_by,
            signature,
            ..
        } => {
            if !has_required_signature(&signature, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "role_revoke_signature_missing",
                    "RoleRevoke signature is required",
                );
            }
            if !actor_is_authorized(&revoked_by, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "role_revoke_not_authorized",
                    "RoleRevoke issuer is not host/co-host",
                );
            }
            if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                if participant.role == role {
                    participant.role = ParticipantRole::Participant;
                }
            }
            state
        }
        ProtocolFrame::PermissionsSnapshot {
            participant_id,
            effective_permissions,
            epoch,
        } => {
            let should_apply = state
                .permission_snapshots
                .get(&participant_id)
                .is_none_or(|snapshot| epoch > snapshot.epoch);
            if should_apply {
                state.permission_snapshots.insert(
                    participant_id,
                    PermissionSnapshot {
                        effective_permissions,
                        epoch,
                    },
                );
            }
            state
        }
        ProtocolFrame::ModerationSigned {
            target_participant_id,
            action,
            issued_by,
            signature,
            ..
        } => {
            if !has_required_signature(&signature, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "moderation_signature_missing",
                    "Moderation signature is required",
                );
            }
            if !actor_is_authorized(&issued_by, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "moderation_not_authorized",
                    "Moderation issuer is not host/co-host",
                );
            }
            match action {
                ModerationAction::Mute => {
                    if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                        participant.muted = true;
                    }
                }
                ModerationAction::VideoOff => {
                    if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                        participant.video_enabled = false;
                    }
                }
                ModerationAction::StopShare => {
                    if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                        participant.share_enabled = false;
                    }
                }
                ModerationAction::Kick | ModerationAction::DenyFromWaiting => {
                    state.participants.remove(&target_participant_id);
                }
                ModerationAction::AdmitFromWaiting => {
                    if let Some(participant) = state.participants.get_mut(&target_participant_id) {
                        participant.waiting_room = false;
                    }
                }
            }
            state
        }
        ProtocolFrame::SessionPolicy {
            room_lock,
            waiting_room_enabled,
            recording_policy,
            guest_policy,
            e2ee_required,
            max_participants,
            policy_epoch,
            updated_by,
            signature,
            ..
        } => {
            if !has_required_signature(&signature, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "session_policy_signature_missing",
                    "SessionPolicy signature is required",
                );
            }
            if !actor_is_authorized(&updated_by, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "session_policy_not_authorized",
                    "SessionPolicy issuer is not host/co-host",
                );
            }
            if policy_epoch < state.policy_epoch {
                return state;
            }

            state.room_locked = room_lock;
            state.waiting_room_enabled = waiting_room_enabled;
            state.recording_notice = recording_policy;
            state.guest_policy = guest_policy;
            state.e2ee_required = e2ee_required;
            state.max_participants = max_participants;
            state.policy_epoch = policy_epoch;
            enforce_e2ee_epoch_policy(state, now_ms)
        }
        ProtocolFrame::MediaProfileNegotiation {
            preferred_profile,
            negotiated_profile,
            color_primaries,
            transfer_function,
            codec,
        } => {
            state.media_profile = MediaProfileState {
                preferred_profile,
                negotiated_profile,
                color_primaries,
                transfer_function,
                codec,
            };
            if preferred_profile == MediaProfile::Hdr && negotiated_profile == MediaProfile::Sdr {
                state.connection_phase = ConnectionPhase::Degraded;
            } else if state.handshake_complete && !state.fallback.active {
                state.connection_phase = ConnectionPhase::Connected;
            }
            state
        }
        ProtocolFrame::RecordingNotice {
            state: recording_state,
            ..
        } => {
            state.recording_notice = recording_state;
            state
        }
        ProtocolFrame::E2eeKeyEpoch {
            epoch, signature, ..
        } => {
            if !has_required_signature(&signature, &state) {
                return policy_reject(
                    state,
                    now_ms,
                    "e2ee_signature_missing",
                    "E2EE key epoch signature is required",
                );
            }
            state.e2ee_state.current_epoch = state.e2ee_state.current_epoch.max(epoch);
            enforce_e2ee_epoch_policy(state, now_ms)
        }
        ProtocolFrame::KeyRotationAck { ack_epoch, .. } => {
            state.e2ee_state.last_ack_epoch = state.e2ee_state.last_ack_epoch.max(ack_epoch);
            state
        }
        ProtocolFrame::PaymentPolicy {
            required,
            destination_account,
        } => {
            state.payment_state.required = required;
            state.payment_state.destination = destination_account;
            state.payment_state.settlement_status = if required {
                PaymentSettlementStatus::Pending
            } else {
                PaymentSettlementStatus::NotRequired
            };
            enforce_payment_settlement_policy(state, now_ms)
        }
        ProtocolFrame::PaymentSettlement { status } => {
            state.payment_state.settlement_status = status;
            enforce_payment_settlement_policy(state, now_ms)
        }
        ProtocolFrame::Error {
            category,
            code,
            message,
        } => {
            state.connection_phase = if state.fallback.active {
                ConnectionPhase::FallbackActive
            } else if category == SessionErrorCategory::PolicyFailure {
                ConnectionPhase::Error
            } else {
                ConnectionPhase::Degraded
            };
            state.last_error = Some(SessionError {
                category,
                code,
                message,
                at_ms: now_ms,
            });
            state
        }
        ProtocolFrame::Handshake { .. }
        | ProtocolFrame::DeviceCapability { .. }
        | ProtocolFrame::Ping { .. }
        | ProtocolFrame::Pong { .. } => state,
    }
}

pub fn reduce(
    mut state: ProtocolSessionState,
    event: ProtocolEvent,
    now_ms: i64,
) -> ProtocolSessionState {
    match event {
        ProtocolEvent::ConnectRequested => {
            state.connection_phase = ConnectionPhase::Connecting;
            state.handshake_complete = false;
            state.last_error = None;
            state
        }
        ProtocolEvent::TransportConnected => {
            state.connection_phase = ConnectionPhase::Connecting;
            state.last_error = None;
            state
        }
        ProtocolEvent::TransportDisconnected { reason } => {
            if state.fallback.active {
                state.connection_phase = ConnectionPhase::FallbackActive;
                state.handshake_complete = false;
                return state;
            }
            state.connection_phase = ConnectionPhase::Degraded;
            state.handshake_complete = false;
            state.last_error = Some(SessionError {
                category: SessionErrorCategory::TransportFailure,
                code: "transport_disconnected".to_string(),
                message: reason,
                at_ms: now_ms,
            });
            state
        }
        ProtocolEvent::TransportFailure { message } => {
            if state.fallback.active {
                state.connection_phase = ConnectionPhase::FallbackActive;
                state.handshake_complete = false;
                return state;
            }
            state.connection_phase = ConnectionPhase::Degraded;
            state.handshake_complete = false;
            state.last_error = Some(SessionError {
                category: SessionErrorCategory::TransportFailure,
                code: "transport_failure".to_string(),
                message,
                at_ms: now_ms,
            });
            state
        }
        ProtocolEvent::FrameSendFailed { message } => {
            if state.fallback.active {
                state.connection_phase = ConnectionPhase::FallbackActive;
                return state;
            }
            state.connection_phase = ConnectionPhase::Degraded;
            state.last_error = Some(SessionError {
                category: SessionErrorCategory::TransportFailure,
                code: "send_failed".to_string(),
                message,
                at_ms: now_ms,
            });
            state
        }
        ProtocolEvent::ManualDisconnected => {
            state.connection_phase = ConnectionPhase::Disconnected;
            state.handshake_complete = false;
            state.last_error = None;
            state
        }
        ProtocolEvent::FallbackActivated { reason } => {
            state.connection_phase = ConnectionPhase::FallbackActive;
            state.fallback.active = true;
            state.fallback.reason = Some(reason.clone());
            state.fallback.activated_at_ms = Some(now_ms);
            state.last_error = Some(SessionError {
                category: SessionErrorCategory::TransportFailure,
                code: "fallback_activated".to_string(),
                message: reason,
                at_ms: now_ms,
            });
            state
        }
        ProtocolEvent::FallbackRecovered => {
            let rto = state
                .fallback
                .activated_at_ms
                .map(|at| (now_ms - at).max(0));
            state.connection_phase = ConnectionPhase::Disconnected;
            state.fallback.active = false;
            state.fallback.reason = None;
            state.fallback.recovered_at_ms = Some(now_ms);
            state.fallback.last_rto_ms = rto;
            state
        }
        ProtocolEvent::ConfigUpdated { config } => {
            state.config = config;
            enforce_payment_settlement_policy(state, now_ms)
        }
        ProtocolEvent::FrameReceived { frame } => reduce_frame(state, frame, now_ms),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDirective {
    None,
    ReconnectScheduled { attempt: usize, due_at_ms: i64 },
    FallbackActivated { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTelemetryCategory {
    ConnectionLifecycle,
    FallbackLifecycle,
    PolicyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingTelemetryEvent {
    pub category: MeetingTelemetryCategory,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub at_ms: i64,
}

pub trait MeetingTelemetrySink: Send + Sync {
    fn record(&self, event: MeetingTelemetryEvent);
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpMeetingTelemetrySink;

impl MeetingTelemetrySink for NoOpMeetingTelemetrySink {
    fn record(&self, _event: MeetingTelemetryEvent) {}
}

pub struct SessionRuntime {
    state: ProtocolSessionState,
    reconnect_backoff_ms: Vec<i64>,
    reconnect_attempt: usize,
    reconnect_due_at_ms: Option<i64>,
    user_initiated_disconnect: bool,
    app_in_background: bool,
    telemetry_sink: Arc<dyn MeetingTelemetrySink>,
}

impl SessionRuntime {
    pub fn new(config: MeetingConfig) -> Self {
        Self::with_backoff(config, vec![1_000, 2_000, 4_000, 8_000])
    }

    pub fn with_backoff(config: MeetingConfig, reconnect_backoff_ms: Vec<i64>) -> Self {
        Self::with_backoff_and_telemetry(
            config,
            reconnect_backoff_ms,
            Arc::new(NoOpMeetingTelemetrySink),
        )
    }

    pub fn with_backoff_and_telemetry(
        config: MeetingConfig,
        reconnect_backoff_ms: Vec<i64>,
        telemetry_sink: Arc<dyn MeetingTelemetrySink>,
    ) -> Self {
        Self {
            state: ProtocolSessionState::initial(config),
            reconnect_backoff_ms,
            reconnect_attempt: 0,
            reconnect_due_at_ms: None,
            user_initiated_disconnect: false,
            app_in_background: false,
            telemetry_sink,
        }
    }

    pub fn state(&self) -> &ProtocolSessionState {
        &self.state
    }

    pub fn reconnect_due_at_ms(&self) -> Option<i64> {
        self.reconnect_due_at_ms
    }

    pub fn connect_requested(&mut self, now_ms: i64) {
        self.request_connect(true, "manual", now_ms);
    }

    pub fn on_app_foregrounded(&mut self, now_ms: i64) {
        if self.user_initiated_disconnect {
            return;
        }

        self.app_in_background = false;
        self.record_connection_event("app_foregrounded", [], now_ms);
        let phase = self.state.connection_phase;
        if matches!(
            phase,
            ConnectionPhase::Disconnected | ConnectionPhase::Degraded | ConnectionPhase::Error
        ) {
            self.request_connect(false, "foreground", now_ms);
        }
    }

    pub fn on_app_backgrounded(&mut self, now_ms: i64) {
        self.app_in_background = true;
        self.reconnect_due_at_ms = None;
        self.record_connection_event("app_backgrounded", [], now_ms);
        if self.user_initiated_disconnect {
            return;
        }

        let phase = self.state.connection_phase;
        if matches!(
            phase,
            ConnectionPhase::Connected | ConnectionPhase::Connecting | ConnectionPhase::Degraded
        ) {
            self.apply_event(
                ProtocolEvent::TransportDisconnected {
                    reason: "app_backgrounded".to_string(),
                },
                now_ms,
            );
        }
    }

    pub fn on_connectivity_changed(&mut self, available: bool, now_ms: i64) {
        if self.user_initiated_disconnect {
            return;
        }

        if available {
            self.record_connection_event("network_available", [], now_ms);
            if self.app_in_background {
                self.record_connection_event(
                    "connectivity_restore_deferred_backgrounded",
                    [],
                    now_ms,
                );
                return;
            }

            if self.state.connection_phase != ConnectionPhase::Connected
                && self.state.connection_phase != ConnectionPhase::Connecting
                && !self.state.fallback.active
            {
                self.request_connect(false, "connectivity_restore", now_ms);
            }
            return;
        }

        self.record_connection_event("network_unavailable", [], now_ms);
        self.apply_event(
            ProtocolEvent::TransportFailure {
                message: "network_unavailable".to_string(),
            },
            now_ms,
        );
        if self.app_in_background {
            self.record_connection_event(
                "reconnect_deferred_backgrounded",
                [("trigger", "network_unavailable".to_string())],
                now_ms,
            );
            return;
        }
        let _ = self.schedule_reconnect_or_fallback("network_unavailable", now_ms);
    }

    pub fn on_audio_interruption_began(&mut self, now_ms: i64) {
        if self.user_initiated_disconnect {
            return;
        }

        self.record_connection_event("audio_interruption_began", [], now_ms);
        self.apply_event(
            ProtocolEvent::TransportFailure {
                message: "audio_interruption".to_string(),
            },
            now_ms,
        );
        if self.app_in_background {
            self.record_connection_event(
                "reconnect_deferred_backgrounded",
                [("trigger", "audio_interruption".to_string())],
                now_ms,
            );
            return;
        }
        let _ = self.schedule_reconnect_or_fallback("audio_interruption", now_ms);
    }

    pub fn on_audio_interruption_ended(&mut self, should_reconnect: bool, now_ms: i64) {
        if self.user_initiated_disconnect {
            return;
        }

        self.record_connection_event(
            "audio_interruption_ended",
            [(
                "should_reconnect",
                if should_reconnect {
                    "true".to_string()
                } else {
                    "false".to_string()
                },
            )],
            now_ms,
        );
        if !should_reconnect || self.app_in_background || self.state.fallback.active {
            return;
        }

        if self.state.connection_phase != ConnectionPhase::Connected
            && self.state.connection_phase != ConnectionPhase::Connecting
        {
            self.request_connect(false, "audio_interruption_end", now_ms);
        }
    }

    pub fn on_audio_route_changed(&mut self, reason: impl Into<String>, now_ms: i64) {
        let reason = reason.into();
        self.record_connection_event("audio_route_changed", [("reason", reason)], now_ms);
    }

    pub fn on_transport_connected(&mut self, now_ms: i64) -> Vec<ProtocolFrame> {
        if self.app_in_background {
            self.reconnect_due_at_ms = None;
            self.record_connection_event("transport_connected_while_backgrounded", [], now_ms);
            self.apply_event(
                ProtocolEvent::TransportDisconnected {
                    reason: "backgrounded_before_handshake".to_string(),
                },
                now_ms,
            );
            return Vec::new();
        }

        self.user_initiated_disconnect = false;
        self.reconnect_attempt = 0;
        self.reconnect_due_at_ms = None;
        self.record_connection_event("transport_connected", [], now_ms);
        self.apply_event(ProtocolEvent::TransportConnected, now_ms);
        self.handshake_frames(now_ms)
    }

    pub fn on_transport_disconnected(
        &mut self,
        reason: impl Into<String>,
        now_ms: i64,
    ) -> RuntimeDirective {
        let reason = reason.into();
        self.record_connection_event(
            "transport_disconnected",
            [("reason", reason.clone())],
            now_ms,
        );
        self.apply_event(
            ProtocolEvent::TransportDisconnected {
                reason: reason.clone(),
            },
            now_ms,
        );
        if self.app_in_background {
            self.record_connection_event(
                "reconnect_deferred_backgrounded",
                [("trigger", reason)],
                now_ms,
            );
            return RuntimeDirective::None;
        }
        self.schedule_reconnect_or_fallback(&reason, now_ms)
    }

    pub fn on_transport_failure(
        &mut self,
        message: impl Into<String>,
        now_ms: i64,
    ) -> RuntimeDirective {
        let message = message.into();
        self.record_connection_event("transport_failure", [("message", message.clone())], now_ms);
        self.apply_event(
            ProtocolEvent::TransportFailure {
                message: message.clone(),
            },
            now_ms,
        );
        if self.app_in_background {
            self.record_connection_event(
                "reconnect_deferred_backgrounded",
                [("trigger", message)],
                now_ms,
            );
            return RuntimeDirective::None;
        }
        self.schedule_reconnect_or_fallback(&message, now_ms)
    }

    pub fn on_send_failure(&mut self, message: impl Into<String>, now_ms: i64) {
        let message = message.into();
        self.record_connection_event("send_failure", [("message", message.clone())], now_ms);
        self.apply_event(ProtocolEvent::FrameSendFailed { message }, now_ms);
    }

    pub fn on_manual_disconnect(&mut self, now_ms: i64) {
        self.user_initiated_disconnect = true;
        self.reconnect_due_at_ms = None;
        self.record_connection_event("manual_disconnect", [], now_ms);
        self.apply_event(ProtocolEvent::ManualDisconnected, now_ms);
    }

    pub fn on_frame(&mut self, frame: ProtocolFrame, now_ms: i64) -> Vec<ProtocolFrame> {
        let should_pong = matches!(frame, ProtocolFrame::Ping { .. });
        let maybe_e2ee_epoch = match &frame {
            ProtocolFrame::E2eeKeyEpoch {
                epoch, signature, ..
            } => Some((*epoch, signature.clone())),
            _ => None,
        };
        let participant_id = self.resolved_participant_id();
        self.apply_event(ProtocolEvent::FrameReceived { frame }, now_ms);

        let mut outbound = Vec::new();
        if should_pong {
            outbound.push(ProtocolFrame::Pong { sent_at_ms: now_ms });
        }

        if let Some((epoch, signature)) = maybe_e2ee_epoch {
            let signature_ok = if self.state.config.require_signed_moderation {
                signature
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty())
            } else {
                true
            };
            if signature_ok && self.state.e2ee_state.current_epoch >= epoch {
                let ack = ProtocolFrame::KeyRotationAck {
                    ack_epoch: epoch,
                    participant_id: participant_id.clone(),
                    sent_at_ms: now_ms,
                };
                outbound.push(ack.clone());
                self.apply_event(ProtocolEvent::FrameReceived { frame: ack }, now_ms);
            }
        }

        outbound
    }

    pub fn recover_from_fallback(&mut self, now_ms: i64) {
        self.user_initiated_disconnect = false;
        self.reconnect_attempt = 0;
        self.reconnect_due_at_ms = None;
        self.record_connection_event("fallback_recovery_requested", [], now_ms);
        self.apply_event(ProtocolEvent::FallbackRecovered, now_ms);
        self.request_connect(false, "fallback_recovery", now_ms);
    }

    pub fn take_reconnect_if_due(&mut self, now_ms: i64) -> bool {
        let Some(due_at_ms) = self.reconnect_due_at_ms else {
            return false;
        };

        if now_ms < due_at_ms
            || self.user_initiated_disconnect
            || self.state.fallback.active
            || self.app_in_background
        {
            return false;
        }

        self.reconnect_due_at_ms = None;
        self.record_connection_event("reconnect_attempt", [], now_ms);
        self.request_connect(false, "reconnect", now_ms);
        true
    }

    fn schedule_reconnect_or_fallback(&mut self, trigger: &str, now_ms: i64) -> RuntimeDirective {
        if self.user_initiated_disconnect || self.state.fallback.active || self.app_in_background {
            return RuntimeDirective::None;
        }
        if self.reconnect_due_at_ms.is_some() {
            return RuntimeDirective::None;
        }

        if self.reconnect_attempt >= self.reconnect_backoff_ms.len() {
            let reason = format!(
                "Reconnect exhausted after {} attempts: {}",
                self.reconnect_attempt, trigger
            );
            self.state = reduce(
                self.state.clone(),
                ProtocolEvent::FallbackActivated {
                    reason: reason.clone(),
                },
                now_ms,
            );
            return RuntimeDirective::FallbackActivated { reason };
        }

        let delay_ms = self.reconnect_backoff_ms[self.reconnect_attempt].max(0);
        self.reconnect_attempt += 1;
        let due_at_ms = now_ms.saturating_add(delay_ms);
        self.reconnect_due_at_ms = Some(due_at_ms);
        self.record_connection_event(
            "reconnect_scheduled",
            [
                ("attempt", self.reconnect_attempt.to_string()),
                ("due_at_ms", due_at_ms.to_string()),
                ("trigger", trigger.to_string()),
            ],
            now_ms,
        );
        RuntimeDirective::ReconnectScheduled {
            attempt: self.reconnect_attempt,
            due_at_ms,
        }
    }

    fn request_connect(&mut self, reset_backoff: bool, source: &str, now_ms: i64) {
        if self.app_in_background {
            self.record_connection_event(
                "connect_deferred_backgrounded",
                [("source", source.to_string())],
                now_ms,
            );
            return;
        }

        self.user_initiated_disconnect = false;
        self.reconnect_due_at_ms = None;
        if reset_backoff {
            self.reconnect_attempt = 0;
        }
        self.record_connection_event(
            "connect_requested",
            [("source", source.to_string())],
            now_ms,
        );
        self.apply_event(ProtocolEvent::ConnectRequested, now_ms);
    }

    fn handshake_frames(&self, now_ms: i64) -> Vec<ProtocolFrame> {
        let participant_id = self.resolved_participant_id();
        let preferred_profile =
            if self.state.config.supports_hdr_capture && self.state.config.supports_hdr_render {
                MediaProfile::Hdr
            } else {
                MediaProfile::Sdr
            };

        let mut frames = vec![
            ProtocolFrame::Handshake {
                room_id: self.state.config.room_id.clone(),
                participant_id: participant_id.clone(),
                participant_name: self.state.config.participant_name.clone(),
                wallet_identity: self.state.config.wallet_identity.clone(),
                resume_token: self.state.resume_token.clone(),
                preferred_profile,
                hdr_capture: self.state.config.supports_hdr_capture,
                hdr_render: self.state.config.supports_hdr_render,
                sent_at_ms: now_ms,
            },
            ProtocolFrame::DeviceCapability {
                participant_id,
                codecs: vec!["h264".to_string(), "vp9".to_string()],
                hdr_capture: self.state.config.supports_hdr_capture,
                hdr_render: self.state.config.supports_hdr_render,
                max_streams: 4,
                updated_at_ms: now_ms,
            },
        ];

        if self.state.config.require_payment_settlement {
            frames.push(ProtocolFrame::PaymentPolicy {
                required: true,
                destination_account: Some("nexus://payment-policy".to_string()),
            });
        }

        frames
    }

    fn resolved_participant_id(&self) -> String {
        if !self.state.config.participant_id.trim().is_empty() {
            return normalize_participant_id(&self.state.config.participant_id);
        }
        normalize_participant_id(&self.state.config.participant_name)
    }

    fn apply_event(&mut self, event: ProtocolEvent, now_ms: i64) {
        let previous = self.state.clone();
        self.state = reduce(self.state.clone(), event, now_ms);
        self.emit_state_transition_telemetry(&previous, &self.state, now_ms);
    }

    fn emit_state_transition_telemetry(
        &self,
        previous: &ProtocolSessionState,
        next: &ProtocolSessionState,
        now_ms: i64,
    ) {
        if previous.connection_phase != next.connection_phase {
            self.record_connection_event(
                "phase_changed",
                [
                    ("from", format!("{:?}", previous.connection_phase)),
                    ("to", format!("{:?}", next.connection_phase)),
                ],
                now_ms,
            );
        }

        if !previous.fallback.active && next.fallback.active {
            self.record_fallback_event(
                "fallback_activated",
                [(
                    "reason",
                    next.fallback
                        .reason
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                )],
                now_ms,
            );
        }

        if previous.fallback.active && !next.fallback.active {
            let mut attrs = BTreeMap::new();
            if let Some(rto_ms) = next.fallback.last_rto_ms {
                attrs.insert("rto_ms".to_string(), rto_ms.to_string());
            }
            self.record_telemetry(MeetingTelemetryEvent {
                category: MeetingTelemetryCategory::FallbackLifecycle,
                name: "fallback_recovered".to_string(),
                attributes: attrs,
                at_ms: now_ms,
            });
        }

        if previous.last_error != next.last_error
            && next
                .last_error
                .as_ref()
                .is_some_and(|error| error.category == SessionErrorCategory::PolicyFailure)
        {
            if let Some(error) = &next.last_error {
                self.record_policy_failure_event(
                    error.code.clone(),
                    [
                        ("code", error.code.clone()),
                        ("message", error.message.clone()),
                    ],
                    now_ms,
                );
            }
        }
    }

    fn record_connection_event(
        &self,
        name: &str,
        attributes: impl IntoIterator<Item = (&'static str, String)>,
        now_ms: i64,
    ) {
        self.record_typed_event(
            MeetingTelemetryCategory::ConnectionLifecycle,
            name,
            attributes,
            now_ms,
        );
    }

    fn record_fallback_event(
        &self,
        name: &str,
        attributes: impl IntoIterator<Item = (&'static str, String)>,
        now_ms: i64,
    ) {
        self.record_typed_event(
            MeetingTelemetryCategory::FallbackLifecycle,
            name,
            attributes,
            now_ms,
        );
    }

    fn record_policy_failure_event(
        &self,
        name: String,
        attributes: impl IntoIterator<Item = (&'static str, String)>,
        now_ms: i64,
    ) {
        self.record_telemetry(MeetingTelemetryEvent {
            category: MeetingTelemetryCategory::PolicyFailure,
            name,
            attributes: map_attributes(attributes),
            at_ms: now_ms,
        });
    }

    fn record_typed_event(
        &self,
        category: MeetingTelemetryCategory,
        name: &str,
        attributes: impl IntoIterator<Item = (&'static str, String)>,
        now_ms: i64,
    ) {
        self.record_telemetry(MeetingTelemetryEvent {
            category,
            name: name.to_string(),
            attributes: map_attributes(attributes),
            at_ms: now_ms,
        });
    }

    fn record_telemetry(&self, event: MeetingTelemetryEvent) {
        self.telemetry_sink.record(event);
    }
}

fn normalize_participant_id(raw: &str) -> String {
    let source = raw.trim();
    let base = if source.is_empty() {
        "participant"
    } else {
        source
    };
    let normalized: String = base
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect();

    if normalized.is_empty() {
        "participant".to_string()
    } else {
        normalized
    }
}

fn map_attributes(
    attributes: impl IntoIterator<Item = (&'static str, String)>,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (key, value) in attributes {
        map.insert(key.to_string(), value);
    }
    map
}

#[cfg(feature = "gtk-ui")]
pub mod gtk {
    use gtk4 as gtk;

    pub fn app_id() -> &'static str {
        "io.sora.kaigi.linux"
    }

    pub fn validate_runtime() {
        let _ = gtk::glib::MainContext::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct InMemoryMeetingTelemetrySink {
        events: Mutex<Vec<MeetingTelemetryEvent>>,
    }

    impl InMemoryMeetingTelemetrySink {
        fn snapshot(&self) -> Vec<MeetingTelemetryEvent> {
            self.events
                .lock()
                .expect("telemetry mutex poisoned")
                .clone()
        }
    }

    impl MeetingTelemetrySink for InMemoryMeetingTelemetrySink {
        fn record(&self, event: MeetingTelemetryEvent) {
            self.events
                .lock()
                .expect("telemetry mutex poisoned")
                .push(event);
        }
    }

    #[test]
    fn handshake_ack_marks_connected() {
        let state = ProtocolSessionState::initial(MeetingConfig::default());
        let next = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::HandshakeAck {
                    session_id: "session".to_string(),
                    resume_token: "token".to_string(),
                    accepted_at_ms: 10,
                },
            },
            10,
        );

        assert_eq!(next.connection_phase, ConnectionPhase::Connected);
        assert!(next.handshake_complete);
        assert_eq!(next.resume_token.as_deref(), Some("token"));
    }

    #[test]
    fn presence_delta_sequence_remains_monotonic() {
        let state = ProtocolSessionState::initial(MeetingConfig::default());
        let joined = Participant {
            id: "p2".to_string(),
            display_name: "Beta".to_string(),
            role: ParticipantRole::Participant,
            muted: false,
            video_enabled: true,
            share_enabled: true,
            waiting_room: false,
        };

        let next = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::ParticipantPresenceDelta {
                    joined: vec![joined],
                    left: vec![],
                    role_changes: vec![],
                    sequence: 4,
                },
            },
            1,
        );

        let stale = reduce(
            next.clone(),
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::ParticipantPresenceDelta {
                    joined: vec![],
                    left: vec!["p2".to_string()],
                    role_changes: vec![],
                    sequence: 3,
                },
            },
            2,
        );

        assert_eq!(stale.presence_sequence, 4);
        assert!(stale.participants.contains_key("p2"));
    }

    #[test]
    fn unsigned_role_grant_is_rejected() {
        let mut config = MeetingConfig::default();
        config.prefer_web_fallback_on_policy_failure = false;
        let mut state = ProtocolSessionState::initial(config);
        state.participants.insert(
            "host".to_string(),
            Participant {
                id: "host".to_string(),
                display_name: "Host".to_string(),
                role: ParticipantRole::Host,
                muted: false,
                video_enabled: true,
                share_enabled: true,
                waiting_room: false,
            },
        );
        let next = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::RoleGrant {
                    target_participant_id: "host".to_string(),
                    role: ParticipantRole::CoHost,
                    granted_by: "host".to_string(),
                    signature: None,
                    issued_at_ms: 9,
                },
            },
            9,
        );

        assert_eq!(next.connection_phase, ConnectionPhase::Error);
        assert_eq!(
            next.last_error.as_ref().map(|e| e.code.as_str()),
            Some("role_grant_signature_missing")
        );
    }

    #[test]
    fn payment_policy_error_clears_when_settlement_not_required() {
        let state = ProtocolSessionState::initial(MeetingConfig {
            require_payment_settlement: true,
            ..MeetingConfig::default()
        });
        let rejected = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::PaymentPolicy {
                    required: true,
                    destination_account: Some("wallet:dest".to_string()),
                },
            },
            10,
        );

        assert_eq!(
            rejected.last_error.as_ref().map(|e| e.code.as_str()),
            Some("payment_settlement_required")
        );

        let cleared = reduce(
            rejected,
            ProtocolEvent::ConfigUpdated {
                config: MeetingConfig {
                    require_payment_settlement: false,
                    ..MeetingConfig::default()
                },
            },
            11,
        );

        assert!(cleared.last_error.is_none());
    }

    #[test]
    fn payment_policy_uses_blocked_code_when_settlement_blocked() {
        let connected = reduce(
            ProtocolSessionState::initial(MeetingConfig {
                require_payment_settlement: true,
                prefer_web_fallback_on_policy_failure: false,
                ..MeetingConfig::default()
            }),
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::HandshakeAck {
                    session_id: "s-blocked".to_string(),
                    resume_token: "r-blocked".to_string(),
                    accepted_at_ms: 1,
                },
            },
            1,
        );

        let pending = reduce(
            connected,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::PaymentPolicy {
                    required: true,
                    destination_account: Some("wallet:dest".to_string()),
                },
            },
            2,
        );

        let blocked = reduce(
            pending,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::PaymentSettlement {
                    status: PaymentSettlementStatus::Blocked,
                },
            },
            3,
        );

        assert_eq!(blocked.connection_phase, ConnectionPhase::Error);
        assert_eq!(
            blocked.last_error.as_ref().map(|e| e.code.as_str()),
            Some("payment_settlement_blocked")
        );
    }

    #[test]
    fn payment_policy_error_clears_when_settlement_frame_becomes_not_required() {
        let state = ProtocolSessionState::initial(MeetingConfig {
            require_payment_settlement: true,
            prefer_web_fallback_on_policy_failure: false,
            ..MeetingConfig::default()
        });
        let pending = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::PaymentPolicy {
                    required: true,
                    destination_account: Some("wallet:dest".to_string()),
                },
            },
            10,
        );

        assert_eq!(
            pending.last_error.as_ref().map(|e| e.code.as_str()),
            Some("payment_settlement_required")
        );

        let cleared = reduce(
            pending,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::PaymentSettlement {
                    status: PaymentSettlementStatus::NotRequired,
                },
            },
            11,
        );

        assert_eq!(cleared.connection_phase, ConnectionPhase::Connecting);
        assert!(cleared.last_error.is_none());
    }

    #[test]
    fn moderation_requires_host_role_when_signatures_optional() {
        let mut config = MeetingConfig::default();
        config.require_signed_moderation = false;
        config.prefer_web_fallback_on_policy_failure = false;
        let mut state = ProtocolSessionState::initial(config);
        state.participants.insert(
            "host".to_string(),
            Participant {
                id: "host".to_string(),
                display_name: "Host".to_string(),
                role: ParticipantRole::Host,
                muted: false,
                video_enabled: true,
                share_enabled: true,
                waiting_room: false,
            },
        );
        state.participants.insert(
            "participant-1".to_string(),
            Participant {
                id: "participant-1".to_string(),
                display_name: "Participant 1".to_string(),
                role: ParticipantRole::Participant,
                muted: false,
                video_enabled: true,
                share_enabled: true,
                waiting_room: false,
            },
        );
        let next = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::ModerationSigned {
                    target_participant_id: "host".to_string(),
                    action: ModerationAction::Mute,
                    issued_by: "participant-1".to_string(),
                    signature: None,
                    sent_at_ms: 9,
                },
            },
            9,
        );

        assert_eq!(next.connection_phase, ConnectionPhase::Error);
        assert_eq!(
            next.last_error.as_ref().map(|e| e.code.as_str()),
            Some("moderation_not_authorized")
        );
    }

    #[test]
    fn decode_handles_snake_case_kind_alias() {
        let frame = decode_frame(
            r#"{"kind":"handshake_ack","session_id":"s1","resume_token":"r1","accepted_at_ms":10}"#,
        )
        .expect("decode frame");

        match frame {
            ProtocolFrame::HandshakeAck {
                session_id,
                resume_token,
                ..
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(resume_token, "r1");
            }
            _ => panic!("expected handshake ack"),
        }
    }

    #[test]
    fn decode_rejects_legacy_raw_join_frame() {
        let err = decode_frame("JOIN room=daily participant=alice").expect_err("legacy frame must fail");
        assert!(matches!(err, CodecError::InvalidJson(_)));
    }

    #[test]
    fn decode_handles_nested_payload_aliases() {
        let frame = decode_frame(
            r#"{"kind":"session_policy","session_policy":{"room_lock":true,"waiting_room_enabled":true,"recording_policy":"started","guest_policy":"inviteOnly","e2ee_required":true,"max_participants":250,"policy_epoch":7,"updated_by":"system","signature":"sig-7"}}"#,
        )
        .expect("decode frame");

        match frame {
            ProtocolFrame::SessionPolicy {
                room_lock,
                waiting_room_enabled,
                recording_policy,
                guest_policy,
                policy_epoch,
                updated_by,
                ..
            } => {
                assert!(room_lock);
                assert!(waiting_room_enabled);
                assert_eq!(recording_policy, RecordingState::Started);
                assert_eq!(guest_policy, GuestPolicy::InviteOnly);
                assert_eq!(policy_epoch, 7);
                assert_eq!(updated_by, "system");
            }
            _ => panic!("expected session policy"),
        }
    }

    #[test]
    fn decode_device_capability_and_pong_from_nested_payload() {
        let capability_frame = decode_frame(
            r#"{"kind":"device_capability","device_capability":{"participant_id":"linux-1","codecs":["h264","vp9"],"hdr_capture":true,"hdr_render":false,"max_streams":3,"updated_at_ms":11}}"#,
        )
        .expect("decode capability frame");

        match capability_frame {
            ProtocolFrame::DeviceCapability {
                participant_id,
                codecs,
                hdr_capture,
                hdr_render,
                max_streams,
                updated_at_ms,
            } => {
                assert_eq!(participant_id, "linux-1");
                assert_eq!(codecs, vec!["h264".to_string(), "vp9".to_string()]);
                assert!(hdr_capture);
                assert!(!hdr_render);
                assert_eq!(max_streams, 3);
                assert_eq!(updated_at_ms, 11);
            }
            _ => panic!("expected device capability"),
        }

        let pong_frame =
            decode_frame(r#"{"kind":"pong","pong":{"sent_at_ms":12}}"#).expect("decode pong frame");
        match pong_frame {
            ProtocolFrame::Pong { sent_at_ms } => assert_eq!(sent_at_ms, 12),
            _ => panic!("expected pong"),
        }
    }

    #[test]
    fn session_policy_signed_by_system_is_accepted() {
        let state = ProtocolSessionState::initial(MeetingConfig::default());
        let connected = reduce(
            state,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::HandshakeAck {
                    session_id: "s1".to_string(),
                    resume_token: "r1".to_string(),
                    accepted_at_ms: 1,
                },
            },
            1,
        );

        let next = reduce(
            connected,
            ProtocolEvent::FrameReceived {
                frame: ProtocolFrame::SessionPolicy {
                    room_lock: true,
                    waiting_room_enabled: true,
                    recording_policy: RecordingState::Started,
                    guest_policy: GuestPolicy::InviteOnly,
                    e2ee_required: false,
                    max_participants: 500,
                    policy_epoch: 5,
                    updated_by: "system".to_string(),
                    signature: Some("sig-system".to_string()),
                    updated_at_ms: 2,
                },
            },
            2,
        );

        assert_eq!(next.connection_phase, ConnectionPhase::Connected);
        assert!(next.room_locked);
        assert!(next.waiting_room_enabled);
        assert_eq!(next.guest_policy, GuestPolicy::InviteOnly);
        assert!(next.last_error.is_none());
    }

    #[test]
    fn fallback_rto_is_recorded_on_recovery() {
        let state = ProtocolSessionState::initial(MeetingConfig::default());
        let active = reduce(
            state,
            ProtocolEvent::FallbackActivated {
                reason: "manual-drill".to_string(),
            },
            1_000,
        );

        let recovered = reduce(active, ProtocolEvent::FallbackRecovered, 1_900);
        assert_eq!(recovered.fallback.last_rto_ms, Some(900));
    }

    #[test]
    fn runtime_schedules_reconnect_with_deterministic_backoff() {
        let mut runtime =
            SessionRuntime::with_backoff(MeetingConfig::default(), vec![1_000, 2_000]);
        runtime.connect_requested(100);

        let directive = runtime.on_transport_failure("network_unavailable", 200);
        assert_eq!(
            directive,
            RuntimeDirective::ReconnectScheduled {
                attempt: 1,
                due_at_ms: 1_200
            }
        );
        assert_eq!(runtime.reconnect_due_at_ms(), Some(1_200));
        assert!(!runtime.take_reconnect_if_due(1_199));
        assert!(runtime.take_reconnect_if_due(1_200));
        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );
    }

    #[test]
    fn runtime_background_defers_reconnect_until_foregrounded() {
        let mut runtime =
            SessionRuntime::with_backoff(MeetingConfig::default(), vec![1_000, 2_000]);
        runtime.connect_requested(0);

        let directive = runtime.on_transport_failure("network_unavailable", 200);
        assert_eq!(
            directive,
            RuntimeDirective::ReconnectScheduled {
                attempt: 1,
                due_at_ms: 1_200
            }
        );
        assert_eq!(runtime.reconnect_due_at_ms(), Some(1_200));

        runtime.on_app_backgrounded(300);
        assert_eq!(runtime.reconnect_due_at_ms(), None);
        assert!(!runtime.take_reconnect_if_due(1_200));

        runtime.on_app_foregrounded(1_300);
        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );
    }

    #[test]
    fn runtime_connectivity_restore_defers_while_backgrounded() {
        let mut runtime = SessionRuntime::with_backoff(MeetingConfig::default(), vec![1_000]);
        runtime.connect_requested(0);
        runtime.on_app_backgrounded(1);

        runtime.on_connectivity_changed(true, 2);
        assert_ne!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );

        runtime.on_app_foregrounded(3);
        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );
    }

    #[test]
    fn runtime_audio_interruption_hooks_emit_telemetry_and_reconnect() {
        let telemetry = Arc::new(InMemoryMeetingTelemetrySink::default());
        let mut runtime = SessionRuntime::with_backoff_and_telemetry(
            MeetingConfig::default(),
            vec![1_000],
            telemetry.clone(),
        );
        runtime.connect_requested(0);

        runtime.on_audio_interruption_began(10);
        runtime.on_audio_interruption_ended(true, 20);

        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );

        let events = telemetry.snapshot();
        assert!(events.iter().any(|event| {
            event.category == MeetingTelemetryCategory::ConnectionLifecycle
                && event.name == "audio_interruption_began"
        }));
        assert!(events.iter().any(|event| {
            event.category == MeetingTelemetryCategory::ConnectionLifecycle
                && event.name == "audio_interruption_ended"
                && event.attributes.get("should_reconnect").map(String::as_str) == Some("true")
        }));
    }

    #[test]
    fn runtime_audio_route_change_emits_telemetry() {
        let telemetry = Arc::new(InMemoryMeetingTelemetrySink::default());
        let mut runtime = SessionRuntime::with_backoff_and_telemetry(
            MeetingConfig::default(),
            vec![1_000],
            telemetry.clone(),
        );

        runtime.on_audio_route_changed("becoming_noisy", 10);

        let events = telemetry.snapshot();
        assert!(events.iter().any(|event| {
            event.category == MeetingTelemetryCategory::ConnectionLifecycle
                && event.name == "audio_route_changed"
                && event.attributes.get("reason").map(String::as_str) == Some("becoming_noisy")
        }));
    }

    #[test]
    fn runtime_activates_fallback_after_backoff_exhaustion() {
        let mut runtime = SessionRuntime::with_backoff(MeetingConfig::default(), vec![50]);
        runtime.connect_requested(0);

        let first = runtime.on_transport_failure("socket_closed", 10);
        assert_eq!(
            first,
            RuntimeDirective::ReconnectScheduled {
                attempt: 1,
                due_at_ms: 60
            }
        );
        assert!(runtime.take_reconnect_if_due(60));

        let second = runtime.on_transport_failure("socket_closed", 70);
        match second {
            RuntimeDirective::FallbackActivated { reason } => {
                assert!(reason.contains("Reconnect exhausted"));
            }
            _ => panic!("expected fallback activation"),
        }
        assert!(runtime.state().fallback.active);
        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::FallbackActive
        );
    }

    #[test]
    fn runtime_builds_handshake_capability_and_payment_frames() {
        let config = MeetingConfig {
            room_id: "ga-room".to_string(),
            participant_id: "linux-guest-1".to_string(),
            participant_name: "Linux Guest".to_string(),
            require_payment_settlement: true,
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        runtime.connect_requested(0);

        runtime.on_frame(
            ProtocolFrame::HandshakeAck {
                session_id: "s1".to_string(),
                resume_token: "resume-1".to_string(),
                accepted_at_ms: 1,
            },
            1,
        );

        let frames = runtime.on_transport_connected(2);
        assert_eq!(frames.len(), 3);
        match &frames[0] {
            ProtocolFrame::Handshake {
                room_id,
                participant_id,
                participant_name,
                resume_token,
                preferred_profile,
                hdr_capture,
                hdr_render,
                ..
            } => {
                assert_eq!(room_id, "ga-room");
                assert_eq!(participant_id, "linux-guest-1");
                assert_eq!(participant_name, "Linux Guest");
                assert_eq!(resume_token.as_deref(), Some("resume-1"));
                assert_eq!(*preferred_profile, MediaProfile::Hdr);
                assert!(*hdr_capture);
                assert!(*hdr_render);
            }
            _ => panic!("expected handshake"),
        }
        match &frames[1] {
            ProtocolFrame::DeviceCapability {
                participant_id,
                hdr_capture,
                hdr_render,
                ..
            } => {
                assert_eq!(participant_id, "linux-guest-1");
                assert!(*hdr_capture);
                assert!(*hdr_render);
            }
            _ => panic!("expected device capability"),
        }
        assert!(matches!(
            frames[2],
            ProtocolFrame::PaymentPolicy { required: true, .. }
        ));
    }

    #[test]
    fn runtime_builds_sdr_handshake_when_hdr_capabilities_disabled() {
        let config = MeetingConfig {
            participant_id: "linux-sdr-guest".to_string(),
            supports_hdr_capture: false,
            supports_hdr_render: false,
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        runtime.connect_requested(0);
        runtime.on_frame(
            ProtocolFrame::HandshakeAck {
                session_id: "s1".to_string(),
                resume_token: "resume-1".to_string(),
                accepted_at_ms: 1,
            },
            1,
        );

        let frames = runtime.on_transport_connected(2);
        assert_eq!(frames.len(), 2);

        match &frames[0] {
            ProtocolFrame::Handshake {
                participant_id,
                preferred_profile,
                hdr_capture,
                hdr_render,
                ..
            } => {
                assert_eq!(participant_id, "linux-sdr-guest");
                assert_eq!(*preferred_profile, MediaProfile::Sdr);
                assert!(!*hdr_capture);
                assert!(!*hdr_render);
            }
            _ => panic!("expected handshake"),
        }

        match &frames[1] {
            ProtocolFrame::DeviceCapability {
                participant_id,
                hdr_capture,
                hdr_render,
                ..
            } => {
                assert_eq!(participant_id, "linux-sdr-guest");
                assert!(!*hdr_capture);
                assert!(!*hdr_render);
            }
            _ => panic!("expected device capability"),
        }
    }

    #[test]
    fn runtime_resolves_participant_id_from_name_when_missing() {
        let config = MeetingConfig {
            participant_id: "   ".to_string(),
            participant_name: "Linux QA 42".to_string(),
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        runtime.connect_requested(0);
        runtime.on_frame(
            ProtocolFrame::HandshakeAck {
                session_id: "s1".to_string(),
                resume_token: "resume-1".to_string(),
                accepted_at_ms: 1,
            },
            1,
        );

        let frames = runtime.on_transport_connected(2);
        assert_eq!(frames.len(), 2);
        match &frames[0] {
            ProtocolFrame::Handshake { participant_id, .. } => {
                assert_eq!(participant_id, "linux-qa-42");
            }
            _ => panic!("expected handshake"),
        }
        match &frames[1] {
            ProtocolFrame::DeviceCapability { participant_id, .. } => {
                assert_eq!(participant_id, "linux-qa-42");
            }
            _ => panic!("expected device capability"),
        }
    }

    #[test]
    fn runtime_falls_back_to_participant_when_explicit_id_normalizes_empty() {
        let config = MeetingConfig {
            participant_id: "###@@@".to_string(),
            participant_name: "Linux QA 42".to_string(),
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        runtime.connect_requested(0);
        runtime.on_frame(
            ProtocolFrame::HandshakeAck {
                session_id: "s1".to_string(),
                resume_token: "resume-1".to_string(),
                accepted_at_ms: 1,
            },
            1,
        );

        let frames = runtime.on_transport_connected(2);
        assert_eq!(frames.len(), 2);
        match &frames[0] {
            ProtocolFrame::Handshake { participant_id, .. } => {
                assert_eq!(participant_id, "participant");
            }
            _ => panic!("expected handshake"),
        }
        match &frames[1] {
            ProtocolFrame::DeviceCapability { participant_id, .. } => {
                assert_eq!(participant_id, "participant");
            }
            _ => panic!("expected device capability"),
        }

        let ack = runtime.on_frame(
            ProtocolFrame::E2eeKeyEpoch {
                epoch: 5,
                issued_by: "host".to_string(),
                signature: Some("sig-5".to_string()),
                sent_at_ms: 500,
            },
            501,
        );
        match &ack[0] {
            ProtocolFrame::KeyRotationAck {
                participant_id,
                ack_epoch,
                ..
            } => {
                assert_eq!(participant_id, "participant");
                assert_eq!(*ack_epoch, 5);
            }
            _ => panic!("expected key rotation ack"),
        }
    }

    #[test]
    fn runtime_replies_to_ping_with_pong() {
        let mut runtime = SessionRuntime::new(MeetingConfig::default());
        let outbound = runtime.on_frame(ProtocolFrame::Ping { sent_at_ms: 10 }, 20);
        assert_eq!(outbound, vec![ProtocolFrame::Pong { sent_at_ms: 20 }]);
    }

    #[test]
    fn runtime_acknowledges_e2ee_key_epoch() {
        let config = MeetingConfig {
            participant_id: "guest".to_string(),
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        let outbound = runtime.on_frame(
            ProtocolFrame::E2eeKeyEpoch {
                epoch: 3,
                issued_by: "host".to_string(),
                signature: Some("sig-3".to_string()),
                sent_at_ms: 10,
            },
            20,
        );

        assert_eq!(
            outbound,
            vec![ProtocolFrame::KeyRotationAck {
                ack_epoch: 3,
                participant_id: "guest".to_string(),
                sent_at_ms: 20,
            }]
        );
        assert_eq!(runtime.state().e2ee_state.current_epoch, 3);
        assert_eq!(runtime.state().e2ee_state.last_ack_epoch, 3);
    }

    #[test]
    fn runtime_does_not_acknowledge_unsigned_e2ee_key_epoch_when_signatures_required() {
        let config = MeetingConfig {
            participant_id: "guest".to_string(),
            require_signed_moderation: true,
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        let first = runtime.on_frame(
            ProtocolFrame::E2eeKeyEpoch {
                epoch: 3,
                issued_by: "host".to_string(),
                signature: Some("sig-3".to_string()),
                sent_at_ms: 10,
            },
            20,
        );
        assert_eq!(first.len(), 1);
        assert_eq!(runtime.state().e2ee_state.last_ack_epoch, 3);

        let second = runtime.on_frame(
            ProtocolFrame::E2eeKeyEpoch {
                epoch: 2,
                issued_by: "host".to_string(),
                signature: None,
                sent_at_ms: 30,
            },
            40,
        );

        assert!(second.is_empty());
        assert_eq!(runtime.state().e2ee_state.last_ack_epoch, 3);
        assert_eq!(
            runtime.state().last_error.as_ref().map(|e| e.code.as_str()),
            Some("e2ee_signature_missing")
        );
    }

    #[test]
    fn runtime_acknowledges_e2ee_key_epoch_with_resolved_participant_id_when_missing() {
        let config = MeetingConfig {
            participant_id: "".to_string(),
            participant_name: "Linux Ops Guest".to_string(),
            ..MeetingConfig::default()
        };
        let mut runtime = SessionRuntime::new(config);
        let outbound = runtime.on_frame(
            ProtocolFrame::E2eeKeyEpoch {
                epoch: 4,
                issued_by: "host".to_string(),
                signature: Some("sig-4".to_string()),
                sent_at_ms: 10,
            },
            20,
        );

        assert_eq!(
            outbound,
            vec![ProtocolFrame::KeyRotationAck {
                ack_epoch: 4,
                participant_id: "linux-ops-guest".to_string(),
                sent_at_ms: 20,
            }]
        );
    }

    #[test]
    fn runtime_manual_disconnect_suppresses_reconnect_schedule() {
        let mut runtime = SessionRuntime::new(MeetingConfig::default());
        runtime.connect_requested(0);
        runtime.on_manual_disconnect(1);

        let directive = runtime.on_transport_failure("network", 2);
        assert_eq!(directive, RuntimeDirective::None);
        assert_eq!(runtime.reconnect_due_at_ms(), None);
    }

    #[test]
    fn recover_from_fallback_resets_reconnect_backoff_attempt() {
        let mut runtime = SessionRuntime::with_backoff(MeetingConfig::default(), vec![50]);
        runtime.connect_requested(0);

        let first = runtime.on_transport_failure("socket_closed", 10);
        assert_eq!(
            first,
            RuntimeDirective::ReconnectScheduled {
                attempt: 1,
                due_at_ms: 60
            }
        );
        assert!(runtime.take_reconnect_if_due(60));

        let exhausted = runtime.on_transport_failure("socket_closed", 70);
        assert!(matches!(
            exhausted,
            RuntimeDirective::FallbackActivated { .. }
        ));
        assert!(runtime.state().fallback.active);

        runtime.recover_from_fallback(100);
        assert!(!runtime.state().fallback.active);
        assert_eq!(
            runtime.state().connection_phase,
            ConnectionPhase::Connecting
        );

        let after_recovery = runtime.on_transport_failure("socket_closed", 110);
        assert_eq!(
            after_recovery,
            RuntimeDirective::ReconnectScheduled {
                attempt: 1,
                due_at_ms: 160
            }
        );
    }

    #[test]
    fn runtime_emits_policy_failure_and_fallback_telemetry_events() {
        let telemetry = Arc::new(InMemoryMeetingTelemetrySink::default());
        let mut runtime = SessionRuntime::with_backoff_and_telemetry(
            MeetingConfig {
                require_payment_settlement: true,
                prefer_web_fallback_on_policy_failure: true,
                ..MeetingConfig::default()
            },
            vec![1_000],
            telemetry.clone(),
        );

        runtime.connect_requested(0);
        runtime.on_frame(
            ProtocolFrame::PaymentPolicy {
                required: true,
                destination_account: Some("nexus://dest".to_string()),
            },
            10,
        );

        let events = telemetry.snapshot();
        let policy_event = events
            .iter()
            .find(|event| {
                event.category == MeetingTelemetryCategory::PolicyFailure
                    && event.name == "payment_settlement_required"
            })
            .expect("policy failure telemetry event should be recorded");
        assert_eq!(
            policy_event.attributes.get("code").map(String::as_str),
            Some("payment_settlement_required")
        );

        let fallback_event = events
            .iter()
            .find(|event| {
                event.category == MeetingTelemetryCategory::FallbackLifecycle
                    && event.name == "fallback_activated"
            })
            .expect("fallback activation telemetry event should be recorded");
        assert_eq!(
            fallback_event.attributes.get("reason").map(String::as_str),
            Some("policy:payment_settlement_required")
        );
    }

    #[test]
    fn runtime_emits_fallback_recovered_rto_telemetry_event() {
        let telemetry = Arc::new(InMemoryMeetingTelemetrySink::default());
        let mut runtime = SessionRuntime::with_backoff_and_telemetry(
            MeetingConfig::default(),
            Vec::new(),
            telemetry.clone(),
        );

        runtime.connect_requested(0);
        let directive = runtime.on_transport_failure("socket_closed", 10);
        assert!(matches!(
            directive,
            RuntimeDirective::FallbackActivated { .. }
        ));
        runtime.recover_from_fallback(120);

        let events = telemetry.snapshot();
        let recovered_event = events
            .iter()
            .find(|event| {
                event.category == MeetingTelemetryCategory::FallbackLifecycle
                    && event.name == "fallback_recovered"
            })
            .expect("fallback recovery telemetry event should be recorded");
        assert_eq!(
            recovered_event.attributes.get("rto_ms").map(String::as_str),
            Some("110")
        );
    }
}
