use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow};
use clap::Parser;
use futures_util::{SinkExt as _, StreamExt as _};
use kaigi_wire::{
    AnonGroupKeyRotateFrame, AnonHelloFrame, AnonRosterEntry, AnonRosterFrame, ChatFrame,
    DeviceCapabilityFrame, E2EEKeyEpochFrame, EncryptedControlFrame, ErrorFrame, EscrowAckFrame,
    EscrowProofFrame, FrameDecoder, GroupKeyUpdateFrame, HelloFrame, KaigiFrame,
    MAX_ANON_PARTICIPANT_HANDLE_LEN, MAX_ESCROW_ID_LEN, MAX_ESCROW_PROOF_HEX_LEN, MediaProfileKind,
    MediaProfileNegotiationFrame, MediaTrackKind, ModerationAction, ModerationFrame,
    ModerationSignedFrame, ModerationTarget, PROTOCOL_VERSION, ParticipantLeftFrame,
    ParticipantPresenceDeltaFrame, ParticipantSnapshot, PaymentAckFrame, PermissionsSnapshotFrame,
    RecordingNoticeFrame, RecordingState, RoleChangeEntry, RoleGrantFrame, RoleKind,
    RoleRevokeFrame, RoomConfigFrame, RoomEventFrame, RosterEntry, RosterFrame, SessionPolicyFrame,
    encode_framed,
};
#[cfg(test)]
use kaigi_wire::{KeyRotationAckFrame, ParticipantStateFrame};
use norito::{
    decode_from_bytes,
    derive::{NoritoDeserialize, NoritoSerialize},
    streaming::SoranetAccessKind,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};
use tracing::{debug, info, warn};

const DEFAULT_XOR_RATE_PER_MINUTE_NANO: u64 = 1_000_000;
const MAX_ENCRYPTED_CIPHERTEXT_HEX_LEN: usize = 64 * 1024;
const MIN_ENCRYPTED_CIPHERTEXT_HEX_LEN: usize = 32;
const MAX_ENCRYPTED_RECIPIENTS_PER_FRAME: usize = 256;
const DEFAULT_ANON_MAX_PARTICIPANTS: usize = 256;
const WARN_ANON_MAX_PARTICIPANTS_THRESHOLD: usize = 1024;
const WARN_ANON_ESCROW_PROOF_STALE_SECS_THRESHOLD: u64 = 3600;
const DEFAULT_MAX_PARTICIPANTS: u32 = 500;
const SYSTEM_POLICY_UPDATED_BY: &str = "system";
const BACKPRESSURE_NOTICE_MIN_INTERVAL_MS: u64 = 5_000;

#[derive(Parser, Debug)]
#[command(
    name = "kaigi-hub-echo",
    version,
    about = "Kaigi hub adapter room hub (dev harness)"
)]
struct Args {
    /// Listen address for the WebSocket server.
    #[arg(long, default_value = "127.0.0.1:9000")]
    listen: SocketAddr,
    /// Log level (defaults to `info`).
    #[arg(long, default_value = "info")]
    log_level: String,
    /// XOR rate per minute in nano-XOR (1e-9 XOR).
    ///
    /// If set to a non-zero value the hub will enforce that participants keep up with
    /// payments signalled via `KaigiFrame::Payment` (dev harness).
    #[arg(long, default_value_t = DEFAULT_XOR_RATE_PER_MINUTE_NANO)]
    xor_rate_per_minute_nano: u64,
    /// Allow zero-rate rooms (free calls). Disabled by default.
    #[arg(long)]
    allow_free_calls: bool,
    /// Allow payment frames that omit `tx_hash_hex` while room rate is non-zero (dev-only).
    #[arg(long)]
    allow_unhashed_payments: bool,
    /// Billing grace window in seconds.
    ///
    /// Participants may temporarily fall behind by up to this many seconds worth of charges
    /// before being disconnected.
    #[arg(long, default_value_t = 30)]
    billing_grace_secs: u64,
    /// How often to check billing state (seconds).
    #[arg(long, default_value_t = 5)]
    billing_check_interval_secs: u64,
    /// Maximum allowed age (seconds) of the latest anonymous escrow proof before disconnect.
    ///
    /// Set to 0 to disable stale-proof enforcement.
    #[arg(long, default_value_t = 90)]
    anon_escrow_proof_max_stale_secs: u64,
    /// Maximum anonymous participants allowed in a single room.
    #[arg(long, default_value_t = DEFAULT_ANON_MAX_PARTICIPANTS)]
    anon_max_participants: usize,
    /// Additional nano-XOR/min surcharge applied when a room first enters anonymous mode.
    #[arg(long, default_value_t = 0)]
    anon_zk_extra_fee_per_minute_nano: u64,
}

type RoomId = [u8; 32];
type ConnId = u64;

#[derive(Clone, Copy, Debug)]
struct BillingConfig {
    default_rate_per_minute_nano: u64,
    anon_zk_extra_fee_per_minute_nano: u64,
    grace_secs: u64,
    check_interval_secs: u64,
    anon_escrow_proof_max_stale_secs: u64,
}

impl BillingConfig {
    fn grace_nano(&self, rate_per_minute_nano: u64) -> u64 {
        // Convert grace time window to nano-XOR at the provided rate.
        let grace = (rate_per_minute_nano as u128).saturating_mul(self.grace_secs as u128) / 60u128;
        grace.min(u64::MAX as u128) as u64
    }
}

#[allow(unexpected_cfgs)]
#[derive(Clone, Debug, NoritoSerialize, NoritoDeserialize)]
struct KaigiStreamOpen {
    channel_id: [u8; 32],
    route_id: [u8; 32],
    stream_id: [u8; 32],
    room_id: [u8; 32],
    authenticated: bool,
    access_kind: SoranetAccessKind,
    exit_token: Vec<u8>,
    exit_multiaddr: String,
}

#[derive(Clone)]
struct HubState {
    rooms: Arc<Mutex<HashMap<RoomId, RoomState>>>,
    next_conn_id: Arc<AtomicU64>,
    billing: BillingConfig,
    anon_max_participants: usize,
    allow_free_calls: bool,
    require_payment_tx_hash: bool,
}

struct RoomState {
    participants: HashMap<ConnId, Participant>,
    device_caps_by_conn: HashMap<ConnId, DeviceCapabilityFrame>,
    host_conn_id: Option<ConnId>,
    host_role_owner: Option<String>,
    co_host_conn_ids: HashSet<ConnId>,
    co_host_role_owners: HashSet<String>,
    e2ee_epochs: HashMap<ConnId, u64>,
    key_rotation_ack_epochs: HashMap<ConnId, u64>,
    signed_action_clock_by_signer: HashMap<String, u64>,
    presence_sequence: u64,
    rate_per_minute_nano: u64,
    max_screen_shares: u8,
    room_lock: bool,
    waiting_room_enabled: bool,
    guest_join_allowed: bool,
    local_recording_allowed: bool,
    e2ee_required: bool,
    max_participants: u32,
    policy_epoch: u64,
    policy_updated_at_ms: u64,
    policy_updated_by: String,
    policy_signature_hex: String,
    permissions_epoch: u64,
    anonymous_mode: bool,
    anon_admission_rejections: u64,
    backpressure_dropped_messages: u64,
    backpressure_last_notice_at_ms: u64,
}

#[derive(Clone)]
struct Participant {
    tx: mpsc::Sender<Message>,
    state: ParticipantState,
}

#[derive(Clone, Debug)]
struct ParticipantState {
    participant_id: String,
    display_name: Option<String>,
    participant_handle: Option<String>,
    x25519_pubkey_hex: Option<String>,
    x25519_epoch: u64,
    mic_enabled: bool,
    video_enabled: bool,
    screen_share_enabled: bool,
    anonymous_mode: bool,
    waiting_room_pending: bool,
    hello_seen: bool,
    last_billed_at_ms: u64,
    billed_nano_xor: u64,
    billing_remainder_mod_60k: u32,
    paid_nano_xor: u64,
    last_payment_at_ms: Option<u64>,
    last_escrow_proof_at_ms: Option<u64>,
    escrow_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing(&args.log_level)?;
    let initial_rate =
        normalize_initial_rate(args.xor_rate_per_minute_nano, args.allow_free_calls)?;
    let anon_max_participants = normalize_anon_max_participants(args.anon_max_participants);

    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("bind {}", args.listen))?;
    info!(
        listen = %args.listen,
        xor_rate_per_minute_nano = initial_rate,
        anon_zk_extra_fee_per_minute_nano = args.anon_zk_extra_fee_per_minute_nano,
        allow_free_calls = args.allow_free_calls,
        billing_grace_secs = args.billing_grace_secs,
        billing_check_interval_secs = args.billing_check_interval_secs,
        anon_escrow_proof_max_stale_secs = args.anon_escrow_proof_max_stale_secs,
        anon_max_participants,
        "kaigi hub listening"
    );
    if should_warn_high_anon_capacity(anon_max_participants) {
        warn!(
            anon_max_participants,
            threshold = WARN_ANON_MAX_PARTICIPANTS_THRESHOLD,
            "high anonymous participant cap configured; expect increased roster and broadcast cost"
        );
    }
    if is_anon_escrow_stale_enforcement_disabled(args.anon_escrow_proof_max_stale_secs) {
        warn!("anonymous escrow stale enforcement disabled (--anon-escrow-proof-max-stale-secs=0)");
    }
    if should_warn_high_anon_escrow_stale_secs(args.anon_escrow_proof_max_stale_secs) {
        warn!(
            anon_escrow_proof_max_stale_secs = args.anon_escrow_proof_max_stale_secs,
            threshold = WARN_ANON_ESCROW_PROOF_STALE_SECS_THRESHOLD,
            "high escrow-proof stale window configured; stale participants may linger longer than expected"
        );
    }

    let state = HubState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        next_conn_id: Arc::new(AtomicU64::new(1)),
        billing: BillingConfig {
            default_rate_per_minute_nano: initial_rate,
            anon_zk_extra_fee_per_minute_nano: args.anon_zk_extra_fee_per_minute_nano,
            grace_secs: args.billing_grace_secs,
            check_interval_secs: args.billing_check_interval_secs,
            anon_escrow_proof_max_stale_secs: args.anon_escrow_proof_max_stale_secs,
        },
        anon_max_participants,
        allow_free_calls: args.allow_free_calls,
        require_payment_tx_hash: !args.allow_unhashed_payments,
    };

    loop {
        let (stream, peer) = listener.accept().await.context("accept")?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_conn(state, stream, peer).await {
                warn!(peer = %peer, error = %err, "hub connection error");
            }
        });
    }
}

async fn handle_conn(state: HubState, stream: TcpStream, peer: SocketAddr) -> Result<()> {
    let mut ws = accept_async(stream)
        .await
        .with_context(|| format!("websocket accept from {peer}"))?;

    let Some(first) = ws.next().await else {
        return Err(anyhow!("connection closed before handshake"));
    };
    let first = first.context("read handshake message")?;
    let Message::Binary(bytes) = first else {
        return Err(anyhow!(
            "expected first message to be binary KaigiStreamOpen"
        ));
    };
    let open: KaigiStreamOpen =
        decode_from_bytes(&bytes).context("decode KaigiStreamOpen (norito)")?;

    let room_id = open.room_id;
    info!(
        peer = %peer,
        channel = %hex::encode(open.channel_id),
        route = %hex::encode(open.route_id),
        stream = %hex::encode(open.stream_id),
        room = %hex::encode(room_id),
        authenticated = open.authenticated,
        access_kind = ?open.access_kind,
        exit_multiaddr = %open.exit_multiaddr,
        exit_token_len = open.exit_token.len(),
        "kaigi stream opened"
    );

    let conn_id = state.next_conn_id.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    let connected_at_ms = now_ms();
    let fallback_participant_id = format!("conn-{conn_id}");
    let participant = Participant {
        tx: tx.clone(),
        state: ParticipantState {
            participant_id: fallback_participant_id,
            display_name: None,
            participant_handle: None,
            x25519_pubkey_hex: None,
            x25519_epoch: 0,
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            anonymous_mode: false,
            waiting_room_pending: false,
            hello_seen: false,
            last_billed_at_ms: connected_at_ms,
            billed_nano_xor: 0,
            billing_remainder_mod_60k: 0,
            paid_nano_xor: 0,
            last_payment_at_ms: None,
            last_escrow_proof_at_ms: None,
            escrow_id: None,
        },
    };

    {
        let mut rooms = state.rooms.lock().await;
        let default_rate = state.billing.default_rate_per_minute_nano;
        let room = rooms.entry(room_id).or_insert_with(|| RoomState {
            participants: HashMap::new(),
            device_caps_by_conn: HashMap::new(),
            host_conn_id: None,
            host_role_owner: None,
            co_host_conn_ids: HashSet::new(),
            co_host_role_owners: HashSet::new(),
            e2ee_epochs: HashMap::new(),
            key_rotation_ack_epochs: HashMap::new(),
            signed_action_clock_by_signer: HashMap::new(),
            presence_sequence: 0,
            rate_per_minute_nano: default_rate,
            max_screen_shares: 1,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: DEFAULT_MAX_PARTICIPANTS,
            policy_epoch: 0,
            policy_updated_at_ms: 0,
            policy_updated_by: SYSTEM_POLICY_UPDATED_BY.to_string(),
            policy_signature_hex: String::new(),
            permissions_epoch: 0,
            anonymous_mode: false,
            anon_admission_rejections: 0,
            backpressure_dropped_messages: 0,
            backpressure_last_notice_at_ms: 0,
        });
        room.participants.insert(conn_id, participant);
    }

    let (mut sink, mut stream) = ws.split();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(err) = sink.send(msg).await {
                debug!(error = %err, "hub writer send failed");
                break;
            }
        }
    });

    let billing = state.billing;
    let billing_state = state.clone();
    let billing_tx = tx.clone();
    tokio::spawn(async move {
        enforce_billing(billing_state, billing_tx, room_id, conn_id, peer, billing).await;
    });

    let mut decoder = FrameDecoder::new();
    while let Some(msg) = stream.next().await {
        let msg = msg.context("ws recv")?;
        match msg {
            Message::Binary(chunk) => {
                decoder.push(&chunk);
                while let Some(frame) = decoder.try_next()? {
                    handle_frame(&state, room_id, conn_id, peer, frame).await?;
                }
            }
            Message::Ping(payload) => {
                let _ = tx.try_send(Message::Pong(payload));
            }
            Message::Pong(_) => {}
            Message::Close(frame) => {
                if let Some(frame) = frame {
                    info!(
                        peer = %peer,
                        code = u16::from(frame.code),
                        reason = %frame.reason,
                        "client closed"
                    );
                } else {
                    info!(peer = %peer, "client closed");
                }
                break;
            }
            Message::Text(text) => {
                warn!(peer = %peer, "ignoring text frame: {text}");
            }
            Message::Frame(_) => {}
        }
    }

    drop(tx);
    let _ = writer.await;

    handle_disconnect(&state, room_id, conn_id).await?;

    Ok(())
}

async fn handle_disconnect(state: &HubState, room_id: RoomId, conn_id: ConnId) -> Result<()> {
    let (
        left_participant_id,
        left_participant_handle,
        remove_room,
        host_changed,
        anonymous_mode,
        presence_delta,
    ) = {
        let mut rooms = state.rooms.lock().await;
        let Some(room) = rooms.get_mut(&room_id) else {
            return Ok(());
        };
        let left = room.participants.remove(&conn_id);
        room.device_caps_by_conn.remove(&conn_id);
        room.co_host_conn_ids.remove(&conn_id);
        room.e2ee_epochs.remove(&conn_id);
        room.key_rotation_ack_epochs.remove(&conn_id);
        let left_id = left.as_ref().map(|p| p.state.participant_id.clone());
        let left_handle = left
            .as_ref()
            .and_then(|p| p.state.participant_handle.clone());
        let remove_room = room.participants.is_empty();
        let anonymous_mode = room.anonymous_mode;
        if remove_room {
            rooms.remove(&room_id);
            (left_id, left_handle, true, false, anonymous_mode, None)
        } else {
            let mut host_changed = false;
            let mut presence_delta = None;
            if !room.anonymous_mode && room.host_conn_id == Some(conn_id) {
                let new_host = room.participants.keys().min().copied();
                room.host_conn_id = new_host;
                sync_active_co_host_bindings(room);
                room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                host_changed = true;
            }
            if !room.anonymous_mode
                && let Some(ref participant_id) = left_id
            {
                room.presence_sequence = room.presence_sequence.saturating_add(1);
                presence_delta = Some(ParticipantPresenceDeltaFrame {
                    at_ms: now_ms(),
                    sequence: room.presence_sequence,
                    joined: Vec::new(),
                    left: vec![ParticipantLeftFrame {
                        at_ms: now_ms(),
                        participant_id: participant_id.clone(),
                    }],
                    role_changes: Vec::new(),
                });
            }
            (
                left_id,
                left_handle,
                false,
                host_changed,
                anonymous_mode,
                presence_delta,
            )
        }
    };

    if remove_room {
        return Ok(());
    }

    if anonymous_mode {
        let roster = anon_roster_frame(state, room_id).await?;
        broadcast_frame(state, &room_id, &KaigiFrame::AnonRoster(roster)).await?;
        if let Some(handle) = left_participant_handle {
            let update = KaigiFrame::GroupKeyUpdate(GroupKeyUpdateFrame {
                sent_at_ms: now_ms(),
                participant_handle: handle,
                x25519_pubkey_hex: String::new(),
                epoch: 0,
            });
            broadcast_frame(state, &room_id, &update).await?;
        }
        let rotate = {
            let mut rooms = state.rooms.lock().await;
            let Some(room) = rooms.get_mut(&room_id) else {
                return Ok(());
            };
            build_anon_group_key_rotate_frame(room, now_ms())
        };
        broadcast_frame(state, &room_id, &KaigiFrame::AnonGroupKeyRotate(rotate)).await?;
    } else if let Some(participant_id) = left_participant_id {
        let at_ms = now_ms();
        let event = KaigiFrame::Event(RoomEventFrame::Left(ParticipantLeftFrame {
            at_ms,
            participant_id,
        }));
        broadcast_frame(state, &room_id, &event).await?;
        if let Some(delta) = presence_delta {
            broadcast_frame(
                state,
                &room_id,
                &KaigiFrame::ParticipantPresenceDelta(delta),
            )
            .await?;
        }
    }

    if host_changed && !anonymous_mode {
        let cfg = room_config_frame(state, room_id).await?;
        broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
        broadcast_permissions_snapshots(state, room_id).await?;
    }
    Ok(())
}

async fn enforce_billing(
    state: HubState,
    tx: mpsc::Sender<Message>,
    room_id: RoomId,
    conn_id: ConnId,
    peer: SocketAddr,
    billing: BillingConfig,
) {
    let interval = Duration::from_secs(billing.check_interval_secs.max(1));

    loop {
        tokio::time::sleep(interval).await;

        let now = now_ms();
        let mut disconnect_message: Option<String> = None;
        {
            let mut rooms = state.rooms.lock().await;
            let Some(room) = rooms.get_mut(&room_id) else {
                return;
            };
            let room_anonymous = room.anonymous_mode;
            let Some(participant) = room.participants.get_mut(&conn_id) else {
                return;
            };
            if room_anonymous || participant.state.anonymous_mode {
                if billing.anon_escrow_proof_max_stale_secs > 0 && !participant.state.hello_seen {
                    let connected_at = participant.state.last_billed_at_ms;
                    if anonymous_hello_timed_out(
                        connected_at,
                        now,
                        billing.anon_escrow_proof_max_stale_secs,
                    ) {
                        disconnect_message = Some(format!(
                            "anonymous hello timeout: connected_at_ms={connected_at} max_wait_secs={}",
                            billing.anon_escrow_proof_max_stale_secs
                        ));
                    }
                }
                if disconnect_message.is_none()
                    && billing.anon_escrow_proof_max_stale_secs > 0
                    && participant.state.hello_seen
                {
                    let last = participant
                        .state
                        .last_escrow_proof_at_ms
                        .unwrap_or(participant.state.last_billed_at_ms);
                    if escrow_proof_stale(last, now, billing.anon_escrow_proof_max_stale_secs) {
                        disconnect_message = Some(format!(
                            "anonymous escrow proof stale: last_proof_at_ms={last} max_stale_secs={}",
                            billing.anon_escrow_proof_max_stale_secs
                        ));
                    }
                }
                if disconnect_message.is_some() {
                    // continue to disconnect block below
                } else {
                    continue;
                }
            }
            let rate = room.rate_per_minute_nano;
            if participant.state.hello_seen {
                advance_billing(&mut participant.state, rate, now);
            } else {
                // Don't bill before `Hello` is processed, but keep the cursor up to date.
                participant.state.last_billed_at_ms = now;
            }
            let billed_nano_xor = participant.state.billed_nano_xor;
            let paid_nano_xor = participant.state.paid_nano_xor;
            let grace_nano = billing.grace_nano(rate);
            if participant.state.hello_seen
                && rate > 0
                && billed_nano_xor > paid_nano_xor.saturating_add(grace_nano)
            {
                disconnect_message = Some(format!(
                    "payment required: billed_nano_xor={billed_nano_xor} paid_nano_xor={paid_nano_xor} grace_nano_xor={grace_nano} rate_per_minute_nano={rate}"
                ));
            }
        }

        if let Some(message) = disconnect_message {
            let frame = KaigiFrame::Error(ErrorFrame {
                message: message.clone(),
            });
            if let Ok(bytes) = encode_framed(&frame) {
                let _ = tx.try_send(Message::Binary(bytes.into()));
            }
            let _ = tx.try_send(Message::Close(None));
            warn!(
                peer = %peer,
                room = %hex::encode(room_id),
                conn_id,
                reason = %message,
                "disconnecting due to billing/escrow policy"
            );
            return;
        }
    }
}

fn advance_billing(state: &mut ParticipantState, rate_per_minute_nano: u64, now_ms: u64) {
    let elapsed_ms = now_ms.saturating_sub(state.last_billed_at_ms);
    state.last_billed_at_ms = now_ms;
    if elapsed_ms == 0 || rate_per_minute_nano == 0 {
        return;
    }

    // Integrate per-ms billing with remainder carry so we don't lose precision across ticks.
    let numerator = (rate_per_minute_nano as u128)
        .saturating_mul(elapsed_ms as u128)
        .saturating_add(u128::from(state.billing_remainder_mod_60k));
    let delta_nano = numerator / 60_000u128;
    state.billing_remainder_mod_60k = (numerator % 60_000u128) as u32;
    let delta_nano = delta_nano.min(u64::MAX as u128) as u64;
    state.billed_nano_xor = state.billed_nano_xor.saturating_add(delta_nano);
}

async fn handle_frame(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
    peer: SocketAddr,
    frame: KaigiFrame,
) -> Result<()> {
    match frame {
        KaigiFrame::AnonHello(hello) => {
            if hello.protocol_version != PROTOCOL_VERSION {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    format!("unsupported protocol_version {}", hello.protocol_version),
                )
                .await?;
                return Ok(());
            }
            if !is_valid_hex_len(&hello.x25519_pubkey_hex, 32) {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "x25519_pubkey_hex must be 32-byte hex".to_string(),
                )
                .await?;
                return Ok(());
            }

            let mut anon_cap_rejection_count: Option<u64> = None;
            let (roster, update, rotate, mode_error, surcharge_applied) = {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                if let Some(handle_error) =
                    validate_anon_participant_handle(room, conn_id, &hello.participant_handle)
                {
                    (None, None, None, Some(handle_error), None)
                } else if let Some(cap_error) =
                    validate_anonymous_room_capacity(room, conn_id, state.anon_max_participants)
                {
                    anon_cap_rejection_count = Some(record_anon_admission_rejection(room));
                    (None, None, None, Some(cap_error), None)
                } else if !room.anonymous_mode {
                    let transparent_hello_seen = room
                        .participants
                        .values()
                        .any(|p| p.state.hello_seen && !p.state.anonymous_mode);
                    if transparent_hello_seen {
                        (
                            None,
                            None,
                            None,
                            Some("room is already in transparent mode".to_string()),
                            None,
                        )
                    } else {
                        let base_rate = room.rate_per_minute_nano;
                        match apply_anonymous_zk_surcharge(
                            base_rate,
                            state.billing.anon_zk_extra_fee_per_minute_nano,
                        ) {
                            Err(err) => (None, None, None, Some(err), None),
                            Ok(effective_rate) => {
                                let Some(participant) = room.participants.get_mut(&conn_id) else {
                                    return Ok(());
                                };
                                let now = now_ms();
                                if let Err(err) =
                                    apply_anon_hello_state(&mut participant.state, &hello, now)
                                {
                                    (None, None, None, Some(err), None)
                                } else {
                                    let epoch = participant.state.x25519_epoch;
                                    room.rate_per_minute_nano = effective_rate;
                                    room.anonymous_mode = true;
                                    let roster = anon_roster_frame_locked(room);
                                    let update = GroupKeyUpdateFrame {
                                        sent_at_ms: now,
                                        participant_handle: hello.participant_handle.clone(),
                                        x25519_pubkey_hex: hello.x25519_pubkey_hex.clone(),
                                        epoch,
                                    };
                                    let rotate = build_anon_group_key_rotate_frame(room, now);
                                    (
                                        Some(roster),
                                        Some(update),
                                        Some(rotate),
                                        None,
                                        (base_rate != effective_rate)
                                            .then_some((base_rate, effective_rate)),
                                    )
                                }
                            }
                        }
                    }
                } else {
                    let Some(participant) = room.participants.get_mut(&conn_id) else {
                        return Ok(());
                    };
                    let now = now_ms();
                    if let Err(err) = apply_anon_hello_state(&mut participant.state, &hello, now) {
                        (None, None, None, Some(err), None)
                    } else {
                        let epoch = participant.state.x25519_epoch;
                        let roster = anon_roster_frame_locked(room);
                        let update = GroupKeyUpdateFrame {
                            sent_at_ms: now,
                            participant_handle: hello.participant_handle.clone(),
                            x25519_pubkey_hex: hello.x25519_pubkey_hex.clone(),
                            epoch,
                        };
                        let rotate = build_anon_group_key_rotate_frame(room, now);
                        (Some(roster), Some(update), Some(rotate), None, None)
                    }
                }
            };

            if let Some(err) = mode_error {
                if let Some(rejections) = anon_cap_rejection_count {
                    warn!(
                        peer = %peer,
                        room = %hex::encode(room_id),
                        conn_id,
                        max_anon_participants = state.anon_max_participants,
                        anon_admission_rejections = rejections,
                        "anonymous admission rejected at room capacity"
                    );
                }
                send_error(state, room_id, conn_id, err).await?;
                return Ok(());
            }

            if let Some(roster) = roster {
                send_frame_to(state, room_id, conn_id, &KaigiFrame::AnonRoster(roster)).await?;
            }
            if let Some(update) = update {
                broadcast_frame(state, &room_id, &KaigiFrame::GroupKeyUpdate(update)).await?;
            }
            if let Some(rotate) = rotate {
                broadcast_frame(state, &room_id, &KaigiFrame::AnonGroupKeyRotate(rotate)).await?;
            }
            if let Some((base_rate, effective_rate)) = surcharge_applied {
                info!(
                    peer = %peer,
                    room = %hex::encode(room_id),
                    base_rate_per_minute_nano = base_rate,
                    anon_zk_extra_fee_per_minute_nano = state.billing.anon_zk_extra_fee_per_minute_nano,
                    effective_rate_per_minute_nano = effective_rate,
                    "anonymous room surcharge applied"
                );
            }
            info!(peer = %peer, room = %hex::encode(room_id), "anon hello received");
        }
        KaigiFrame::GroupKeyUpdate(update) => {
            let mut send_error_msg: Option<String> = None;
            let mut should_broadcast = false;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                if !room.anonymous_mode {
                    send_error_msg =
                        Some("group key updates require anonymous room mode".to_string());
                } else if let Some(validation_err) = validate_group_key_update_frame(&update) {
                    send_error_msg = Some(validation_err);
                } else if let Some(participant) = room.participants.get_mut(&conn_id) {
                    if !participant.state.hello_seen || !participant.state.anonymous_mode {
                        send_error_msg = Some("send AnonHello first".to_string());
                    } else if participant.state.participant_handle.as_deref()
                        != Some(update.participant_handle.as_str())
                    {
                        send_error_msg =
                            Some("group key sender handle does not match connection".to_string());
                    } else {
                        match should_apply_group_key_update(
                            participant.state.x25519_epoch,
                            participant.state.x25519_pubkey_hex.as_deref(),
                            update.epoch,
                            &update.x25519_pubkey_hex,
                        ) {
                            Ok(should_apply) => {
                                if should_apply {
                                    participant.state.x25519_pubkey_hex =
                                        Some(update.x25519_pubkey_hex.clone());
                                    participant.state.x25519_epoch = update.epoch;
                                    should_broadcast = true;
                                }
                            }
                            Err(err) => {
                                send_error_msg = Some(err);
                            }
                        }
                    }
                }
            }

            if let Some(msg) = send_error_msg {
                send_error(state, room_id, conn_id, msg).await?;
                return Ok(());
            }
            if should_broadcast {
                broadcast_frame(state, &room_id, &KaigiFrame::GroupKeyUpdate(update)).await?;
            }
        }
        KaigiFrame::EncryptedControl(enc) => {
            let mut send_error_msg: Option<String> = None;
            let mut should_broadcast = false;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                if !room.anonymous_mode {
                    send_error_msg =
                        Some("encrypted control frames require anonymous room mode".to_string());
                } else if let Some(validation_err) = validate_encrypted_control_frame(&enc) {
                    send_error_msg = Some(validation_err);
                } else if let Some(participant) = room.participants.get(&conn_id) {
                    if !participant.state.hello_seen || !participant.state.anonymous_mode {
                        send_error_msg = Some("send AnonHello first".to_string());
                    } else if participant.state.participant_handle.as_deref()
                        != Some(enc.sender_handle.as_str())
                    {
                        send_error_msg =
                            Some("encrypted sender handle does not match connection".to_string());
                    } else if let Some(epoch_err) =
                        validate_encrypted_control_epoch(enc.epoch, participant.state.x25519_epoch)
                    {
                        send_error_msg = Some(epoch_err);
                    } else if let Some(recipient_err) =
                        validate_encrypted_control_room_recipients(&enc, room)
                    {
                        send_error_msg = Some(recipient_err);
                    } else {
                        should_broadcast = true;
                    }
                }
            }

            if let Some(msg) = send_error_msg {
                send_error(state, room_id, conn_id, msg).await?;
                return Ok(());
            }
            if should_broadcast {
                broadcast_frame(state, &room_id, &KaigiFrame::EncryptedControl(enc)).await?;
            }
        }
        KaigiFrame::EscrowProof(proof) => {
            let (accepted, reason) = {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(participant) = room.participants.get_mut(&conn_id) else {
                    return Ok(());
                };
                if !room.anonymous_mode || !participant.state.anonymous_mode {
                    (
                        false,
                        Some("escrow proofs require anonymous room mode".to_string()),
                    )
                } else if !participant.state.hello_seen {
                    (false, Some("send AnonHello first".to_string()))
                } else if participant.state.participant_handle.as_deref()
                    != Some(proof.payer_handle.as_str())
                {
                    (
                        false,
                        Some("escrow proof payer handle does not match connection".to_string()),
                    )
                } else if let Some(validation_err) = validate_escrow_proof_frame(&proof) {
                    (false, Some(validation_err))
                } else if let Some(mismatch_err) = validate_escrow_id_consistency(
                    participant.state.escrow_id.as_deref(),
                    &proof.escrow_id,
                ) {
                    (false, Some(mismatch_err))
                } else {
                    participant.state.last_escrow_proof_at_ms = Some(now_ms());
                    participant
                        .state
                        .escrow_id
                        .get_or_insert_with(|| proof.escrow_id.clone());
                    (true, None)
                }
            };
            let ack = KaigiFrame::EscrowAck(EscrowAckFrame {
                received_at_ms: now_ms(),
                escrow_id: proof.escrow_id,
                accepted,
                reason: reason.clone(),
            });
            send_frame_to(state, room_id, conn_id, &ack).await?;
            if let Some(reason) = reason {
                warn!(peer = %peer, room = %hex::encode(room_id), reason, "escrow proof rejected");
            }
        }
        KaigiFrame::EscrowAck(_) => {}
        KaigiFrame::AnonRoster(_) => {}
        KaigiFrame::Hello(mut hello) => {
            if hello.protocol_version != PROTOCOL_VERSION {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    format!("unsupported protocol_version {}", hello.protocol_version),
                )
                .await?;
                return Ok(());
            }

            // Enforce "mic/cam/share off on join".
            let forced_off = enforce_join_media_defaults(&mut hello);

            let (snapshot, mode_conflict, join_denied_reason, waiting_room_notice, presence_delta) = {
                let mut rooms = state.rooms.lock().await;
                let room = rooms
                    .get_mut(&room_id)
                    .ok_or_else(|| anyhow!("room missing"))?;
                if room.anonymous_mode {
                    (
                        ParticipantSnapshot {
                            at_ms: now_ms(),
                            participant_id: String::new(),
                            display_name: None,
                            mic_enabled: false,
                            video_enabled: false,
                            screen_share_enabled: false,
                        },
                        true,
                        None,
                        None,
                        None,
                    )
                } else if let Some(reason) =
                    validate_join_allowed(room, conn_id, &hello.participant_id)
                {
                    (
                        ParticipantSnapshot {
                            at_ms: now_ms(),
                            participant_id: String::new(),
                            display_name: None,
                            mic_enabled: false,
                            video_enabled: false,
                            screen_share_enabled: false,
                        },
                        false,
                        Some(reason),
                        None,
                        None,
                    )
                } else {
                    if room.host_conn_id.is_none() {
                        room.host_conn_id = Some(conn_id);
                        room.host_role_owner = Some(hello.participant_id.clone());
                        sync_active_co_host_bindings(room);
                        room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                    }
                    let already_joined = room
                        .participants
                        .get(&conn_id)
                        .is_some_and(|participant| participant.state.hello_seen);
                    let has_moderation_role =
                        conn_or_participant_can_moderate(room, conn_id, &hello.participant_id);
                    let route_to_waiting_room = !already_joined
                        && room.waiting_room_enabled
                        && room.host_conn_id.is_some()
                        && !has_moderation_role;
                    let joined_state = {
                        let participant = room
                            .participants
                            .get_mut(&conn_id)
                            .ok_or_else(|| anyhow!("participant missing"))?;
                        participant.state.participant_id = hello.participant_id.clone();
                        participant.state.display_name = hello.display_name.clone();
                        participant.state.participant_handle = None;
                        participant.state.x25519_pubkey_hex = None;
                        participant.state.x25519_epoch = 0;
                        participant.state.anonymous_mode = false;
                        participant.state.mic_enabled = hello.mic_enabled;
                        participant.state.video_enabled = hello.video_enabled;
                        participant.state.screen_share_enabled = hello.screen_share_enabled;
                        participant.state.waiting_room_pending = route_to_waiting_room;
                        participant.state.hello_seen = !route_to_waiting_room;
                        participant.state.last_escrow_proof_at_ms = None;
                        participant.state.escrow_id = None;
                        participant.state.clone()
                    };
                    if route_to_waiting_room {
                        (
                            ParticipantSnapshot {
                                at_ms: now_ms(),
                                participant_id: String::new(),
                                display_name: None,
                                mic_enabled: false,
                                video_enabled: false,
                                screen_share_enabled: false,
                            },
                            false,
                            None,
                            Some(format!(
                                "waiting room: pending admission for participant_id={}",
                                hello.participant_id
                            )),
                            None,
                        )
                    } else {
                        room.e2ee_epochs.entry(conn_id).or_insert(0);
                        if restore_reserved_roles_on_join(room, conn_id, &hello.participant_id) {
                            room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                        }
                        room.presence_sequence = room.presence_sequence.saturating_add(1);
                        let snapshot = participant_snapshot_from_state(&joined_state, now_ms());
                        let presence_delta = ParticipantPresenceDeltaFrame {
                            at_ms: now_ms(),
                            sequence: room.presence_sequence,
                            joined: vec![snapshot.clone()],
                            left: Vec::new(),
                            role_changes: Vec::new(),
                        };
                        (snapshot, false, None, None, Some(presence_delta))
                    }
                }
            };

            if mode_conflict {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "room is already in anonymous mode".to_string(),
                )
                .await?;
                return Ok(());
            }
            if let Some(reason) = join_denied_reason {
                send_error(state, room_id, conn_id, reason).await?;
                return Ok(());
            }
            if let Some(waiting_notice) = waiting_room_notice {
                notify_moderators(state, room_id, &waiting_notice).await?;
                send_error(state, room_id, conn_id, waiting_notice).await?;
                return Ok(());
            }

            if forced_off {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "mic/video/screen share must start disabled; forced off".to_string(),
                )
                .await?;
            }

            // Send roster to the joining participant.
            let roster = roster_frame(state, room_id).await?;
            send_frame_to(state, room_id, conn_id, &KaigiFrame::Roster(roster)).await?;

            // Broadcast join event.
            broadcast_frame(
                state,
                &room_id,
                &KaigiFrame::Event(RoomEventFrame::Joined(snapshot)),
            )
            .await?;
            if let Some(delta) = presence_delta {
                broadcast_frame(
                    state,
                    &room_id,
                    &KaigiFrame::ParticipantPresenceDelta(delta),
                )
                .await?;
            }

            // Broadcast room config (host, rate, etc).
            let cfg = room_config_frame(state, room_id).await?;
            broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
            let policy = session_policy_frame(state, room_id).await?;
            send_frame_to(state, room_id, conn_id, &KaigiFrame::SessionPolicy(policy)).await?;
            broadcast_permissions_snapshots(state, room_id).await?;

            info!(peer = %peer, room = %hex::encode(room_id), "hello received");
        }
        KaigiFrame::Chat(chat) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode requires EncryptedControl frames".to_string(),
                )
                .await?;
                return Ok(());
            }
            if !hello_seen(state, room_id, conn_id).await {
                send_error(state, room_id, conn_id, "send Hello first".to_string()).await?;
                return Ok(());
            }
            if let Some(message) = validate_plaintext_e2ee_gate(state, room_id, conn_id).await {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let at_ms = now_ms();
            let (from_id, from_name) = participant_identity(state, room_id, conn_id).await;
            let message = KaigiFrame::Chat(ChatFrame {
                sent_at_ms: at_ms,
                from_participant_id: from_id,
                from_display_name: from_name,
                text: chat.text,
            });
            broadcast_frame(state, &room_id, &message).await?;
        }
        KaigiFrame::ParticipantState(update) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode requires EncryptedControl frames".to_string(),
                )
                .await?;
                return Ok(());
            }
            let at_ms = now_ms();
            let mut deny_share_max: Option<u8> = None;
            let snapshot = {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(existing) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !existing.state.hello_seen {
                    drop(rooms);
                    send_error(state, room_id, conn_id, "send Hello first".to_string()).await?;
                    return Ok(());
                }
                if room.e2ee_required && room.e2ee_epochs.get(&conn_id).copied().unwrap_or(0) == 0 {
                    drop(rooms);
                    send_error(
                        state,
                        room_id,
                        conn_id,
                        "e2ee required: publish E2EEKeyEpoch before plaintext control".to_string(),
                    )
                    .await?;
                    return Ok(());
                }

                let mut apply_screen_share: Option<bool> = None;
                if let Some(value) = update.screen_share_enabled {
                    if value {
                        if existing.state.screen_share_enabled {
                            apply_screen_share = Some(true);
                        } else {
                            let active = room
                                .participants
                                .iter()
                                .filter(|(id, p)| **id != conn_id && p.state.screen_share_enabled)
                                .count();
                            let max_screen_shares = room.max_screen_shares.max(1);
                            if active >= max_screen_shares as usize {
                                deny_share_max = Some(max_screen_shares);
                            } else {
                                apply_screen_share = Some(true);
                            }
                        }
                    } else {
                        apply_screen_share = Some(false);
                    }
                }

                let Some(participant) = room.participants.get_mut(&conn_id) else {
                    return Ok(());
                };
                if let Some(value) = update.mic_enabled {
                    participant.state.mic_enabled = value;
                }
                if let Some(value) = update.video_enabled {
                    participant.state.video_enabled = value;
                }
                if let Some(value) = apply_screen_share {
                    participant.state.screen_share_enabled = value;
                }
                participant_snapshot_from_state(&participant.state, at_ms)
            };

            if let Some(max_screen_shares) = deny_share_max {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    format!(
                        "screen share denied: max_screen_shares={} already in use",
                        max_screen_shares
                    ),
                )
                .await?;
            }

            broadcast_frame(
                state,
                &room_id,
                &KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot)),
            )
            .await?;
        }
        KaigiFrame::MediaCapability(cap) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext media capabilities".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !sender.state.hello_seen {
                    error = Some("send Hello first".to_string());
                } else if sender.state.participant_id != cap.participant_id {
                    error = Some("participant_id must match sender participant_id".to_string());
                } else if cap.max_video_width == 0
                    || cap.max_video_height == 0
                    || cap.max_video_fps == 0
                {
                    error = Some("media capability dimensions/fps must be > 0".to_string());
                } else if cap.audio_sample_rate == 0 || cap.audio_channels == 0 {
                    error = Some("audio sample rate/channels must be > 0".to_string());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::MediaCapability(cap)).await?;
        }
        KaigiFrame::MediaTrackState(track) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext media track states".to_string(),
                )
                .await?;
                return Ok(());
            }
            if let Some(message) = validate_plaintext_e2ee_gate(state, room_id, conn_id).await {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let at_ms = now_ms();
            let mut error: Option<String> = None;
            let mut snapshot: Option<ParticipantSnapshot> = None;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !sender.state.hello_seen {
                    error = Some("send Hello first".to_string());
                } else if sender.state.participant_id != track.participant_id {
                    error = Some("participant_id must match sender participant_id".to_string());
                }
                if error.is_none()
                    && let Some(participant) = room.participants.get_mut(&conn_id)
                {
                    participant.state.mic_enabled = track.mic_enabled;
                    match track.active_video_track {
                        MediaTrackKind::Camera => {
                            participant.state.video_enabled = track.camera_enabled;
                            participant.state.screen_share_enabled = false;
                        }
                        MediaTrackKind::ScreenShare => {
                            participant.state.video_enabled = false;
                            participant.state.screen_share_enabled = track.screen_share_enabled;
                        }
                    }
                    snapshot = Some(participant_snapshot_from_state(&participant.state, at_ms));
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::MediaTrackState(track)).await?;
            if let Some(snapshot) = snapshot {
                broadcast_frame(
                    state,
                    &room_id,
                    &KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot)),
                )
                .await?;
            }
        }
        KaigiFrame::VideoSegment(segment) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode requires encrypted media payloads".to_string(),
                )
                .await?;
                return Ok(());
            }
            if let Some(message) = validate_plaintext_e2ee_gate(state, room_id, conn_id).await {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !sender.state.hello_seen {
                    error = Some("send Hello first".to_string());
                } else if sender.state.participant_id != segment.participant_id {
                    error = Some("participant_id must match sender participant_id".to_string());
                } else if segment.frame_width == 0
                    || segment.frame_height == 0
                    || segment.frame_duration_ns == 0
                {
                    error = Some(
                        "video segment frame_width/frame_height/frame_duration_ns must be > 0"
                            .to_string(),
                    );
                } else if segment.payload.is_empty() {
                    error = Some("video segment payload must not be empty".to_string());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::VideoSegment(segment)).await?;
        }
        KaigiFrame::AudioPacket(packet) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode requires encrypted media payloads".to_string(),
                )
                .await?;
                return Ok(());
            }
            if let Some(message) = validate_plaintext_e2ee_gate(state, room_id, conn_id).await {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !sender.state.hello_seen {
                    error = Some("send Hello first".to_string());
                } else if sender.state.participant_id != packet.participant_id {
                    error = Some("participant_id must match sender participant_id".to_string());
                } else if packet.sample_rate == 0
                    || packet.channels == 0
                    || packet.frame_samples == 0
                {
                    error = Some(
                        "audio packet sample_rate/channels/frame_samples must be > 0".to_string(),
                    );
                } else if packet.payload.is_empty() {
                    error = Some("audio packet payload must not be empty".to_string());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::AudioPacket(packet)).await?;
        }
        KaigiFrame::AnonGroupKeyRotate(_) => {
            send_error(
                state,
                room_id,
                conn_id,
                "anonymous group-key rotation frames are hub-managed".to_string(),
            )
            .await?;
        }
        KaigiFrame::AnonEncryptedPayload(enc) => {
            let mut send_error_msg: Option<String> = None;
            let mut should_broadcast = false;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                if !room.anonymous_mode {
                    send_error_msg = Some(
                        "anonymous encrypted payloads require anonymous room mode".to_string(),
                    );
                } else if !is_valid_hex_len(&enc.nonce_hex, 24) {
                    send_error_msg = Some("nonce_hex must be 24-byte hex".to_string());
                } else if !is_valid_hex(&enc.ciphertext_hex) {
                    send_error_msg = Some("ciphertext_hex must be valid hex".to_string());
                } else if let Some(participant) = room.participants.get(&conn_id) {
                    if !participant.state.hello_seen || !participant.state.anonymous_mode {
                        send_error_msg = Some("send AnonHello first".to_string());
                    } else if participant.state.participant_handle.as_deref()
                        != Some(enc.sender_handle.as_str())
                    {
                        send_error_msg = Some(
                            "anonymous encrypted sender_handle does not match connection"
                                .to_string(),
                        );
                    } else if enc.epoch == 0 {
                        send_error_msg = Some("epoch must be >= 1".to_string());
                    } else {
                        should_broadcast = true;
                    }
                }
            }

            if let Some(msg) = send_error_msg {
                send_error(state, room_id, conn_id, msg).await?;
                return Ok(());
            }
            if should_broadcast {
                broadcast_frame(state, &room_id, &KaigiFrame::AnonEncryptedPayload(enc)).await?;
            }
        }
        KaigiFrame::RoomConfigUpdate(update) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext room config updates".to_string(),
                )
                .await?;
                return Ok(());
            }
            let now = now_ms();
            let mut changed = false;
            let mut rejected_zero_rate = false;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(sender_hello_seen) =
                    room.participants.get(&conn_id).map(|p| p.state.hello_seen)
                else {
                    return Ok(());
                };
                if !sender_hello_seen {
                    drop(rooms);
                    send_error(state, room_id, conn_id, "send Hello first".to_string()).await?;
                    return Ok(());
                }
                if room.host_conn_id != Some(conn_id) {
                    drop(rooms);
                    send_error(state, room_id, conn_id, "host only".to_string()).await?;
                    return Ok(());
                }

                let current_rate = room.rate_per_minute_nano;
                for participant in room.participants.values_mut() {
                    advance_billing(&mut participant.state, current_rate, now);
                }

                if let Some(rate) = update.rate_per_minute_nano {
                    let normalized_rate = normalize_rate_for_policy(rate, state.allow_free_calls);
                    if rate == 0 && !state.allow_free_calls {
                        rejected_zero_rate = true;
                    }
                    if room.rate_per_minute_nano != normalized_rate {
                        room.rate_per_minute_nano = normalized_rate;
                        changed = true;
                    }
                }
                if let Some(max) = update.max_screen_shares {
                    let normalized_max = max.max(1);
                    if room.max_screen_shares != normalized_max {
                        room.max_screen_shares = normalized_max;
                        changed = true;
                    }
                }
            }

            if rejected_zero_rate {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "rate_per_minute_nano=0 rejected: paid calls are required unless hub started with --allow-free-calls".to_string(),
                )
                .await?;
            }

            if changed {
                let cfg = room_config_frame(state, room_id).await?;
                broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
            }
        }
        KaigiFrame::RoleGrant(grant) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext role grants".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut host_changed = false;
            let mut permissions_changed = false;
            let mut presence_delta: Option<ParticipantPresenceDeltaFrame> = None;
            let mut error: Option<String> = None;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                match apply_role_grant(room, conn_id, &grant) {
                    Ok(outcome) => {
                        host_changed = outcome.host_changed;
                        permissions_changed = outcome.permissions_changed;
                        if let Some(role_change_entry) = outcome.role_change {
                            room.presence_sequence = room.presence_sequence.saturating_add(1);
                            presence_delta = Some(ParticipantPresenceDeltaFrame {
                                at_ms: now_ms(),
                                sequence: room.presence_sequence,
                                joined: Vec::new(),
                                left: Vec::new(),
                                role_changes: vec![role_change_entry],
                            });
                        }
                    }
                    Err(err) => {
                        error = Some(err);
                    }
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::RoleGrant(grant.clone())).await?;
            if host_changed {
                let cfg = room_config_frame(state, room_id).await?;
                broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
            }
            if permissions_changed {
                broadcast_permissions_snapshots(state, room_id).await?;
            }
            if let Some(delta) = presence_delta {
                broadcast_frame(
                    state,
                    &room_id,
                    &KaigiFrame::ParticipantPresenceDelta(delta),
                )
                .await?;
            }
        }
        KaigiFrame::RoleRevoke(revoke) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext role revokes".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut host_changed = false;
            let mut permissions_changed = false;
            let mut presence_delta: Option<ParticipantPresenceDeltaFrame> = None;
            let mut error: Option<String> = None;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                match apply_role_revoke(room, conn_id, &revoke) {
                    Ok(outcome) => {
                        host_changed = outcome.host_changed;
                        permissions_changed = outcome.permissions_changed;
                        if let Some(role_change_entry) = outcome.role_change {
                            room.presence_sequence = room.presence_sequence.saturating_add(1);
                            presence_delta = Some(ParticipantPresenceDeltaFrame {
                                at_ms: now_ms(),
                                sequence: room.presence_sequence,
                                joined: Vec::new(),
                                left: Vec::new(),
                                role_changes: vec![role_change_entry],
                            });
                        }
                    }
                    Err(err) => {
                        error = Some(err);
                    }
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::RoleRevoke(revoke.clone())).await?;
            if host_changed {
                let cfg = room_config_frame(state, room_id).await?;
                broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
            }
            if permissions_changed {
                broadcast_permissions_snapshots(state, room_id).await?;
            }
            if let Some(delta) = presence_delta {
                broadcast_frame(
                    state,
                    &room_id,
                    &KaigiFrame::ParticipantPresenceDelta(delta),
                )
                .await?;
            }
        }
        KaigiFrame::SessionPolicy(update) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext policy updates".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            let mut permissions_changed = false;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                match apply_session_policy_update(room, conn_id, &update) {
                    Ok(()) => {
                        permissions_changed = true;
                    }
                    Err(err) => {
                        error = Some(err);
                    }
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let current_policy = session_policy_frame(state, room_id).await?;
            broadcast_frame(state, &room_id, &KaigiFrame::SessionPolicy(current_policy)).await?;
            if permissions_changed {
                broadcast_permissions_snapshots(state, room_id).await?;
            }
        }
        KaigiFrame::PermissionsSnapshot(_) => {
            send_error(
                state,
                room_id,
                conn_id,
                "permissions snapshots are hub-managed".to_string(),
            )
            .await?;
        }
        KaigiFrame::DeviceCapability(cap) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext device capabilities".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error = validate_device_capability_frame(&cap);
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                if let Some(sender) = room.participants.get(&conn_id) {
                    if !sender.state.hello_seen {
                        error.get_or_insert_with(|| "send Hello first".to_string());
                    } else if sender.state.participant_id != cap.participant_id {
                        error.get_or_insert_with(|| {
                            "participant_id must match sender participant_id".to_string()
                        });
                    }
                } else {
                    return Ok(());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            {
                let mut rooms = state.rooms.lock().await;
                if let Some(room) = rooms.get_mut(&room_id) {
                    room.device_caps_by_conn.insert(conn_id, cap.clone());
                }
            }
            broadcast_frame(state, &room_id, &KaigiFrame::DeviceCapability(cap)).await?;
        }
        KaigiFrame::MediaProfileNegotiation(profile) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext media profile negotiation".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error = validate_media_profile_negotiation_frame(&profile);
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                if let Some(sender) = room.participants.get(&conn_id) {
                    if !sender.state.hello_seen {
                        error.get_or_insert_with(|| "send Hello first".to_string());
                    } else if sender.state.participant_id != profile.participant_id {
                        error.get_or_insert_with(|| {
                            "participant_id must match sender participant_id".to_string()
                        });
                    }
                } else {
                    return Ok(());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let resolved_profile = {
                let rooms = state.rooms.lock().await;
                if let Some(room) = rooms.get(&room_id) {
                    resolve_media_profile_negotiation(room, conn_id, &profile)
                } else {
                    profile
                }
            };
            broadcast_frame(
                state,
                &room_id,
                &KaigiFrame::MediaProfileNegotiation(resolved_profile),
            )
            .await?;
        }
        KaigiFrame::RecordingNotice(notice) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext recording notices".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error = validate_recording_notice_frame(&notice);
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                if let Some(sender) = room.participants.get(&conn_id) {
                    if !sender.state.hello_seen {
                        error.get_or_insert_with(|| "send Hello first".to_string());
                    } else if sender.state.participant_id != notice.participant_id {
                        error.get_or_insert_with(|| {
                            "participant_id must match sender participant_id".to_string()
                        });
                    } else if sender.state.participant_id != notice.issued_by {
                        error.get_or_insert_with(|| {
                            "issued_by must match sender participant_id".to_string()
                        });
                    } else if matches!(notice.state, RecordingState::Started)
                        && !room.local_recording_allowed
                    {
                        error.get_or_insert_with(|| {
                            "local recording is disabled by room policy".to_string()
                        });
                    }
                } else {
                    return Ok(());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::RecordingNotice(notice)).await?;
        }
        KaigiFrame::E2EEKeyEpoch(key_epoch) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext e2ee key epochs".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error = validate_e2ee_key_epoch_frame(&key_epoch);
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                if let Some(sender) = room.participants.get(&conn_id) {
                    if !sender.state.hello_seen {
                        error.get_or_insert_with(|| "send Hello first".to_string());
                    } else if sender.state.participant_id != key_epoch.participant_id {
                        error.get_or_insert_with(|| {
                            "participant_id must match sender participant_id".to_string()
                        });
                    }
                } else {
                    return Ok(());
                }
                let current_epoch = room.e2ee_epochs.get(&conn_id).copied().unwrap_or(0);
                if key_epoch.epoch <= current_epoch {
                    error.get_or_insert_with(|| {
                        format!(
                            "e2ee key epoch must increase: current={} got={}",
                            current_epoch, key_epoch.epoch
                        )
                    });
                }
                if error.is_none() {
                    let signer_id = key_epoch.participant_id.clone();
                    if let Err(clock_err) = enforce_signed_action_clock(
                        room,
                        "e2ee_key_epoch",
                        &signer_id,
                        "sent_at_ms",
                        key_epoch.sent_at_ms,
                    ) {
                        error = Some(clock_err);
                    }
                }
                if error.is_none() {
                    room.e2ee_epochs.insert(conn_id, key_epoch.epoch);
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::E2EEKeyEpoch(key_epoch)).await?;
        }
        KaigiFrame::KeyRotationAck(ack) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode rejects plaintext key rotation acks".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                let sender_hello_seen = sender.state.hello_seen;
                let sender_participant_id = sender.state.participant_id.clone();
                if !sender_hello_seen {
                    error = Some("send Hello first".to_string());
                } else if sender_participant_id != ack.participant_id {
                    error = Some("participant_id must match sender participant_id".to_string());
                } else if ack.ack_epoch == 0 {
                    error = Some("ack_epoch must be >= 1".to_string());
                } else {
                    let current_epoch = room.e2ee_epochs.get(&conn_id).copied().unwrap_or(0);
                    if current_epoch == 0 || ack.ack_epoch > current_epoch {
                        error = Some(format!(
                            "ack_epoch exceeds sender key epoch: sender_epoch={} ack_epoch={}",
                            current_epoch, ack.ack_epoch
                        ));
                    } else {
                        let last_acked_epoch = room
                            .key_rotation_ack_epochs
                            .get(&conn_id)
                            .copied()
                            .unwrap_or(0);
                        if ack.ack_epoch <= last_acked_epoch {
                            error = Some(format!(
                                "ack_epoch must increase: last={} got={} (stale/replay rejected)",
                                last_acked_epoch, ack.ack_epoch
                            ));
                        }
                    }
                }
                if error.is_none()
                    && let Err(clock_err) = enforce_signed_action_clock(
                        room,
                        "key_rotation_ack",
                        &ack.participant_id,
                        "received_at_ms",
                        ack.received_at_ms,
                    )
                {
                    error = Some(clock_err);
                }
                if error.is_none() {
                    room.key_rotation_ack_epochs.insert(conn_id, ack.ack_epoch);
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            broadcast_frame(state, &room_id, &KaigiFrame::KeyRotationAck(ack)).await?;
        }
        KaigiFrame::Moderation(moderation) => {
            let audit = KaigiFrame::Moderation(moderation.clone());
            process_moderation_frame(state, room_id, conn_id, moderation, Some(audit)).await?;
        }
        KaigiFrame::ModerationSigned(signed) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode requires encrypted moderation payloads".to_string(),
                )
                .await?;
                return Ok(());
            }
            let mut error: Option<String> = None;
            {
                let rooms = state.rooms.lock().await;
                let Some(room) = rooms.get(&room_id) else {
                    return Ok(());
                };
                let Some(sender) = room.participants.get(&conn_id) else {
                    return Ok(());
                };
                if !sender.state.hello_seen {
                    error = Some("send Hello first".to_string());
                } else if !can_moderate_conn(room, conn_id) {
                    error = Some("host/co-host only".to_string());
                } else if sender.state.participant_id != signed.issued_by {
                    error = Some("issued_by must match sender participant_id".to_string());
                } else if !is_valid_hex_len(&signed.signature_hex, 32) {
                    error = Some("signature_hex must be 32-byte hex".to_string());
                } else if !moderation_signed_signature_is_valid(&signed) {
                    error = Some("signature_hex failed moderation_signed verification".to_string());
                }
            }
            if let Some(message) = error {
                send_error(state, room_id, conn_id, message).await?;
                return Ok(());
            }
            let moderation = ModerationFrame {
                sent_at_ms: signed.sent_at_ms,
                target: signed.target.clone(),
                action: signed.action.clone(),
            };
            process_moderation_frame(
                state,
                room_id,
                conn_id,
                moderation,
                Some(KaigiFrame::ModerationSigned(signed)),
            )
            .await?;
        }
        KaigiFrame::RoomConfig(_) => {}
        KaigiFrame::ParticipantPresenceDelta(_) => {
            send_error(
                state,
                room_id,
                conn_id,
                "presence deltas are hub-managed".to_string(),
            )
            .await?;
        }
        KaigiFrame::Payment(payment) => {
            if room_is_anonymous(state, room_id).await {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "anonymous mode uses escrow proofs instead of payment frames".to_string(),
                )
                .await?;
                return Ok(());
            }
            let now = now_ms();
            let mut require_hello = false;
            let mut invalid_payment_hash = false;
            let (paid_nano_xor, billed_nano_xor) = {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                let Some(participant) = room.participants.get_mut(&conn_id) else {
                    return Ok(());
                };
                if !participant.state.hello_seen {
                    require_hello = true;
                    participant.state.last_billed_at_ms = now;
                    (
                        participant.state.paid_nano_xor,
                        participant.state.billed_nano_xor,
                    )
                } else {
                    let rate = room.rate_per_minute_nano;
                    advance_billing(&mut participant.state, rate, now);
                    if state.require_payment_tx_hash
                        && rate > 0
                        && !payment
                            .tx_hash_hex
                            .as_deref()
                            .is_some_and(is_valid_tx_hash_hex)
                    {
                        invalid_payment_hash = true;
                        (
                            participant.state.paid_nano_xor,
                            participant.state.billed_nano_xor,
                        )
                    } else {
                        participant.state.paid_nano_xor = participant
                            .state
                            .paid_nano_xor
                            .saturating_add(payment.amount_nano_xor);
                        participant.state.last_payment_at_ms = Some(now);
                        (
                            participant.state.paid_nano_xor,
                            participant.state.billed_nano_xor,
                        )
                    }
                }
            };

            if require_hello {
                send_error(state, room_id, conn_id, "send Hello first".to_string()).await?;
                return Ok(());
            }

            if invalid_payment_hash {
                send_error(
                    state,
                    room_id,
                    conn_id,
                    "payment tx_hash_hex is required and must be 32-byte hex when room rate is non-zero".to_string(),
                )
                .await?;
                return Ok(());
            }

            let ack = KaigiFrame::PaymentAck(PaymentAckFrame {
                received_at_ms: now,
                amount_nano_xor: payment.amount_nano_xor,
                total_paid_nano_xor: paid_nano_xor,
                total_billed_nano_xor: billed_nano_xor,
            });
            send_frame_to(state, room_id, conn_id, &ack).await?;
        }
        KaigiFrame::PaymentAck(_) => {}
        KaigiFrame::Ping(ping) => {
            let pong = KaigiFrame::Pong(kaigi_wire::PongFrame { nonce: ping.nonce });
            send_frame_to(state, room_id, conn_id, &pong).await?;
        }
        KaigiFrame::Pong(_) => {}
        KaigiFrame::Roster(_) => {}
        KaigiFrame::Event(_) => {}
        KaigiFrame::Error(err) => {
            warn!(peer = %peer, room = %hex::encode(room_id), message = %err.message, "client error frame");
        }
    }
    Ok(())
}

async fn participant_identity(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
) -> (String, Option<String>) {
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return ("unknown".to_string(), None);
    };
    let Some(participant) = room.participants.get(&conn_id) else {
        return ("unknown".to_string(), None);
    };
    (
        participant.state.participant_id.clone(),
        participant.state.display_name.clone(),
    )
}

async fn hello_seen(state: &HubState, room_id: RoomId, conn_id: ConnId) -> bool {
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return false;
    };
    let Some(participant) = room.participants.get(&conn_id) else {
        return false;
    };
    participant.state.hello_seen
}

async fn room_is_anonymous(state: &HubState, room_id: RoomId) -> bool {
    let rooms = state.rooms.lock().await;
    rooms
        .get(&room_id)
        .map(|room| room.anonymous_mode)
        .unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
struct RoleMutationOutcome {
    host_changed: bool,
    permissions_changed: bool,
    role_change: Option<RoleChangeEntry>,
}

#[derive(Clone, Debug)]
struct WaitingRoomAdmitOutcome {
    conn_id: ConnId,
    snapshot: ParticipantSnapshot,
    presence_delta: ParticipantPresenceDeltaFrame,
}

async fn process_moderation_frame(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
    moderation: ModerationFrame,
    audit_frame: Option<KaigiFrame>,
) -> Result<()> {
    if room_is_anonymous(state, room_id).await {
        send_error(
            state,
            room_id,
            conn_id,
            "anonymous mode requires encrypted moderation payloads".to_string(),
        )
        .await?;
        return Ok(());
    }
    let now = now_ms();
    let action = moderation.action.clone();
    let target = moderation.target.clone();

    let mut targets: Vec<(ConnId, mpsc::Sender<Message>)> = Vec::new();
    let mut snapshots: Vec<ParticipantSnapshot> = Vec::new();
    let mut close_senders: Vec<mpsc::Sender<Message>> = Vec::new();
    let mut admitted_outcomes: Vec<WaitingRoomAdmitOutcome> = Vec::new();
    let mut denied_participants: Vec<(mpsc::Sender<Message>, String)> = Vec::new();
    let mut moderation_error: Option<String> = None;

    {
        let mut rooms = state.rooms.lock().await;
        let Some(room) = rooms.get_mut(&room_id) else {
            return Ok(());
        };
        let Some(sender) = room.participants.get(&conn_id) else {
            return Ok(());
        };
        if !sender.state.hello_seen {
            drop(rooms);
            send_error(state, room_id, conn_id, "send Hello first".to_string()).await?;
            return Ok(());
        }
        if !can_moderate_conn(room, conn_id) {
            drop(rooms);
            send_error(state, room_id, conn_id, "host/co-host only".to_string()).await?;
            return Ok(());
        }
        if let Err(err) = validate_moderation_sender_clock(room, conn_id, &moderation) {
            drop(rooms);
            send_error(state, room_id, conn_id, err).await?;
            return Ok(());
        }

        match &target {
            ModerationTarget::All => {
                if matches!(
                    action,
                    ModerationAction::AdmitFromWaiting | ModerationAction::DenyFromWaiting
                ) {
                    drop(rooms);
                    send_error(
                        state,
                        room_id,
                        conn_id,
                        "waiting room admit/deny requires /admit <participant_id> or /deny <participant_id>"
                            .to_string(),
                    )
                    .await?;
                    return Ok(());
                }
                for (id, participant) in &room.participants {
                    if *id == conn_id && !include_sender_in_all_target(&action) {
                        continue;
                    }
                    targets.push((*id, participant.tx.clone()));
                }
            }
            ModerationTarget::Participant(target_id) => {
                if let Some((id, participant)) = room
                    .participants
                    .iter()
                    .find(|(_, p)| p.state.participant_id == *target_id)
                {
                    targets.push((*id, participant.tx.clone()));
                } else {
                    drop(rooms);
                    send_error(
                        state,
                        room_id,
                        conn_id,
                        format!("unknown participant_id: {}", target_id),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        for (target_conn_id, target_tx) in targets.iter() {
            match &action {
                ModerationAction::Kick => {
                    close_senders.push(target_tx.clone());
                }
                ModerationAction::DisableMic => {
                    let Some(target) = room.participants.get_mut(target_conn_id) else {
                        continue;
                    };
                    target.state.mic_enabled = false;
                    snapshots.push(participant_snapshot_from_state(&target.state, now));
                }
                ModerationAction::DisableVideo => {
                    let Some(target) = room.participants.get_mut(target_conn_id) else {
                        continue;
                    };
                    target.state.video_enabled = false;
                    snapshots.push(participant_snapshot_from_state(&target.state, now));
                }
                ModerationAction::DisableScreenShare => {
                    let Some(target) = room.participants.get_mut(target_conn_id) else {
                        continue;
                    };
                    target.state.screen_share_enabled = false;
                    snapshots.push(participant_snapshot_from_state(&target.state, now));
                }
                ModerationAction::AdmitFromWaiting => {
                    match admit_waiting_participant(room, *target_conn_id) {
                        Ok(outcome) => admitted_outcomes.push(outcome),
                        Err(err) => {
                            moderation_error.get_or_insert(err);
                        }
                    }
                }
                ModerationAction::DenyFromWaiting => {
                    match deny_waiting_participant(room, *target_conn_id) {
                        Ok(denied_id) => denied_participants.push((target_tx.clone(), denied_id)),
                        Err(err) => {
                            moderation_error.get_or_insert(err);
                        }
                    }
                }
            }
        }
    }

    if let Some(err) = moderation_error {
        send_error(state, room_id, conn_id, err).await?;
        return Ok(());
    }

    if let Some(audit_frame) = audit_frame {
        broadcast_frame(state, &room_id, &audit_frame).await?;
    }

    for snap in snapshots {
        broadcast_frame(
            state,
            &room_id,
            &KaigiFrame::Event(RoomEventFrame::StateUpdated(snap)),
        )
        .await?;
    }
    for outcome in &admitted_outcomes {
        broadcast_frame(
            state,
            &room_id,
            &KaigiFrame::Event(RoomEventFrame::Joined(outcome.snapshot.clone())),
        )
        .await?;
    }
    for outcome in &admitted_outcomes {
        broadcast_frame(
            state,
            &room_id,
            &KaigiFrame::ParticipantPresenceDelta(outcome.presence_delta.clone()),
        )
        .await?;
    }
    for outcome in &admitted_outcomes {
        let roster = roster_frame(state, room_id).await?;
        send_frame_to(state, room_id, outcome.conn_id, &KaigiFrame::Roster(roster)).await?;
        let cfg = room_config_frame(state, room_id).await?;
        send_frame_to(
            state,
            room_id,
            outcome.conn_id,
            &KaigiFrame::RoomConfig(cfg),
        )
        .await?;
        let policy = session_policy_frame(state, room_id).await?;
        send_frame_to(
            state,
            room_id,
            outcome.conn_id,
            &KaigiFrame::SessionPolicy(policy),
        )
        .await?;
    }
    if matches!(action, ModerationAction::AdmitFromWaiting) {
        broadcast_permissions_snapshots(state, room_id).await?;
    }

    if matches!(action, ModerationAction::Kick) {
        let reason = match target {
            ModerationTarget::All => "meeting ended by host".to_string(),
            ModerationTarget::Participant(ref p) => format!("removed by host: {p}"),
        };
        let frame = KaigiFrame::Error(ErrorFrame { message: reason });
        let Ok(bytes) = encode_framed(&frame) else {
            return Ok(());
        };
        let bytes = tokio_tungstenite::tungstenite::Bytes::from(bytes);
        for tx in close_senders {
            let _ = tx.try_send(Message::Binary(bytes.clone()));
            let _ = tx.try_send(Message::Close(None));
        }
    }
    if matches!(action, ModerationAction::DenyFromWaiting) {
        for (tx, denied_id) in denied_participants {
            let frame = KaigiFrame::Error(ErrorFrame {
                message: format!("waiting room admission denied: {denied_id}"),
            });
            if let Ok(bytes) = encode_framed(&frame) {
                let _ = tx.try_send(Message::Binary(bytes.into()));
            }
            let _ = tx.try_send(Message::Close(None));
        }
    }

    Ok(())
}

fn role_kind_for_conn(room: &RoomState, conn_id: ConnId) -> RoleKind {
    if room.host_conn_id == Some(conn_id) {
        RoleKind::Host
    } else if room.co_host_conn_ids.contains(&conn_id) {
        RoleKind::CoHost
    } else {
        RoleKind::Participant
    }
}

fn can_moderate_conn(room: &RoomState, conn_id: ConnId) -> bool {
    matches!(
        role_kind_for_conn(room, conn_id),
        RoleKind::Host | RoleKind::CoHost
    )
}

fn can_manage_roles_and_policy(room: &RoomState, conn_id: ConnId) -> bool {
    room.host_conn_id == Some(conn_id)
}

fn sync_active_co_host_bindings(room: &mut RoomState) -> bool {
    let before = room.co_host_conn_ids.clone();

    room.co_host_conn_ids.retain(|conn_id| {
        room.host_conn_id != Some(*conn_id)
            && room.participants.get(conn_id).is_some_and(|participant| {
                participant.state.hello_seen
                    && room
                        .co_host_role_owners
                        .contains(participant.state.participant_id.as_str())
            })
    });

    for (conn_id, participant) in &room.participants {
        if !participant.state.hello_seen || room.host_conn_id == Some(*conn_id) {
            continue;
        }
        if room
            .co_host_role_owners
            .contains(participant.state.participant_id.as_str())
        {
            room.co_host_conn_ids.insert(*conn_id);
        }
    }

    room.co_host_conn_ids != before
}

fn restore_reserved_roles_on_join(
    room: &mut RoomState,
    conn_id: ConnId,
    participant_id: &str,
) -> bool {
    let mut changed = false;
    if room.host_role_owner.as_deref() == Some(participant_id) && room.host_conn_id != Some(conn_id)
    {
        room.host_conn_id = Some(conn_id);
        changed = true;
    }
    if room.host_conn_id != Some(conn_id)
        && room.co_host_role_owners.contains(participant_id)
        && room.co_host_conn_ids.insert(conn_id)
    {
        changed = true;
    }
    if sync_active_co_host_bindings(room) {
        changed = true;
    }
    changed
}

async fn validate_plaintext_e2ee_gate(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
) -> Option<String> {
    let rooms = state.rooms.lock().await;
    let room = rooms.get(&room_id)?;
    let sender = room.participants.get(&conn_id)?;
    if !sender.state.hello_seen || room.anonymous_mode || !room.e2ee_required {
        return None;
    }
    let sender_epoch = room.e2ee_epochs.get(&conn_id).copied().unwrap_or(0);
    if sender_epoch == 0 {
        return Some("e2ee required: publish E2EEKeyEpoch before plaintext control".to_string());
    }
    None
}

fn resolve_media_profile_negotiation(
    room: &RoomState,
    sender_conn_id: ConnId,
    profile: &MediaProfileNegotiationFrame,
) -> MediaProfileNegotiationFrame {
    let mut resolved = profile.clone();
    if !matches!(resolved.negotiated_profile, MediaProfileKind::Hdr) {
        return resolved;
    }

    let sender_hdr_capture = room
        .device_caps_by_conn
        .get(&sender_conn_id)
        .is_some_and(|cap| cap.hdr_capture);
    let any_remote_hdr_render = room.participants.iter().any(|(conn_id, participant)| {
        *conn_id != sender_conn_id
            && participant.state.hello_seen
            && room
                .device_caps_by_conn
                .get(conn_id)
                .is_some_and(|cap| cap.hdr_render)
    });
    if !sender_hdr_capture || !any_remote_hdr_render {
        resolved.negotiated_profile = MediaProfileKind::Sdr;
    }
    resolved
}

fn participant_id_is_guest(participant_id: &str) -> bool {
    // Heuristic for dev harness:
    // Account-like ids are `name@domain`; ids without `@` are treated as guests.
    !participant_id.contains('@')
}

fn participant_id_has_reserved_moderation_role(room: &RoomState, participant_id: &str) -> bool {
    room.host_role_owner.as_deref() == Some(participant_id)
        || room.co_host_role_owners.contains(participant_id)
}

fn conn_or_participant_can_moderate(
    room: &RoomState,
    conn_id: ConnId,
    participant_id: &str,
) -> bool {
    room.host_conn_id == Some(conn_id)
        || room.co_host_conn_ids.contains(&conn_id)
        || participant_id_has_reserved_moderation_role(room, participant_id)
}

fn validate_join_allowed(
    room: &RoomState,
    conn_id: ConnId,
    participant_id: &str,
) -> Option<String> {
    if room.participants.get(&conn_id).is_some_and(|participant| {
        participant.state.hello_seen && participant.state.participant_id != participant_id
    }) {
        return Some("participant_id cannot change after Hello".to_string());
    }
    if room.participants.iter().any(|(id, participant)| {
        *id != conn_id
            && (participant.state.hello_seen || participant.state.waiting_room_pending)
            && participant.state.participant_id == participant_id
    }) {
        return Some(format!("participant_id already in use: {participant_id}"));
    }

    let can_moderate = conn_or_participant_can_moderate(room, conn_id, participant_id);
    if room.room_lock && room.host_conn_id.is_some() && !can_moderate {
        return Some("room is locked".to_string());
    }
    let already_joined = room
        .participants
        .get(&conn_id)
        .is_some_and(|participant| participant.state.hello_seen);
    if !already_joined
        && !room.guest_join_allowed
        && participant_id_is_guest(participant_id)
        && room.host_conn_id.is_some()
        && !can_moderate
    {
        return Some("guest participants are not allowed by room policy".to_string());
    }
    if !already_joined {
        let active_joined = room
            .participants
            .values()
            .filter(|participant| participant.state.hello_seen)
            .count();
        if active_joined >= room.max_participants as usize {
            return Some(format!(
                "room is full: max_participants={}",
                room.max_participants
            ));
        }
    }
    None
}

fn admit_waiting_participant(
    room: &mut RoomState,
    target_conn_id: ConnId,
) -> std::result::Result<WaitingRoomAdmitOutcome, String> {
    let joined_state = {
        let Some(target) = room.participants.get_mut(&target_conn_id) else {
            return Err("participant missing".to_string());
        };
        if !target.state.waiting_room_pending {
            return Err(format!(
                "participant_id is not waiting: {}",
                target.state.participant_id
            ));
        }

        target.state.waiting_room_pending = false;
        target.state.hello_seen = true;
        target.state.mic_enabled = false;
        target.state.video_enabled = false;
        target.state.screen_share_enabled = false;
        target.state.clone()
    };
    room.e2ee_epochs.entry(target_conn_id).or_insert(0);
    if restore_reserved_roles_on_join(room, target_conn_id, &joined_state.participant_id) {
        room.permissions_epoch = room.permissions_epoch.saturating_add(1);
    }
    room.presence_sequence = room.presence_sequence.saturating_add(1);

    let at_ms = now_ms();
    let snapshot = participant_snapshot_from_state(&joined_state, at_ms);
    Ok(WaitingRoomAdmitOutcome {
        conn_id: target_conn_id,
        snapshot: snapshot.clone(),
        presence_delta: ParticipantPresenceDeltaFrame {
            at_ms,
            sequence: room.presence_sequence,
            joined: vec![snapshot],
            left: Vec::new(),
            role_changes: Vec::new(),
        },
    })
}

fn deny_waiting_participant(
    room: &mut RoomState,
    target_conn_id: ConnId,
) -> std::result::Result<String, String> {
    let Some(target) = room.participants.get(&target_conn_id) else {
        return Err("participant missing".to_string());
    };
    if !target.state.waiting_room_pending {
        return Err(format!(
            "participant_id is not waiting: {}",
            target.state.participant_id
        ));
    }
    let denied_participant_id = target.state.participant_id.clone();

    room.participants.remove(&target_conn_id);
    room.co_host_conn_ids.remove(&target_conn_id);
    room.e2ee_epochs.remove(&target_conn_id);
    room.key_rotation_ack_epochs.remove(&target_conn_id);
    if room.host_conn_id == Some(target_conn_id) {
        room.host_conn_id = room.participants.keys().min().copied();
        sync_active_co_host_bindings(room);
    }

    Ok(denied_participant_id)
}

fn apply_role_grant(
    room: &mut RoomState,
    sender_conn_id: ConnId,
    grant: &RoleGrantFrame,
) -> std::result::Result<RoleMutationOutcome, String> {
    let sender = room
        .participants
        .get(&sender_conn_id)
        .ok_or_else(|| "participant missing".to_string())?;
    if !sender.state.hello_seen {
        return Err("send Hello first".to_string());
    }
    if !can_manage_roles_and_policy(room, sender_conn_id) {
        return Err("host only".to_string());
    }
    if sender.state.participant_id != grant.granted_by {
        return Err("granted_by must match sender participant_id".to_string());
    }
    if !is_valid_hex_len(&grant.signature_hex, 32) {
        return Err("signature_hex must be 32-byte hex".to_string());
    }
    if !role_grant_signature_is_valid(grant) {
        return Err("signature_hex failed role_grant verification".to_string());
    }

    let target_conn_id = room
        .participants
        .iter()
        .find(|(_, participant)| {
            participant.state.hello_seen
                && participant.state.participant_id == grant.target_participant_id
        })
        .map(|(id, _)| *id)
        .ok_or_else(|| format!("unknown participant_id: {}", grant.target_participant_id))?;

    if matches!(grant.role, RoleKind::Participant) {
        return Err("role grant supports Host or CoHost only".to_string());
    }
    if matches!(grant.role, RoleKind::CoHost) && room.host_conn_id == Some(target_conn_id) {
        return Err("host already has full permissions".to_string());
    }
    enforce_signed_action_clock(
        room,
        "role_grant",
        &grant.granted_by,
        "issued_at_ms",
        grant.issued_at_ms,
    )?;

    let mut outcome = RoleMutationOutcome::default();
    match grant.role {
        RoleKind::Host => {
            let mut permissions_changed = false;
            if room.host_role_owner.as_deref() != Some(grant.target_participant_id.as_str()) {
                room.host_role_owner = Some(grant.target_participant_id.clone());
                permissions_changed = true;
            }
            if room.host_conn_id != Some(target_conn_id) {
                room.host_conn_id = Some(target_conn_id);
                outcome.host_changed = true;
                outcome.role_change = Some(RoleChangeEntry {
                    participant_id: grant.target_participant_id.clone(),
                    role: RoleKind::Host,
                    granted: true,
                });
                permissions_changed = true;
            }
            if sync_active_co_host_bindings(room) {
                permissions_changed = true;
            }
            if permissions_changed {
                room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                outcome.permissions_changed = true;
            }
        }
        RoleKind::CoHost => {
            let mut permissions_changed = room
                .co_host_role_owners
                .insert(grant.target_participant_id.clone());
            if room.host_conn_id != Some(target_conn_id)
                && room.co_host_conn_ids.insert(target_conn_id)
            {
                permissions_changed = true;
            }
            if sync_active_co_host_bindings(room) {
                permissions_changed = true;
            }
            if permissions_changed {
                room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                outcome.permissions_changed = true;
                outcome.role_change = Some(RoleChangeEntry {
                    participant_id: grant.target_participant_id.clone(),
                    role: RoleKind::CoHost,
                    granted: true,
                });
            }
        }
        RoleKind::Participant => unreachable!("validated above"),
    }
    Ok(outcome)
}

fn apply_role_revoke(
    room: &mut RoomState,
    sender_conn_id: ConnId,
    revoke: &RoleRevokeFrame,
) -> std::result::Result<RoleMutationOutcome, String> {
    let sender = room
        .participants
        .get(&sender_conn_id)
        .ok_or_else(|| "participant missing".to_string())?;
    if !sender.state.hello_seen {
        return Err("send Hello first".to_string());
    }
    if !can_manage_roles_and_policy(room, sender_conn_id) {
        return Err("host only".to_string());
    }
    if sender.state.participant_id != revoke.revoked_by {
        return Err("revoked_by must match sender participant_id".to_string());
    }
    if !is_valid_hex_len(&revoke.signature_hex, 32) {
        return Err("signature_hex must be 32-byte hex".to_string());
    }
    if !role_revoke_signature_is_valid(revoke) {
        return Err("signature_hex failed role_revoke verification".to_string());
    }

    let target_conn_id = room
        .participants
        .iter()
        .find(|(_, participant)| {
            participant.state.hello_seen
                && participant.state.participant_id == revoke.target_participant_id
        })
        .map(|(id, _)| *id)
        .ok_or_else(|| format!("unknown participant_id: {}", revoke.target_participant_id))?;

    if !matches!(revoke.role, RoleKind::CoHost) {
        return Err("role revoke supports CoHost only".to_string());
    }
    enforce_signed_action_clock(
        room,
        "role_revoke",
        &revoke.revoked_by,
        "issued_at_ms",
        revoke.issued_at_ms,
    )?;

    let mut outcome = RoleMutationOutcome::default();
    match revoke.role {
        RoleKind::Host => {
            return Err("host role cannot be revoked directly; transfer host instead".to_string());
        }
        RoleKind::CoHost => {
            let mut permissions_changed = room
                .co_host_role_owners
                .remove(revoke.target_participant_id.as_str());
            if room.co_host_conn_ids.remove(&target_conn_id) {
                permissions_changed = true;
            }
            if sync_active_co_host_bindings(room) {
                permissions_changed = true;
            }
            if permissions_changed {
                room.permissions_epoch = room.permissions_epoch.saturating_add(1);
                outcome.permissions_changed = true;
                outcome.role_change = Some(RoleChangeEntry {
                    participant_id: revoke.target_participant_id.clone(),
                    role: RoleKind::CoHost,
                    granted: false,
                });
            }
        }
        RoleKind::Participant => unreachable!("validated above"),
    }
    Ok(outcome)
}

fn apply_session_policy_update(
    room: &mut RoomState,
    sender_conn_id: ConnId,
    update: &SessionPolicyFrame,
) -> std::result::Result<(), String> {
    let sender = room
        .participants
        .get(&sender_conn_id)
        .ok_or_else(|| "participant missing".to_string())?;
    if !sender.state.hello_seen {
        return Err("send Hello first".to_string());
    }
    if !can_manage_roles_and_policy(room, sender_conn_id) {
        return Err("host only".to_string());
    }
    if sender.state.participant_id != update.updated_by {
        return Err("updated_by must match sender participant_id".to_string());
    }
    if !is_valid_hex_len(&update.signature_hex, 32) {
        return Err("signature_hex must be 32-byte hex".to_string());
    }
    if !session_policy_signature_is_valid(update) {
        return Err("signature_hex failed session_policy verification".to_string());
    }
    if update.policy_epoch <= room.policy_epoch {
        return Err(format!(
            "policy_epoch must increase: current={} got={}",
            room.policy_epoch, update.policy_epoch
        ));
    }
    if update.max_participants == 0 {
        return Err("max_participants must be >= 1".to_string());
    }
    enforce_signed_action_clock(
        room,
        "session_policy",
        &update.updated_by,
        "updated_at_ms",
        update.updated_at_ms,
    )?;

    room.room_lock = update.room_lock;
    room.waiting_room_enabled = update.waiting_room_enabled;
    room.guest_join_allowed = update.guest_join_allowed;
    room.local_recording_allowed = update.local_recording_allowed;
    room.e2ee_required = update.e2ee_required;
    room.max_participants = update.max_participants;
    room.policy_epoch = update.policy_epoch;
    room.policy_updated_at_ms = update.updated_at_ms;
    room.policy_updated_by = update.updated_by.clone();
    room.policy_signature_hex = update.signature_hex.clone();
    room.permissions_epoch = room.permissions_epoch.saturating_add(1);
    Ok(())
}

fn validate_moderation_sender_clock(
    room: &mut RoomState,
    sender_conn_id: ConnId,
    moderation: &ModerationFrame,
) -> std::result::Result<(), String> {
    let signer_id = room
        .participants
        .get(&sender_conn_id)
        .ok_or_else(|| "participant missing".to_string())?
        .state
        .participant_id
        .clone();
    enforce_signed_action_clock(
        room,
        "moderation",
        &signer_id,
        "sent_at_ms",
        moderation.sent_at_ms,
    )
}

fn permissions_snapshot_for_conn(
    room: &RoomState,
    conn_id: ConnId,
    at_ms: u64,
) -> Option<PermissionsSnapshotFrame> {
    let participant = room.participants.get(&conn_id)?;
    if !participant.state.hello_seen {
        return None;
    }
    let role = role_kind_for_conn(room, conn_id);
    let is_host = matches!(role, RoleKind::Host);
    let is_co_host = matches!(role, RoleKind::CoHost);
    Some(PermissionsSnapshotFrame {
        at_ms,
        participant_id: participant.state.participant_id.clone(),
        host: is_host,
        co_host: is_co_host,
        can_moderate: can_moderate_conn(room, conn_id),
        can_record_local: room.local_recording_allowed,
        epoch: room.permissions_epoch,
    })
}

fn participant_snapshot_from_state(state: &ParticipantState, at_ms: u64) -> ParticipantSnapshot {
    ParticipantSnapshot {
        at_ms,
        participant_id: state.participant_id.clone(),
        display_name: state.display_name.clone(),
        mic_enabled: state.mic_enabled,
        video_enabled: state.video_enabled,
        screen_share_enabled: state.screen_share_enabled,
    }
}

fn anon_roster_frame_locked(room: &RoomState) -> AnonRosterFrame {
    let mut participants = room
        .participants
        .values()
        .filter_map(|participant| {
            let handle = participant.state.participant_handle.as_ref()?;
            let pubkey = participant.state.x25519_pubkey_hex.as_ref()?;
            Some(AnonRosterEntry {
                participant_handle: handle.clone(),
                x25519_pubkey_hex: pubkey.clone(),
                joined_at_ms: participant.state.last_billed_at_ms,
            })
        })
        .collect::<Vec<_>>();
    participants.sort_by(|a, b| a.participant_handle.cmp(&b.participant_handle));
    AnonRosterFrame {
        at_ms: now_ms(),
        participants,
    }
}

fn build_anon_group_key_rotate_frame(room: &mut RoomState, at_ms: u64) -> AnonGroupKeyRotateFrame {
    room.permissions_epoch = room.permissions_epoch.saturating_add(1);
    let epoch = room.permissions_epoch.max(1);
    let mut member_handles = room
        .participants
        .values()
        .filter_map(|participant| {
            if participant.state.hello_seen && participant.state.anonymous_mode {
                participant.state.participant_handle.clone()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    member_handles.sort();
    let entropy = member_handles.join("|");
    let key_wrap_hex = deterministic_signature_hex(
        "anon_group_key_rotate",
        &format!("{at_ms}|{epoch}|{entropy}"),
    );
    AnonGroupKeyRotateFrame {
        sent_at_ms: at_ms,
        epoch,
        key_wrap_hex,
        member_handles,
    }
}

async fn anon_roster_frame(state: &HubState, room_id: RoomId) -> Result<AnonRosterFrame> {
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return Ok(AnonRosterFrame {
            at_ms: now_ms(),
            participants: Vec::new(),
        });
    };
    Ok(anon_roster_frame_locked(room))
}

async fn roster_frame(state: &HubState, room_id: RoomId) -> Result<RosterFrame> {
    let at_ms = now_ms();
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return Ok(RosterFrame {
            at_ms,
            participants: Vec::new(),
        });
    };
    let mut participants = Vec::with_capacity(room.participants.len());
    for participant in room.participants.values() {
        if !participant.state.hello_seen {
            continue;
        }
        participants.push(RosterEntry {
            participant_id: participant.state.participant_id.clone(),
            display_name: participant.state.display_name.clone(),
            mic_enabled: participant.state.mic_enabled,
            video_enabled: participant.state.video_enabled,
            screen_share_enabled: participant.state.screen_share_enabled,
        });
    }
    participants.sort_by(|a, b| a.participant_id.cmp(&b.participant_id));
    Ok(RosterFrame {
        at_ms,
        participants,
    })
}

async fn room_config_frame(state: &HubState, room_id: RoomId) -> Result<RoomConfigFrame> {
    let updated_at_ms = now_ms();
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return Ok(RoomConfigFrame {
            updated_at_ms,
            host_participant_id: None,
            rate_per_minute_nano: state.billing.default_rate_per_minute_nano,
            billing_grace_secs: state.billing.grace_secs,
            max_screen_shares: 1,
        });
    };

    let host_participant_id = if room.anonymous_mode {
        None
    } else {
        room.host_conn_id.and_then(|host_id| {
            room.participants
                .get(&host_id)
                .map(|p| p.state.participant_id.clone())
        })
    };

    Ok(RoomConfigFrame {
        updated_at_ms,
        host_participant_id,
        rate_per_minute_nano: room.rate_per_minute_nano,
        billing_grace_secs: state.billing.grace_secs,
        max_screen_shares: room.max_screen_shares.max(1),
    })
}

async fn session_policy_frame(state: &HubState, room_id: RoomId) -> Result<SessionPolicyFrame> {
    let rooms = state.rooms.lock().await;
    let Some(room) = rooms.get(&room_id) else {
        return Ok(SessionPolicyFrame {
            updated_at_ms: 0,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: DEFAULT_MAX_PARTICIPANTS,
            policy_epoch: 0,
            updated_by: SYSTEM_POLICY_UPDATED_BY.to_string(),
            signature_hex: String::new(),
        });
    };
    Ok(SessionPolicyFrame {
        updated_at_ms: room.policy_updated_at_ms,
        room_lock: room.room_lock,
        waiting_room_enabled: room.waiting_room_enabled,
        guest_join_allowed: room.guest_join_allowed,
        local_recording_allowed: room.local_recording_allowed,
        e2ee_required: room.e2ee_required,
        max_participants: room.max_participants,
        policy_epoch: room.policy_epoch,
        updated_by: room.policy_updated_by.clone(),
        signature_hex: room.policy_signature_hex.clone(),
    })
}

async fn send_frame_to(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
    frame: &KaigiFrame,
) -> Result<()> {
    let msg = Message::Binary(encode_framed(frame)?.into());
    let tx = {
        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).ok_or_else(|| anyhow!("room missing"))?;
        room.participants
            .get(&conn_id)
            .ok_or_else(|| anyhow!("participant missing"))?
            .tx
            .clone()
    };
    let _ = tx.try_send(msg);
    Ok(())
}

async fn send_error(
    state: &HubState,
    room_id: RoomId,
    conn_id: ConnId,
    message: String,
) -> Result<()> {
    send_frame_to(
        state,
        room_id,
        conn_id,
        &KaigiFrame::Error(ErrorFrame { message }),
    )
    .await
}

async fn broadcast_frame(state: &HubState, room_id: &RoomId, frame: &KaigiFrame) -> Result<()> {
    let bytes = tokio_tungstenite::tungstenite::Bytes::from(encode_framed(frame)?);
    let senders = {
        let rooms = state.rooms.lock().await;
        let Some(room) = rooms.get(room_id) else {
            return Ok(());
        };
        room.participants
            .values()
            .map(|p| p.tx.clone())
            .collect::<Vec<_>>()
    };

    let mut dropped = 0usize;
    for tx in senders {
        if tx.try_send(Message::Binary(bytes.clone())).is_err() {
            dropped = dropped.saturating_add(1);
        }
    }
    let mut backpressure_notice = None;
    if dropped > 0 {
        let mut rooms = state.rooms.lock().await;
        if let Some(room) = rooms.get_mut(room_id) {
            room.backpressure_dropped_messages = room
                .backpressure_dropped_messages
                .saturating_add(dropped as u64);
            let now = now_ms();
            if now.saturating_sub(room.backpressure_last_notice_at_ms)
                >= BACKPRESSURE_NOTICE_MIN_INTERVAL_MS
            {
                room.backpressure_last_notice_at_ms = now;
                backpressure_notice = Some(format!(
                    "backpressure: dropped fanout to {dropped} participant(s); total_dropped={}",
                    room.backpressure_dropped_messages
                ));
            }
        }
    }
    if let Some(notice) = backpressure_notice {
        notify_moderators(state, *room_id, &notice).await?;
    }

    Ok(())
}

async fn notify_moderators(state: &HubState, room_id: RoomId, message: &str) -> Result<()> {
    let targets = {
        let rooms = state.rooms.lock().await;
        let Some(room) = rooms.get(&room_id) else {
            return Ok(());
        };
        let mut targets = Vec::new();
        if let Some(host_conn_id) = room.host_conn_id {
            targets.push(host_conn_id);
        }
        for conn_id in &room.co_host_conn_ids {
            targets.push(*conn_id);
        }
        targets
    };

    for conn_id in targets {
        send_frame_to(
            state,
            room_id,
            conn_id,
            &KaigiFrame::Error(ErrorFrame {
                message: message.to_string(),
            }),
        )
        .await?;
    }

    Ok(())
}

async fn broadcast_permissions_snapshots(state: &HubState, room_id: RoomId) -> Result<()> {
    let at_ms = now_ms();
    let snapshots = {
        let rooms = state.rooms.lock().await;
        let Some(room) = rooms.get(&room_id) else {
            return Ok(());
        };
        if room.anonymous_mode {
            return Ok(());
        }
        room.participants
            .keys()
            .copied()
            .filter_map(|conn_id| {
                permissions_snapshot_for_conn(room, conn_id, at_ms)
                    .map(|snapshot| (conn_id, KaigiFrame::PermissionsSnapshot(snapshot)))
            })
            .collect::<Vec<_>>()
    };

    for (conn_id, frame) in snapshots {
        send_frame_to(state, room_id, conn_id, &frame).await?;
    }
    Ok(())
}

fn now_ms() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.as_millis()).unwrap_or(u64::MAX)
}

fn deterministic_signature_hex(tag: &str, payload: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag.as_bytes());
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize().as_bytes())
}

fn signed_action_clock_key(action_tag: &str, signer_id: &str) -> String {
    format!("{action_tag}:{signer_id}")
}

fn enforce_signed_action_clock(
    room: &mut RoomState,
    action_tag: &str,
    signer_id: &str,
    timestamp_label: &str,
    timestamp_ms: u64,
) -> std::result::Result<(), String> {
    if timestamp_ms == 0 {
        return Err(format!("{timestamp_label} must be >= 1"));
    }

    let key = signed_action_clock_key(action_tag, signer_id);
    if let Some(last_seen) = room.signed_action_clock_by_signer.get(&key)
        && timestamp_ms <= *last_seen
    {
        return Err(format!(
            "{action_tag} {timestamp_label} must increase for signer {signer_id}: last={last_seen} got={timestamp_ms} (replay/stale rejected)"
        ));
    }

    room.signed_action_clock_by_signer.insert(key, timestamp_ms);
    Ok(())
}

fn role_kind_token(role: &RoleKind) -> &'static str {
    match role {
        RoleKind::Host => "host",
        RoleKind::CoHost => "cohost",
        RoleKind::Participant => "participant",
    }
}

fn moderation_action_token(action: &ModerationAction) -> &'static str {
    match action {
        ModerationAction::DisableMic => "disable_mic",
        ModerationAction::DisableVideo => "disable_video",
        ModerationAction::DisableScreenShare => "disable_screen_share",
        ModerationAction::Kick => "kick",
        ModerationAction::AdmitFromWaiting => "admit_from_waiting",
        ModerationAction::DenyFromWaiting => "deny_from_waiting",
    }
}

fn moderation_target_token(target: &ModerationTarget) -> String {
    match target {
        ModerationTarget::All => "all".to_string(),
        ModerationTarget::Participant(participant_id) => {
            format!("participant:{participant_id}")
        }
    }
}

fn moderation_signed_signature_is_valid(moderation: &ModerationSignedFrame) -> bool {
    let expected = deterministic_signature_hex(
        "moderation",
        &format!(
            "{}|{}|{}|{}",
            moderation.issued_by,
            moderation_target_token(&moderation.target),
            moderation_action_token(&moderation.action),
            moderation.sent_at_ms
        ),
    );
    expected == moderation.signature_hex.to_ascii_lowercase()
}

fn role_grant_signature_is_valid(grant: &RoleGrantFrame) -> bool {
    let expected = deterministic_signature_hex(
        "role_grant",
        &format!(
            "{}|{}|{}|{}",
            grant.granted_by,
            grant.target_participant_id,
            role_kind_token(&grant.role),
            grant.issued_at_ms
        ),
    );
    expected == grant.signature_hex.to_ascii_lowercase()
}

fn role_revoke_signature_is_valid(revoke: &RoleRevokeFrame) -> bool {
    let expected = deterministic_signature_hex(
        "role_revoke",
        &format!(
            "{}|{}|{}|{}",
            revoke.revoked_by,
            revoke.target_participant_id,
            role_kind_token(&revoke.role),
            revoke.issued_at_ms
        ),
    );
    expected == revoke.signature_hex.to_ascii_lowercase()
}

fn session_policy_signature_is_valid(policy: &SessionPolicyFrame) -> bool {
    let expected = deterministic_signature_hex(
        "session_policy",
        &format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            policy.updated_by,
            policy.room_lock,
            policy.waiting_room_enabled,
            policy.guest_join_allowed,
            policy.local_recording_allowed,
            policy.e2ee_required,
            policy.max_participants,
            policy.policy_epoch,
            policy.updated_at_ms
        ),
    );
    expected == policy.signature_hex.to_ascii_lowercase()
}

fn e2ee_public_key_is_valid(frame: &E2EEKeyEpochFrame) -> bool {
    let expected = deterministic_signature_hex(
        "e2ee_public_key",
        &format!("{}|{}", frame.participant_id, frame.epoch),
    );
    expected == frame.public_key_hex.to_ascii_lowercase()
}

fn e2ee_signature_is_valid(frame: &E2EEKeyEpochFrame) -> bool {
    let expected = deterministic_signature_hex(
        "e2ee_key_epoch",
        &format!(
            "{}|{}|{}",
            frame.participant_id, frame.epoch, frame.sent_at_ms
        ),
    );
    expected == frame.signature_hex.to_ascii_lowercase()
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

fn is_valid_tx_hash_hex(hash: &str) -> bool {
    let normalized = strip_hex_prefix(hash);
    normalized.len() == 64 && hex::decode(normalized).is_ok()
}

fn is_valid_hex(value: &str) -> bool {
    let normalized = strip_hex_prefix(value);
    !normalized.is_empty() && normalized.len().is_multiple_of(2) && hex::decode(normalized).is_ok()
}

fn is_valid_hex_len(value: &str, bytes: usize) -> bool {
    let normalized = strip_hex_prefix(value);
    normalized.len() == bytes * 2 && hex::decode(normalized).is_ok()
}

fn has_disallowed_anon_handle_chars(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn has_forbidden_anon_handle_symbol(value: &str) -> bool {
    value.contains('@')
}

fn apply_anon_hello_state(
    participant: &mut ParticipantState,
    hello: &AnonHelloFrame,
    now_ms: u64,
) -> std::result::Result<(), String> {
    let first_anonymous_hello = !participant.hello_seen || !participant.anonymous_mode;
    if participant.hello_seen
        && participant.anonymous_mode
        && participant
            .x25519_pubkey_hex
            .as_deref()
            .is_some_and(|current| current != hello.x25519_pubkey_hex.as_str())
    {
        return Err("x25519 key changes require GroupKeyUpdate".to_string());
    }

    participant.anonymous_mode = true;
    participant.waiting_room_pending = false;
    participant.hello_seen = true;
    participant.participant_handle = Some(hello.participant_handle.clone());
    participant.x25519_pubkey_hex = Some(hello.x25519_pubkey_hex.clone());
    if participant.x25519_epoch == 0 {
        participant.x25519_epoch = 1;
    }
    if first_anonymous_hello {
        participant.last_billed_at_ms = now_ms;
    }
    Ok(())
}

fn should_apply_group_key_update(
    current_epoch: u64,
    current_pubkey_hex: Option<&str>,
    update_epoch: u64,
    update_pubkey_hex: &str,
) -> std::result::Result<bool, String> {
    if update_epoch == 0 {
        return Err("group key epoch must be >= 1".to_string());
    }
    if update_epoch < current_epoch {
        return Err(format!(
            "group key epoch is stale: current_epoch={current_epoch} update_epoch={update_epoch}"
        ));
    }
    if update_epoch == current_epoch {
        if current_pubkey_hex == Some(update_pubkey_hex) {
            return Ok(false);
        }
        return Err(
            "group key rotation requires strictly increasing epoch for this sender".to_string(),
        );
    }
    Ok(true)
}

fn validate_group_key_update_frame(update: &GroupKeyUpdateFrame) -> Option<String> {
    if update.participant_handle.trim().is_empty() {
        return Some("group key participant_handle must be non-empty".to_string());
    }
    if !update.participant_handle.is_ascii() {
        return Some("group key participant_handle must be ASCII".to_string());
    }
    if has_forbidden_anon_handle_symbol(&update.participant_handle) {
        return Some("group key participant_handle must not contain '@'".to_string());
    }
    if has_disallowed_anon_handle_chars(&update.participant_handle) {
        return Some(
            "group key participant_handle must not contain whitespace/control chars".to_string(),
        );
    }
    if update.participant_handle.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
        return Some(format!(
            "group key participant_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
        ));
    }
    if !is_valid_hex_len(&update.x25519_pubkey_hex, 32) {
        return Some("x25519_pubkey_hex must be 32-byte hex".to_string());
    }
    if update.epoch == 0 {
        return Some("group key epoch must be >= 1".to_string());
    }
    None
}

fn validate_encrypted_control_frame(enc: &EncryptedControlFrame) -> Option<String> {
    if enc.epoch == 0 {
        return Some("encrypted control epoch must be >= 1".to_string());
    }
    if enc.sender_handle.trim().is_empty() {
        return Some("encrypted sender handle must be non-empty".to_string());
    }
    if !enc.sender_handle.is_ascii() {
        return Some("encrypted sender handle must be ASCII".to_string());
    }
    if has_forbidden_anon_handle_symbol(&enc.sender_handle) {
        return Some("encrypted sender handle must not contain '@'".to_string());
    }
    if has_disallowed_anon_handle_chars(&enc.sender_handle) {
        return Some(
            "encrypted sender handle must not contain whitespace/control chars".to_string(),
        );
    }
    if enc.sender_handle.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
        return Some(format!(
            "encrypted sender handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
        ));
    }
    if enc.payloads.is_empty() {
        return Some("encrypted control payloads must be non-empty".to_string());
    }
    if enc.payloads.len() > MAX_ENCRYPTED_RECIPIENTS_PER_FRAME {
        return Some(format!(
            "encrypted payload fanout too large: max {MAX_ENCRYPTED_RECIPIENTS_PER_FRAME} recipients"
        ));
    }

    let mut recipients = HashSet::<&str>::new();
    for payload in &enc.payloads {
        let normalized_ciphertext = strip_hex_prefix(&payload.ciphertext_hex);
        if payload.recipient_handle.trim().is_empty()
            || !is_valid_hex_len(&payload.nonce_hex, 24)
            || payload.ciphertext_hex.is_empty()
        {
            return Some(
                "encrypted payloads must include recipient, 24-byte nonce, and ciphertext"
                    .to_string(),
            );
        }
        if payload.recipient_handle.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
            return Some(format!(
                "encrypted recipient handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ));
        }
        if !payload.recipient_handle.is_ascii() {
            return Some("encrypted recipient handle must be ASCII".to_string());
        }
        if has_forbidden_anon_handle_symbol(&payload.recipient_handle) {
            return Some("encrypted recipient handle must not contain '@'".to_string());
        }
        if has_disallowed_anon_handle_chars(&payload.recipient_handle) {
            return Some(
                "encrypted recipient handle must not contain whitespace/control chars".to_string(),
            );
        }
        if !is_valid_hex(&payload.ciphertext_hex) {
            return Some("encrypted ciphertext must be valid hex".to_string());
        }
        if normalized_ciphertext.len() > MAX_ENCRYPTED_CIPHERTEXT_HEX_LEN {
            return Some(format!(
                "encrypted ciphertext too long: max {MAX_ENCRYPTED_CIPHERTEXT_HEX_LEN} hex chars"
            ));
        }
        if normalized_ciphertext.len() < MIN_ENCRYPTED_CIPHERTEXT_HEX_LEN {
            return Some(format!(
                "encrypted ciphertext too short: min {MIN_ENCRYPTED_CIPHERTEXT_HEX_LEN} hex chars"
            ));
        }
        if !recipients.insert(payload.recipient_handle.as_str()) {
            return Some("encrypted payload recipients must be unique".to_string());
        }
    }

    None
}

fn validate_encrypted_control_room_recipients(
    enc: &EncryptedControlFrame,
    room: &RoomState,
) -> Option<String> {
    let roster_handles: HashSet<&str> = room
        .participants
        .values()
        .filter(|participant| participant.state.anonymous_mode && participant.state.hello_seen)
        .filter_map(|participant| participant.state.participant_handle.as_deref())
        .collect();
    for payload in &enc.payloads {
        if !roster_handles.contains(payload.recipient_handle.as_str()) {
            return Some(format!(
                "encrypted recipient handle is not in anonymous roster: {}",
                payload.recipient_handle
            ));
        }
    }
    None
}

fn validate_encrypted_control_epoch(frame_epoch: u64, sender_epoch: u64) -> Option<String> {
    if sender_epoch == 0 {
        return Some("sender key epoch is not initialized".to_string());
    }
    if frame_epoch != sender_epoch {
        return Some(format!(
            "encrypted control epoch mismatch: expected {sender_epoch} got {frame_epoch}"
        ));
    }
    None
}

fn validate_escrow_proof_frame(proof: &EscrowProofFrame) -> Option<String> {
    if proof.payer_handle.trim().is_empty() {
        return Some("payer_handle must be non-empty".to_string());
    }
    if !proof.payer_handle.is_ascii() {
        return Some("payer_handle must be ASCII".to_string());
    }
    if has_forbidden_anon_handle_symbol(&proof.payer_handle) {
        return Some("payer_handle must not contain '@'".to_string());
    }
    if has_disallowed_anon_handle_chars(&proof.payer_handle) {
        return Some("payer_handle must not contain whitespace/control chars".to_string());
    }
    if proof.payer_handle.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
        return Some(format!(
            "payer_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
        ));
    }
    if proof.escrow_id.trim().is_empty() {
        return Some("escrow_id must be non-empty".to_string());
    }
    if !proof.escrow_id.is_ascii() {
        return Some("escrow_id must be ASCII".to_string());
    }
    if proof.escrow_id.contains('@') {
        return Some("escrow_id must not contain '@'".to_string());
    }
    if proof
        .escrow_id
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Some("escrow_id must not contain whitespace/control chars".to_string());
    }
    if proof.escrow_id.len() > MAX_ESCROW_ID_LEN {
        return Some(format!("escrow_id too long: max {MAX_ESCROW_ID_LEN} chars"));
    }
    let normalized = strip_hex_prefix(&proof.proof_hex);
    if normalized.len() > MAX_ESCROW_PROOF_HEX_LEN {
        return Some(format!(
            "proof_hex too long: max {MAX_ESCROW_PROOF_HEX_LEN} hex chars"
        ));
    }
    if !is_valid_hex(&proof.proof_hex) {
        return Some("proof_hex must be valid hex".to_string());
    }
    None
}

fn validate_escrow_id_consistency(current: Option<&str>, incoming: &str) -> Option<String> {
    if current.is_some_and(|expected| expected != incoming) {
        return Some("escrow_id must remain stable for a participant session".to_string());
    }
    None
}

fn validate_device_capability_frame(cap: &DeviceCapabilityFrame) -> Option<String> {
    if cap.participant_id.trim().is_empty() {
        return Some("device capability participant_id must be non-empty".to_string());
    }
    if cap.max_video_streams == 0 {
        return Some("device capability max_video_streams must be >= 1".to_string());
    }
    if cap.codecs.is_empty() {
        return Some("device capability codecs must be non-empty".to_string());
    }
    if cap.codecs.iter().any(|codec| codec.trim().is_empty()) {
        return Some("device capability codec entries must be non-empty".to_string());
    }
    None
}

fn validate_media_profile_negotiation_frame(
    profile: &MediaProfileNegotiationFrame,
) -> Option<String> {
    if profile.participant_id.trim().is_empty() {
        return Some("media profile participant_id must be non-empty".to_string());
    }
    if profile.codec.trim().is_empty() {
        return Some("media profile codec must be non-empty".to_string());
    }
    if profile.epoch == 0 {
        return Some("media profile epoch must be >= 1".to_string());
    }
    None
}

fn validate_recording_notice_frame(notice: &RecordingNoticeFrame) -> Option<String> {
    if notice.participant_id.trim().is_empty() {
        return Some("recording notice participant_id must be non-empty".to_string());
    }
    if notice.issued_by.trim().is_empty() {
        return Some("recording notice issued_by must be non-empty".to_string());
    }
    None
}

fn validate_e2ee_key_epoch_frame(frame: &E2EEKeyEpochFrame) -> Option<String> {
    if frame.participant_id.trim().is_empty() {
        return Some("e2ee key epoch participant_id must be non-empty".to_string());
    }
    if frame.epoch == 0 {
        return Some("e2ee key epoch must be >= 1".to_string());
    }
    if !is_valid_hex_len(&frame.public_key_hex, 32) {
        return Some("e2ee key epoch public_key_hex must be 32-byte hex".to_string());
    }
    if !is_valid_hex_len(&frame.signature_hex, 32) {
        return Some("e2ee key epoch signature_hex must be 32-byte hex".to_string());
    }
    if !e2ee_public_key_is_valid(frame) {
        return Some("e2ee key epoch public_key_hex failed verification".to_string());
    }
    if !e2ee_signature_is_valid(frame) {
        return Some("e2ee key epoch signature_hex failed verification".to_string());
    }
    None
}

fn record_anon_admission_rejection(room: &mut RoomState) -> u64 {
    room.anon_admission_rejections = room.anon_admission_rejections.saturating_add(1);
    room.anon_admission_rejections
}

fn validate_anonymous_room_capacity(
    room: &RoomState,
    conn_id: ConnId,
    max_anon_participants: usize,
) -> Option<String> {
    let already_anonymous = room.participants.get(&conn_id).is_some_and(|participant| {
        participant.state.anonymous_mode && participant.state.hello_seen
    });
    if already_anonymous {
        return None;
    }

    let active_anon = room
        .participants
        .values()
        .filter(|participant| participant.state.anonymous_mode && participant.state.hello_seen)
        .count();
    if active_anon >= max_anon_participants {
        return Some(format!(
            "anonymous room participant cap reached: max {max_anon_participants}"
        ));
    }
    None
}

fn validate_anon_participant_handle(
    room: &RoomState,
    conn_id: ConnId,
    requested_handle: &str,
) -> Option<String> {
    if requested_handle.trim().is_empty() {
        return Some("participant_handle must be non-empty".to_string());
    }
    if !requested_handle.is_ascii() {
        return Some("participant_handle must be ASCII".to_string());
    }
    if has_forbidden_anon_handle_symbol(requested_handle) {
        return Some("participant_handle must not contain '@'".to_string());
    }
    if has_disallowed_anon_handle_chars(requested_handle) {
        return Some("participant_handle must not contain whitespace/control chars".to_string());
    }
    if requested_handle.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
        return Some(format!(
            "participant_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
        ));
    }
    if room.participants.iter().any(|(id, participant)| {
        *id != conn_id
            && participant.state.anonymous_mode
            && participant.state.participant_handle.as_deref() == Some(requested_handle)
    }) {
        return Some("participant_handle already in use".to_string());
    }
    if let Some(existing) = room
        .participants
        .get(&conn_id)
        .and_then(|p| p.state.participant_handle.as_deref())
        && existing != requested_handle
    {
        return Some("participant_handle cannot change after AnonHello".to_string());
    }
    None
}

fn escrow_proof_stale(last_proof_at_ms: u64, now_ms: u64, max_stale_secs: u64) -> bool {
    now_ms.saturating_sub(last_proof_at_ms) > max_stale_secs.saturating_mul(1000)
}

fn anonymous_hello_timed_out(connected_at_ms: u64, now_ms: u64, max_wait_secs: u64) -> bool {
    escrow_proof_stale(connected_at_ms, now_ms, max_wait_secs)
}

fn enforce_join_media_defaults(hello: &mut HelloFrame) -> bool {
    let forced_off = hello.mic_enabled || hello.video_enabled || hello.screen_share_enabled;
    hello.mic_enabled = false;
    hello.video_enabled = false;
    hello.screen_share_enabled = false;
    forced_off
}

fn include_sender_in_all_target(action: &ModerationAction) -> bool {
    matches!(action, ModerationAction::Kick)
}

fn normalize_anon_max_participants(value: usize) -> usize {
    value.max(1)
}

fn should_warn_high_anon_capacity(value: usize) -> bool {
    value > WARN_ANON_MAX_PARTICIPANTS_THRESHOLD
}

fn is_anon_escrow_stale_enforcement_disabled(value: u64) -> bool {
    value == 0
}

fn should_warn_high_anon_escrow_stale_secs(value: u64) -> bool {
    value > WARN_ANON_ESCROW_PROOF_STALE_SECS_THRESHOLD
}

fn normalize_rate_for_policy(rate_per_minute_nano: u64, allow_free_calls: bool) -> u64 {
    if allow_free_calls {
        rate_per_minute_nano
    } else {
        rate_per_minute_nano.max(1)
    }
}

fn normalize_initial_rate(rate_per_minute_nano: u64, allow_free_calls: bool) -> Result<u64> {
    if rate_per_minute_nano == 0 && !allow_free_calls {
        return Err(anyhow!(
            "--xor-rate-per-minute-nano must be > 0 unless --allow-free-calls is set"
        ));
    }
    Ok(normalize_rate_for_policy(
        rate_per_minute_nano,
        allow_free_calls,
    ))
}

fn apply_anonymous_zk_surcharge(
    base_rate_per_minute_nano: u64,
    extra_fee_per_minute_nano: u64,
) -> std::result::Result<u64, String> {
    if extra_fee_per_minute_nano == 0 {
        return Ok(base_rate_per_minute_nano);
    }
    base_rate_per_minute_nano
        .checked_add(extra_fee_per_minute_nano)
        .ok_or_else(|| {
            "anonymous surcharge overflow: room_rate_per_minute_nano + --anon-zk-extra-fee-per-minute-nano exceeds u64"
                .to_string()
        })
}

fn init_tracing(level: &str) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(level))
        .context("parse log filter")?;

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_participant_with_handle(handle: Option<&str>, anonymous_mode: bool) -> Participant {
        let (tx, _rx) = mpsc::channel::<Message>(1);
        Participant {
            tx,
            state: ParticipantState {
                participant_id: "p".to_string(),
                display_name: None,
                participant_handle: handle.map(ToString::to_string),
                x25519_pubkey_hex: None,
                x25519_epoch: 0,
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                anonymous_mode,
                waiting_room_pending: false,
                hello_seen: true,
                last_billed_at_ms: 0,
                billed_nano_xor: 0,
                billing_remainder_mod_60k: 0,
                paid_nano_xor: 0,
                last_payment_at_ms: None,
                last_escrow_proof_at_ms: None,
                escrow_id: None,
            },
        }
    }

    fn test_pending_participant() -> Participant {
        let (tx, _rx) = mpsc::channel::<Message>(1);
        Participant {
            tx,
            state: ParticipantState {
                participant_id: "pending".to_string(),
                display_name: None,
                participant_handle: None,
                x25519_pubkey_hex: None,
                x25519_epoch: 0,
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                anonymous_mode: false,
                waiting_room_pending: false,
                hello_seen: false,
                last_billed_at_ms: 0,
                billed_nano_xor: 0,
                billing_remainder_mod_60k: 0,
                paid_nano_xor: 0,
                last_payment_at_ms: None,
                last_escrow_proof_at_ms: None,
                escrow_id: None,
            },
        }
    }

    fn test_pending_participant_with_rx() -> (Participant, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel::<Message>(32);
        (
            Participant {
                tx,
                state: ParticipantState {
                    participant_id: "pending".to_string(),
                    display_name: None,
                    participant_handle: None,
                    x25519_pubkey_hex: None,
                    x25519_epoch: 0,
                    mic_enabled: false,
                    video_enabled: false,
                    screen_share_enabled: false,
                    anonymous_mode: false,
                    waiting_room_pending: false,
                    hello_seen: false,
                    last_billed_at_ms: 0,
                    billed_nano_xor: 0,
                    billing_remainder_mod_60k: 0,
                    paid_nano_xor: 0,
                    last_payment_at_ms: None,
                    last_escrow_proof_at_ms: None,
                    escrow_id: None,
                },
            },
            rx,
        )
    }

    fn test_joined_participant(participant_id: &str) -> Participant {
        let (tx, _rx) = mpsc::channel::<Message>(1);
        Participant {
            tx,
            state: ParticipantState {
                participant_id: participant_id.to_string(),
                display_name: Some(participant_id.to_string()),
                participant_handle: None,
                x25519_pubkey_hex: None,
                x25519_epoch: 0,
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                anonymous_mode: false,
                waiting_room_pending: false,
                hello_seen: true,
                last_billed_at_ms: 0,
                billed_nano_xor: 0,
                billing_remainder_mod_60k: 0,
                paid_nano_xor: 0,
                last_payment_at_ms: None,
                last_escrow_proof_at_ms: None,
                escrow_id: None,
            },
        }
    }

    fn test_joined_participant_with_rx(
        participant_id: &str,
    ) -> (Participant, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel::<Message>(32);
        (
            Participant {
                tx,
                state: ParticipantState {
                    participant_id: participant_id.to_string(),
                    display_name: Some(participant_id.to_string()),
                    participant_handle: None,
                    x25519_pubkey_hex: None,
                    x25519_epoch: 0,
                    mic_enabled: false,
                    video_enabled: false,
                    screen_share_enabled: false,
                    anonymous_mode: false,
                    waiting_room_pending: false,
                    hello_seen: true,
                    last_billed_at_ms: 0,
                    billed_nano_xor: 0,
                    billing_remainder_mod_60k: 0,
                    paid_nano_xor: 0,
                    last_payment_at_ms: None,
                    last_escrow_proof_at_ms: None,
                    escrow_id: None,
                },
            },
            rx,
        )
    }

    fn test_hub_state_with_room(room_id: RoomId, room: RoomState) -> HubState {
        let mut rooms = HashMap::new();
        rooms.insert(room_id, room);
        HubState {
            rooms: Arc::new(Mutex::new(rooms)),
            next_conn_id: Arc::new(AtomicU64::new(100)),
            billing: BillingConfig {
                default_rate_per_minute_nano: 1,
                anon_zk_extra_fee_per_minute_nano: 0,
                grace_secs: 30,
                check_interval_secs: 5,
                anon_escrow_proof_max_stale_secs: 90,
            },
            anon_max_participants: DEFAULT_ANON_MAX_PARTICIPANTS,
            allow_free_calls: true,
            require_payment_tx_hash: false,
        }
    }

    fn test_peer() -> SocketAddr {
        "127.0.0.1:49000".parse().expect("valid test peer addr")
    }

    fn decode_message_frame(msg: Message) -> KaigiFrame {
        let Message::Binary(bytes) = msg else {
            panic!("expected binary frame message");
        };
        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        decoder
            .try_next()
            .expect("decode frame")
            .expect("frame present")
    }

    fn drain_frames(rx: &mut mpsc::Receiver<Message>) -> Vec<KaigiFrame> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let Message::Binary(_) = msg {
                out.push(decode_message_frame(msg));
            }
        }
        out
    }

    fn drain_all_messages(rxs: &mut HashMap<ConnId, mpsc::Receiver<Message>>) {
        for rx in rxs.values_mut() {
            while rx.try_recv().is_ok() {}
        }
    }

    fn contains_error_message(frames: &[KaigiFrame], needle: &str) -> bool {
        frames.iter().any(|frame| {
            matches!(
                frame,
                KaigiFrame::Error(ErrorFrame { message }) if message.contains(needle)
            )
        })
    }

    fn test_encrypted_payload(recipient_handle: &str) -> kaigi_wire::EncryptedRecipientPayload {
        kaigi_wire::EncryptedRecipientPayload {
            recipient_handle: recipient_handle.to_string(),
            nonce_hex: "11".repeat(24),
            ciphertext_hex: "aa".repeat(16),
        }
    }

    fn test_encrypted_control(sender_handle: &str) -> EncryptedControlFrame {
        EncryptedControlFrame {
            sent_at_ms: 1,
            sender_handle: sender_handle.to_string(),
            epoch: 1,
            kind: kaigi_wire::EncryptedControlKind::Chat,
            payloads: vec![test_encrypted_payload("anon-b")],
        }
    }

    fn test_group_key_update(
        participant_handle: &str,
        x25519_pubkey_hex: &str,
        epoch: u64,
    ) -> GroupKeyUpdateFrame {
        GroupKeyUpdateFrame {
            sent_at_ms: 1,
            participant_handle: participant_handle.to_string(),
            x25519_pubkey_hex: x25519_pubkey_hex.to_string(),
            epoch,
        }
    }

    fn test_escrow_proof(escrow_id: &str, proof_hex: &str) -> EscrowProofFrame {
        EscrowProofFrame {
            sent_at_ms: 1,
            payer_handle: "anon-a".to_string(),
            escrow_id: escrow_id.to_string(),
            proof_hex: proof_hex.to_string(),
        }
    }

    fn test_room_state(anonymous_mode: bool) -> RoomState {
        RoomState {
            participants: HashMap::new(),
            device_caps_by_conn: HashMap::new(),
            host_conn_id: None,
            host_role_owner: None,
            co_host_conn_ids: HashSet::new(),
            co_host_role_owners: HashSet::new(),
            e2ee_epochs: HashMap::new(),
            key_rotation_ack_epochs: HashMap::new(),
            signed_action_clock_by_signer: HashMap::new(),
            presence_sequence: 0,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: DEFAULT_MAX_PARTICIPANTS,
            policy_epoch: 0,
            policy_updated_at_ms: 0,
            policy_updated_by: SYSTEM_POLICY_UPDATED_BY.to_string(),
            policy_signature_hex: String::new(),
            permissions_epoch: 0,
            anonymous_mode,
            anon_admission_rejections: 0,
            backpressure_dropped_messages: 0,
            backpressure_last_notice_at_ms: 0,
        }
    }

    #[test]
    fn anon_handle_validation_rejects_empty_and_duplicate() {
        let mut room = test_room_state(true);
        room.participants
            .insert(1, test_participant_with_handle(Some("anon-a"), true));
        room.participants
            .insert(2, test_participant_with_handle(Some("anon-b"), true));

        assert_eq!(
            validate_anon_participant_handle(&room, 1, " "),
            Some("participant_handle must be non-empty".to_string())
        );
        assert_eq!(
            validate_anon_participant_handle(&room, 1, "anon-b"),
            Some("participant_handle already in use".to_string())
        );
        assert_eq!(
            validate_anon_participant_handle(&room, 1, "anon b"),
            Some("participant_handle must not contain whitespace/control chars".to_string())
        );
        assert_eq!(
            validate_anon_participant_handle(&room, 1, "alice@sora"),
            Some("participant_handle must not contain '@'".to_string())
        );
        assert_eq!(
            validate_anon_participant_handle(&room, 1, "匿名"),
            Some("participant_handle must be ASCII".to_string())
        );
        assert_eq!(
            validate_anon_participant_handle(
                &room,
                1,
                &"a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1)
            ),
            Some(format!(
                "participant_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ))
        );
    }

    #[test]
    fn anon_handle_validation_allows_rehello_and_blocks_handle_change() {
        let mut room = test_room_state(true);
        room.participants
            .insert(1, test_participant_with_handle(Some("anon-a"), true));

        assert_eq!(validate_anon_participant_handle(&room, 1, "anon-a"), None);
        assert_eq!(
            validate_anon_participant_handle(&room, 1, "anon-c"),
            Some("participant_handle cannot change after AnonHello".to_string())
        );
    }

    #[test]
    fn anonymous_capacity_validation_rejects_new_join_when_full() {
        let mut room = test_room_state(true);
        for i in 0..DEFAULT_ANON_MAX_PARTICIPANTS {
            room.participants.insert(
                (i + 1) as u64,
                test_participant_with_handle(Some(&format!("anon-{i:03}")), true),
            );
        }
        let newcomer_conn_id = 10_000u64;
        room.participants
            .insert(newcomer_conn_id, test_pending_participant());

        assert_eq!(
            validate_anonymous_room_capacity(
                &room,
                newcomer_conn_id,
                DEFAULT_ANON_MAX_PARTICIPANTS
            ),
            Some(format!(
                "anonymous room participant cap reached: max {DEFAULT_ANON_MAX_PARTICIPANTS}"
            ))
        );
    }

    #[test]
    fn anonymous_capacity_validation_allows_rehello_when_full() {
        let mut room = test_room_state(true);
        for i in 0..DEFAULT_ANON_MAX_PARTICIPANTS {
            room.participants.insert(
                (i + 1) as u64,
                test_participant_with_handle(Some(&format!("anon-{i:03}")), true),
            );
        }

        assert_eq!(
            validate_anonymous_room_capacity(&room, 1, DEFAULT_ANON_MAX_PARTICIPANTS),
            None
        );
    }

    #[test]
    fn anonymous_admission_rejection_counter_increments() {
        let mut room = test_room_state(true);
        assert_eq!(record_anon_admission_rejection(&mut room), 1);
        assert_eq!(record_anon_admission_rejection(&mut room), 2);
    }

    #[test]
    fn signed_action_clock_rejects_replay_and_stale_timestamps() {
        let mut room = test_room_state(false);

        enforce_signed_action_clock(&mut room, "role_grant", "host", "issued_at_ms", 10)
            .expect("first action should pass");
        let replay =
            enforce_signed_action_clock(&mut room, "role_grant", "host", "issued_at_ms", 10)
                .expect_err("same timestamp should be rejected");
        assert!(replay.contains("replay/stale rejected"));

        let stale = enforce_signed_action_clock(&mut room, "role_grant", "host", "issued_at_ms", 9)
            .expect_err("older timestamp should be rejected");
        assert!(stale.contains("must increase"));

        enforce_signed_action_clock(&mut room, "role_grant", "host", "issued_at_ms", 11)
            .expect("newer timestamp should pass");
        enforce_signed_action_clock(&mut room, "role_revoke", "host", "issued_at_ms", 10)
            .expect("separate action stream should have independent clock");
    }

    #[test]
    fn moderation_sender_clock_rejects_replay_and_stale_sent_at_ms() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);

        let first = ModerationFrame {
            sent_at_ms: 10,
            target: ModerationTarget::All,
            action: ModerationAction::DisableMic,
        };
        validate_moderation_sender_clock(&mut room, 1, &first)
            .expect("first moderation should pass");

        let replay =
            validate_moderation_sender_clock(&mut room, 1, &first).expect_err("replay must fail");
        assert!(replay.contains("replay/stale rejected"));

        let stale = ModerationFrame {
            sent_at_ms: 9,
            ..first.clone()
        };
        let stale_err =
            validate_moderation_sender_clock(&mut room, 1, &stale).expect_err("stale must fail");
        assert!(stale_err.contains("must increase"));

        let next = ModerationFrame {
            sent_at_ms: 11,
            ..first
        };
        validate_moderation_sender_clock(&mut room, 1, &next)
            .expect("newer moderation should pass");
    }

    #[test]
    fn moderation_sender_clock_rejects_zero_sent_at_ms() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);

        let moderation = ModerationFrame {
            sent_at_ms: 0,
            target: ModerationTarget::All,
            action: ModerationAction::DisableMic,
        };
        let err =
            validate_moderation_sender_clock(&mut room, 1, &moderation).expect_err("zero sent_at");
        assert!(err.contains("sent_at_ms must be >= 1"));
    }

    #[test]
    fn moderation_signed_signature_validation_rejects_tampering() {
        let mut signed = ModerationSignedFrame {
            sent_at_ms: 1,
            target: ModerationTarget::All,
            action: ModerationAction::DisableMic,
            issued_by: "host@sora".to_string(),
            signature_hex: String::new(),
        };
        signed.signature_hex =
            deterministic_signature_hex("moderation", "host@sora|all|disable_mic|1");
        assert!(moderation_signed_signature_is_valid(&signed));

        signed.signature_hex = "ab".repeat(32);
        assert!(!moderation_signed_signature_is_valid(&signed));
    }

    #[tokio::test]
    async fn moderation_signed_handle_frame_applies_and_broadcasts_audit() {
        let room_id = [7u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.mic_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let signed = ModerationSignedFrame {
            sent_at_ms: 10,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableMic,
            issued_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "moderation",
                "host@sora|participant:alice@sora|disable_mic|10",
            ),
        };

        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ModerationSigned(signed.clone()),
        )
        .await
        .expect("moderation should be accepted");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::ModerationSigned(moderation) if moderation == &signed)
            ),
            "host should receive moderation signed audit frame"
        );
        assert!(
            alice_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::ModerationSigned(moderation) if moderation == &signed)
            ),
            "target should receive moderation signed audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && !snapshot.mic_enabled
                )
            }),
            "host should receive state update"
        );
        assert!(
            alice_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && !snapshot.mic_enabled
                )
            }),
            "target should receive state update"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            !room
                .participants
                .get(&2)
                .expect("alice exists")
                .state
                .mic_enabled
        );
    }

    #[tokio::test]
    async fn moderation_handle_frame_applies_and_broadcasts_legacy_audit() {
        let room_id = [11u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.mic_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let moderation = ModerationFrame {
            sent_at_ms: 15,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableMic,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::Moderation(moderation.clone()),
        )
        .await
        .expect("moderation should be accepted");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Moderation(legacy) if legacy == &moderation)
            ),
            "host should receive legacy moderation audit frame"
        );
        assert!(
            alice_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Moderation(legacy) if legacy == &moderation)
            ),
            "target should receive legacy moderation audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && !snapshot.mic_enabled
                )
            }),
            "host should receive state update"
        );
        assert!(
            alice_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && !snapshot.mic_enabled
                )
            }),
            "target should receive state update"
        );
    }

    #[tokio::test]
    async fn moderation_handle_frame_disables_screen_share_and_broadcasts_state() {
        let room_id = [35u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.screen_share_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let moderation = ModerationFrame {
            sent_at_ms: 16,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableScreenShare,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::Moderation(moderation.clone()),
        )
        .await
        .expect("screen-share moderation should pass");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Moderation(value) if value == &moderation)
            ),
            "host should receive legacy moderation audit frame"
        );
        assert!(
            alice_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Moderation(value) if value == &moderation)
            ),
            "target should receive legacy moderation audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && !snapshot.screen_share_enabled
                )
            }),
            "host should receive screen-share disabled state update"
        );
        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            !room
                .participants
                .get(&2)
                .expect("alice exists")
                .state
                .screen_share_enabled
        );
    }

    #[tokio::test]
    async fn moderation_handle_frame_kick_sends_error_and_close_to_target() {
        let room_id = [36u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let moderation = ModerationFrame {
            sent_at_ms: 17,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::Kick,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::Moderation(moderation.clone()),
        )
        .await
        .expect("kick moderation should pass");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Moderation(value) if value == &moderation)
            ),
            "host should receive kick moderation audit frame"
        );

        let mut saw_kick_error = false;
        let mut saw_close = false;
        while let Ok(message) = alice_rx.try_recv() {
            match message {
                binary @ Message::Binary(_) => {
                    if let KaigiFrame::Error(ErrorFrame { message }) = decode_message_frame(binary)
                        && message.contains("removed by host: alice@sora")
                    {
                        saw_kick_error = true;
                    }
                }
                Message::Close(_) => {
                    saw_close = true;
                }
                _ => {}
            }
        }
        assert!(
            saw_kick_error,
            "kicked participant should receive error frame"
        );
        assert!(saw_close, "kicked participant should receive close frame");
    }

    #[tokio::test]
    async fn waiting_room_admit_handle_frame_promotes_pending_participant() {
        let room_id = [21u8; 32];
        let mut room = test_room_state(false);
        room.waiting_room_enabled = true;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (guest, mut guest_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, guest);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "guest-user".to_string(),
            display_name: Some("Guest".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("hello should be routed to waiting room");

        let host_frames = drain_frames(&mut host_rx);
        let guest_frames = drain_frames(&mut guest_rx);
        assert!(contains_error_message(
            &host_frames,
            "waiting room: pending admission for participant_id=guest-user"
        ));
        assert!(contains_error_message(
            &guest_frames,
            "waiting room: pending admission for participant_id=guest-user"
        ));

        {
            let rooms = state.rooms.lock().await;
            let room = rooms.get(&room_id).expect("room exists");
            let guest_state = &room.participants.get(&2).expect("guest exists").state;
            assert_eq!(guest_state.participant_id, "guest-user");
            assert!(!guest_state.hello_seen);
            assert!(guest_state.waiting_room_pending);
        }

        let admit = ModerationFrame {
            sent_at_ms: 60,
            target: ModerationTarget::Participant("guest-user".to_string()),
            action: ModerationAction::AdmitFromWaiting,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::Moderation(admit.clone()),
        )
        .await
        .expect("admit moderation should pass");

        let host_frames = drain_frames(&mut host_rx);
        let guest_frames = drain_frames(&mut guest_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Moderation(value) if value == &admit)),
            "host should receive admit moderation audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        sequence,
                        joined,
                        ..
                    }) if *sequence == 1
                        && joined.iter().any(|entry| entry.participant_id == "guest-user")
                )
            }),
            "host should receive admit presence delta with joined participant"
        );
        assert!(
            guest_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Event(RoomEventFrame::Joined(snapshot)) if snapshot.participant_id == "guest-user")),
            "admitted participant should receive joined event"
        );
        assert!(
            guest_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Roster(_))),
            "admitted participant should receive roster snapshot"
        );
        assert!(
            guest_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoomConfig(_))),
            "admitted participant should receive room config"
        );
        assert!(
            guest_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::SessionPolicy(_))),
            "admitted participant should receive session policy"
        );
        assert!(
            guest_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        joined,
                        ..
                    }) if joined.iter().any(|entry| entry.participant_id == "guest-user")
                )
            }),
            "admitted participant should receive admit presence delta"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        let guest_state = &room
            .participants
            .get(&2)
            .expect("guest should remain after admission")
            .state;
        assert!(guest_state.hello_seen);
        assert!(!guest_state.waiting_room_pending);
        assert_eq!(room.e2ee_epochs.get(&2), Some(&0));
    }

    #[tokio::test]
    async fn waiting_room_deny_handle_frame_disconnects_pending_participant() {
        let room_id = [22u8; 32];
        let mut room = test_room_state(false);
        room.waiting_room_enabled = true;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (guest, mut guest_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, guest);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "guest-user".to_string(),
            display_name: Some("Guest".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("hello should be routed to waiting room");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut guest_rx);

        let deny = ModerationFrame {
            sent_at_ms: 61,
            target: ModerationTarget::Participant("guest-user".to_string()),
            action: ModerationAction::DenyFromWaiting,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::Moderation(deny.clone()),
        )
        .await
        .expect("deny moderation should pass");

        let host_frames = drain_frames(&mut host_rx);
        let guest_frames = drain_frames(&mut guest_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Moderation(value) if value == &deny)),
            "host should receive deny moderation audit frame"
        );
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::ParticipantPresenceDelta(_))),
            "denied pending participant should not emit joined/left presence delta"
        );
        assert!(contains_error_message(
            &guest_frames,
            "waiting room admission denied: guest-user"
        ));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            !room.participants.contains_key(&2),
            "denied pending participant should be disconnected"
        );
        assert_eq!(room.e2ee_epochs.get(&2), None);
    }

    #[tokio::test]
    async fn hello_handle_frame_rejects_guest_when_guest_policy_disabled() {
        let room_id = [23u8; 32];
        let mut room = test_room_state(false);
        room.guest_join_allowed = false;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (guest, mut guest_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, guest);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "guest-user".to_string(),
            display_name: Some("Guest".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("guest join rejection should return error frame");

        let host_frames = drain_frames(&mut host_rx);
        let guest_frames = drain_frames(&mut guest_rx);
        assert!(
            host_frames.is_empty(),
            "host should not receive join events for rejected guest"
        );
        assert!(contains_error_message(
            &guest_frames,
            "guest participants are not allowed by room policy"
        ));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        let guest_state = &room
            .participants
            .get(&2)
            .expect("guest still tracked")
            .state;
        assert!(!guest_state.hello_seen);
        assert_eq!(room.e2ee_epochs.get(&2), None);
    }

    #[tokio::test]
    async fn hello_handle_frame_rejects_join_when_room_lock_enabled() {
        let room_id = [31u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (guest, mut guest_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, guest);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let policy = SessionPolicyFrame {
            updated_at_ms: 55,
            room_lock: true,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: 500,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|true|false|true|true|true|500|1|55",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(policy),
        )
        .await
        .expect("host should be able to lock room");
        let _ = drain_frames(&mut host_rx);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "guest-user".to_string(),
            display_name: Some("Guest".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("locked room rejection should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let guest_frames = drain_frames(&mut guest_rx);
        assert!(
            host_frames.is_empty(),
            "host should not receive join events for locked-room rejection"
        );
        assert!(contains_error_message(&guest_frames, "room is locked"));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        let guest_state = &room.participants.get(&2).expect("guest tracked").state;
        assert!(!guest_state.hello_seen);
        assert_eq!(room.e2ee_epochs.get(&2), None);
    }

    #[tokio::test]
    async fn media_baseline_join_forces_media_off_then_allows_mic_video_updates() {
        let room_id = [32u8; 32];
        let mut room = test_room_state(false);
        room.e2ee_required = false;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "alice@sora".to_string(),
            display_name: Some("Alice".to_string()),
            mic_enabled: true,
            video_enabled: true,
            screen_share_enabled: true,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("join should pass");

        let host_join_frames = drain_frames(&mut host_rx);
        let _alice_join_frames = drain_frames(&mut alice_rx);
        assert!(
            host_join_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::Joined(snapshot))
                        if snapshot.participant_id == "alice@sora"
                            && !snapshot.mic_enabled
                            && !snapshot.video_enabled
                            && !snapshot.screen_share_enabled
                )
            }),
            "join event should force all media controls OFF"
        );

        let update = ParticipantStateFrame {
            updated_at_ms: 71,
            mic_enabled: Some(true),
            video_enabled: Some(true),
            screen_share_enabled: Some(false),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::ParticipantState(update),
        )
        .await
        .expect("mic/video participant-state update should pass");

        let host_state_frames = drain_frames(&mut host_rx);
        assert!(
            host_state_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora"
                            && snapshot.mic_enabled
                            && snapshot.video_enabled
                            && !snapshot.screen_share_enabled
                )
            }),
            "state update should reflect explicit mic/video enable"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        let alice_state = &room.participants.get(&2).expect("alice tracked").state;
        assert!(alice_state.hello_seen);
        assert!(alice_state.mic_enabled);
        assert!(alice_state.video_enabled);
        assert!(!alice_state.screen_share_enabled);
    }

    #[tokio::test]
    async fn reconnect_rejoin_restores_cohost_role_and_policy() {
        let room_id = [37u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        let grant = RoleGrantFrame {
            issued_at_ms: 210,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_grant",
                "host@sora|alice@sora|cohost|210",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(grant),
        )
        .await
        .expect("cohost grant should pass");

        let policy = SessionPolicyFrame {
            updated_at_ms: 220,
            room_lock: true,
            waiting_room_enabled: true,
            guest_join_allowed: false,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 250,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|true|true|false|false|true|250|1|220",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(policy.clone()),
        )
        .await
        .expect("policy update should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        handle_disconnect(&state, room_id, 2)
            .await
            .expect("disconnect should pass");

        let (alice_rejoin, mut alice_rejoin_rx) = test_pending_participant_with_rx();
        {
            let mut rooms = state.rooms.lock().await;
            let room = rooms.get_mut(&room_id).expect("room exists");
            room.participants.insert(3, alice_rejoin);
        }

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "alice@sora".to_string(),
            display_name: Some("Alice".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 3, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("cohost reconnect should join directly");

        let frames = drain_frames(&mut alice_rejoin_rx);
        assert!(
            !contains_error_message(&frames, "room is locked"),
            "reserved cohost reconnect should bypass room lock"
        );
        assert!(
            !contains_error_message(&frames, "waiting room: pending admission"),
            "reserved cohost reconnect should bypass waiting room"
        );
        assert!(
            frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Event(RoomEventFrame::Joined(snapshot)) if snapshot.participant_id == "alice@sora")),
            "rejoining cohost should receive joined event"
        );
        assert!(
            frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::SessionPolicy(value)
                        if value.room_lock
                            && value.waiting_room_enabled
                            && !value.guest_join_allowed
                            && !value.local_recording_allowed
                            && value.policy_epoch == policy.policy_epoch
                            && value.updated_at_ms == policy.updated_at_ms
                )
            }),
            "rejoining participant should receive current session policy"
        );
        assert!(
            frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::PermissionsSnapshot(PermissionsSnapshotFrame {
                        participant_id,
                        co_host,
                        can_moderate,
                        ..
                    }) if participant_id == "alice@sora" && *co_host && *can_moderate
                )
            }),
            "rejoining participant should regain cohost permissions"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(room.co_host_conn_ids.contains(&3));
    }

    #[tokio::test]
    async fn reconnect_rejoin_restores_host_role_after_temporary_disconnect() {
        let room_id = [38u8; 32];
        let mut room = test_room_state(false);
        room.room_lock = true;
        room.waiting_room_enabled = true;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (bob, mut bob_rx) = test_joined_participant_with_rx("bob@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, bob);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        handle_disconnect(&state, room_id, 1)
            .await
            .expect("disconnect should pass");
        {
            let rooms = state.rooms.lock().await;
            let room = rooms.get(&room_id).expect("room exists");
            assert_eq!(
                room.host_conn_id,
                Some(2),
                "fallback host should be assigned while original host is disconnected"
            );
        }
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut bob_rx);

        let (host_rejoin, mut host_rejoin_rx) = test_pending_participant_with_rx();
        {
            let mut rooms = state.rooms.lock().await;
            let room = rooms.get_mut(&room_id).expect("room exists");
            room.participants.insert(3, host_rejoin);
        }
        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "host@sora".to_string(),
            display_name: Some("Host".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 3, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("host reconnect should join directly");

        let host_frames = drain_frames(&mut host_rejoin_rx);
        assert!(
            !contains_error_message(&host_frames, "room is locked"),
            "reserved host reconnect should bypass room lock"
        );
        assert!(
            !contains_error_message(&host_frames, "waiting room: pending admission"),
            "reserved host reconnect should bypass waiting room"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::RoomConfig(RoomConfigFrame { host_participant_id, .. })
                        if host_participant_id.as_deref() == Some("host@sora")
                )
            }),
            "reconnecting host should observe host ownership in room config"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::PermissionsSnapshot(PermissionsSnapshotFrame {
                        participant_id,
                        host,
                        can_moderate,
                        ..
                    }) if participant_id == "host@sora" && *host && *can_moderate
                )
            }),
            "reconnecting host should regain host permissions"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.host_conn_id, Some(3));
    }

    #[tokio::test]
    async fn broadcast_frame_notifies_moderators_when_fanout_backpressures() {
        let room_id = [39u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (slow_tx, mut slow_rx) = mpsc::channel::<Message>(1);
        let slow = Participant {
            tx: slow_tx.clone(),
            state: ParticipantState {
                participant_id: "slow@sora".to_string(),
                display_name: Some("slow@sora".to_string()),
                participant_handle: None,
                x25519_pubkey_hex: None,
                x25519_epoch: 0,
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                anonymous_mode: false,
                waiting_room_pending: false,
                hello_seen: true,
                last_billed_at_ms: 0,
                billed_nano_xor: 0,
                billing_remainder_mod_60k: 0,
                paid_nano_xor: 0,
                last_payment_at_ms: None,
                last_escrow_proof_at_ms: None,
                escrow_id: None,
            },
        };
        room.participants.insert(1, host);
        room.participants.insert(2, slow);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        slow_tx
            .try_send(Message::Close(None))
            .expect("prefill slow participant queue");

        let chat = KaigiFrame::Chat(ChatFrame {
            sent_at_ms: 300,
            from_participant_id: "host@sora".to_string(),
            from_display_name: Some("Host".to_string()),
            text: "hello".to_string(),
        });
        broadcast_frame(&state, &room_id, &chat)
            .await
            .expect("broadcast should not fail");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::Chat(ChatFrame { text, .. }) if text == "hello")
            ),
            "host should still receive broadcasted chat frame"
        );
        assert!(contains_error_message(
            &host_frames,
            "backpressure: dropped fanout to 1 participant(s)"
        ));

        while slow_rx.try_recv().is_ok() {}
    }

    #[tokio::test]
    async fn broadcast_frame_rate_limits_backpressure_notices() {
        let room_id = [42u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (slow_tx, mut slow_rx) = mpsc::channel::<Message>(1);
        let slow = Participant {
            tx: slow_tx.clone(),
            state: ParticipantState {
                participant_id: "slow@sora".to_string(),
                display_name: Some("slow@sora".to_string()),
                participant_handle: None,
                x25519_pubkey_hex: None,
                x25519_epoch: 0,
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                anonymous_mode: false,
                waiting_room_pending: false,
                hello_seen: true,
                last_billed_at_ms: 0,
                billed_nano_xor: 0,
                billing_remainder_mod_60k: 0,
                paid_nano_xor: 0,
                last_payment_at_ms: None,
                last_escrow_proof_at_ms: None,
                escrow_id: None,
            },
        };
        room.participants.insert(1, host);
        room.participants.insert(2, slow);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        slow_tx
            .try_send(Message::Close(None))
            .expect("prefill slow participant queue");

        let first = KaigiFrame::Chat(ChatFrame {
            sent_at_ms: 400,
            from_participant_id: "host@sora".to_string(),
            from_display_name: Some("Host".to_string()),
            text: "first".to_string(),
        });
        broadcast_frame(&state, &room_id, &first)
            .await
            .expect("first broadcast should not fail");

        let second = KaigiFrame::Chat(ChatFrame {
            sent_at_ms: 401,
            from_participant_id: "host@sora".to_string(),
            from_display_name: Some("Host".to_string()),
            text: "second".to_string(),
        });
        broadcast_frame(&state, &room_id, &second)
            .await
            .expect("second broadcast should not fail");

        let host_frames = drain_frames(&mut host_rx);
        let notice_count = host_frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Error(ErrorFrame { message })
                        if message.contains("backpressure: dropped fanout")
                )
            })
            .count();
        assert_eq!(
            notice_count, 1,
            "backpressure notices should be rate-limited"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.backpressure_dropped_messages, 2);

        while slow_rx.try_recv().is_ok() {}
    }

    #[tokio::test]
    async fn roster_remains_consistent_under_join_leave_churn() {
        let room_id = [40u8; 32];
        let mut room = test_room_state(false);
        let (host, host_rx) = test_joined_participant_with_rx("host@sora");
        room.participants.insert(1, host);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        let mut receivers: HashMap<ConnId, mpsc::Receiver<Message>> = HashMap::new();
        receivers.insert(1, host_rx);
        let mut expected_ids = HashSet::new();
        expected_ids.insert("host@sora".to_string());

        for conn_id in 2..=60 {
            let (participant, rx) = test_pending_participant_with_rx();
            {
                let mut rooms = state.rooms.lock().await;
                let room = rooms.get_mut(&room_id).expect("room exists");
                room.participants.insert(conn_id, participant);
            }
            receivers.insert(conn_id, rx);

            let participant_id = format!("user{conn_id}@sora");
            let hello = HelloFrame {
                protocol_version: PROTOCOL_VERSION,
                participant_id: participant_id.clone(),
                display_name: Some(participant_id.clone()),
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                hdr_display: false,
                hdr_capture: false,
            };
            handle_frame(
                &state,
                room_id,
                conn_id,
                test_peer(),
                KaigiFrame::Hello(hello),
            )
            .await
            .expect("churn join should pass");
            expected_ids.insert(participant_id);
            drain_all_messages(&mut receivers);
        }

        for conn_id in (2..=60).step_by(4) {
            handle_disconnect(&state, room_id, conn_id)
                .await
                .expect("churn leave should pass");
            receivers.remove(&conn_id);
            expected_ids.remove(&format!("user{conn_id}@sora"));
            drain_all_messages(&mut receivers);
        }

        for conn_id in 100..=110 {
            let (participant, rx) = test_pending_participant_with_rx();
            {
                let mut rooms = state.rooms.lock().await;
                let room = rooms.get_mut(&room_id).expect("room exists");
                room.participants.insert(conn_id, participant);
            }
            receivers.insert(conn_id, rx);

            let participant_id = format!("late{conn_id}@sora");
            let hello = HelloFrame {
                protocol_version: PROTOCOL_VERSION,
                participant_id: participant_id.clone(),
                display_name: Some(participant_id.clone()),
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                hdr_display: false,
                hdr_capture: false,
            };
            handle_frame(
                &state,
                room_id,
                conn_id,
                test_peer(),
                KaigiFrame::Hello(hello),
            )
            .await
            .expect("late join should pass");
            expected_ids.insert(participant_id);
            drain_all_messages(&mut receivers);
        }

        let roster = roster_frame(&state, room_id).await.expect("roster frame");
        let roster_ids = roster
            .participants
            .iter()
            .map(|entry| entry.participant_id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(roster_ids, expected_ids);
        assert_eq!(roster.participants.len(), expected_ids.len());
        assert!(
            roster
                .participants
                .windows(2)
                .all(|pair| pair[0].participant_id <= pair[1].participant_id),
            "roster should stay deterministically sorted"
        );
    }

    #[tokio::test]
    async fn participant_presence_delta_sequence_is_monotonic_across_join_role_and_leave() {
        let room_id = [44u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_pending_participant_with_rx();
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        let hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "alice@sora".to_string(),
            display_name: Some("Alice".to_string()),
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: false,
        };
        handle_frame(&state, room_id, 2, test_peer(), KaigiFrame::Hello(hello))
            .await
            .expect("join should pass");

        let host_join_frames = drain_frames(&mut host_rx);
        let join_sequence = host_join_frames
            .iter()
            .find_map(|frame| {
                let KaigiFrame::ParticipantPresenceDelta(delta) = frame else {
                    return None;
                };
                if delta
                    .joined
                    .iter()
                    .any(|entry| entry.participant_id == "alice@sora")
                {
                    Some(delta.sequence)
                } else {
                    None
                }
            })
            .expect("join presence delta must be broadcast");
        assert_eq!(join_sequence, 1);
        let _ = drain_frames(&mut alice_rx);

        let grant = RoleGrantFrame {
            issued_at_ms: 120,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_grant",
                "host@sora|alice@sora|cohost|120",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(grant),
        )
        .await
        .expect("role grant should pass");

        let host_role_frames = drain_frames(&mut host_rx);
        let role_sequence = host_role_frames
            .iter()
            .find_map(|frame| {
                let KaigiFrame::ParticipantPresenceDelta(delta) = frame else {
                    return None;
                };
                if delta.role_changes
                    == vec![RoleChangeEntry {
                        participant_id: "alice@sora".to_string(),
                        role: RoleKind::CoHost,
                        granted: true,
                    }]
                {
                    Some(delta.sequence)
                } else {
                    None
                }
            })
            .expect("role-change presence delta must be broadcast");
        assert_eq!(role_sequence, 2);
        let _ = drain_frames(&mut alice_rx);

        handle_disconnect(&state, room_id, 2)
            .await
            .expect("disconnect should pass");

        let host_leave_frames = drain_frames(&mut host_rx);
        let leave_sequence = host_leave_frames
            .iter()
            .find_map(|frame| {
                let KaigiFrame::ParticipantPresenceDelta(delta) = frame else {
                    return None;
                };
                if delta
                    .left
                    .iter()
                    .any(|entry| entry.participant_id == "alice@sora")
                {
                    Some(delta.sequence)
                } else {
                    None
                }
            })
            .expect("leave presence delta must be broadcast");
        assert_eq!(leave_sequence, 3);

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.presence_sequence, 3);
    }

    #[tokio::test]
    async fn long_duration_control_plane_soak_preserves_core_invariants() {
        let room_id = [41u8; 32];
        let mut room = test_room_state(false);
        room.local_recording_allowed = true;
        let (host, host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.host_role_owner = Some("host@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        let mut receivers: HashMap<ConnId, mpsc::Receiver<Message>> = HashMap::new();
        receivers.insert(1, host_rx);
        receivers.insert(2, alice_rx);

        let host_cap = DeviceCapabilityFrame {
            reported_at_ms: 10,
            participant_id: "host@sora".to_string(),
            codecs: vec!["av1".to_string()],
            hdr_capture: false,
            hdr_render: true,
            max_video_streams: 2,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::DeviceCapability(host_cap),
        )
        .await
        .expect("host capability should pass");

        let alice_cap = DeviceCapabilityFrame {
            reported_at_ms: 11,
            participant_id: "alice@sora".to_string(),
            codecs: vec!["av1".to_string()],
            hdr_capture: true,
            hdr_render: true,
            max_video_streams: 2,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::DeviceCapability(alice_cap),
        )
        .await
        .expect("alice capability should pass");

        let host_epoch = E2EEKeyEpochFrame {
            sent_at_ms: 12,
            participant_id: "host@sora".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "host@sora|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "host@sora|1|12"),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(host_epoch),
        )
        .await
        .expect("host e2ee bootstrap should pass");

        let mut alice_epoch = 1u64;
        let alice_epoch_bootstrap = E2EEKeyEpochFrame {
            sent_at_ms: 13,
            participant_id: "alice@sora".to_string(),
            epoch: alice_epoch,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice@sora|1|13"),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(alice_epoch_bootstrap),
        )
        .await
        .expect("alice e2ee bootstrap should pass");
        drain_all_messages(&mut receivers);

        for tick in 1u64..=720 {
            let at_ms = 1_000 + tick * 10;

            handle_frame(
                &state,
                room_id,
                2,
                test_peer(),
                KaigiFrame::Chat(ChatFrame {
                    sent_at_ms: at_ms,
                    from_participant_id: "alice@sora".to_string(),
                    from_display_name: Some("Alice".to_string()),
                    text: format!("tick-{tick}"),
                }),
            )
            .await
            .expect("chat should pass in soak loop");

            handle_frame(
                &state,
                room_id,
                2,
                test_peer(),
                KaigiFrame::ParticipantState(ParticipantStateFrame {
                    updated_at_ms: at_ms + 1,
                    mic_enabled: Some(tick % 2 == 0),
                    video_enabled: Some(tick % 3 == 0),
                    screen_share_enabled: Some(tick % 6 == 0),
                }),
            )
            .await
            .expect("participant state should pass in soak loop");

            if tick % 60 == 0 {
                let profile = MediaProfileNegotiationFrame {
                    at_ms: at_ms + 2,
                    participant_id: "alice@sora".to_string(),
                    requested_profile: MediaProfileKind::Hdr,
                    negotiated_profile: MediaProfileKind::Hdr,
                    codec: "av1".to_string(),
                    epoch: tick / 60,
                };
                handle_frame(
                    &state,
                    room_id,
                    2,
                    test_peer(),
                    KaigiFrame::MediaProfileNegotiation(profile),
                )
                .await
                .expect("media profile negotiation should pass in soak loop");
            }

            if tick % 90 == 0 {
                alice_epoch = alice_epoch.saturating_add(1);
                let epoch_frame = E2EEKeyEpochFrame {
                    sent_at_ms: at_ms + 3,
                    participant_id: "alice@sora".to_string(),
                    epoch: alice_epoch,
                    public_key_hex: deterministic_signature_hex(
                        "e2ee_public_key",
                        &format!("alice@sora|{alice_epoch}"),
                    ),
                    signature_hex: deterministic_signature_hex(
                        "e2ee_key_epoch",
                        &format!("alice@sora|{alice_epoch}|{}", at_ms + 3),
                    ),
                };
                handle_frame(
                    &state,
                    room_id,
                    2,
                    test_peer(),
                    KaigiFrame::E2EEKeyEpoch(epoch_frame),
                )
                .await
                .expect("e2ee rotation should pass in soak loop");

                let ack = KeyRotationAckFrame {
                    received_at_ms: at_ms + 4,
                    participant_id: "alice@sora".to_string(),
                    ack_epoch: alice_epoch,
                };
                handle_frame(
                    &state,
                    room_id,
                    2,
                    test_peer(),
                    KaigiFrame::KeyRotationAck(ack),
                )
                .await
                .expect("key rotation ack should pass in soak loop");
            }

            if tick % 120 == 0 {
                let start = RecordingNoticeFrame {
                    at_ms: at_ms + 5,
                    participant_id: "host@sora".to_string(),
                    state: RecordingState::Started,
                    local_recording: true,
                    policy_basis: Some("host-allowed".to_string()),
                    issued_by: "host@sora".to_string(),
                };
                handle_frame(
                    &state,
                    room_id,
                    1,
                    test_peer(),
                    KaigiFrame::RecordingNotice(start),
                )
                .await
                .expect("recording start should pass in soak loop");

                let stop = RecordingNoticeFrame {
                    at_ms: at_ms + 6,
                    participant_id: "host@sora".to_string(),
                    state: RecordingState::Stopped,
                    local_recording: true,
                    policy_basis: Some("host-stopped".to_string()),
                    issued_by: "host@sora".to_string(),
                };
                handle_frame(
                    &state,
                    room_id,
                    1,
                    test_peer(),
                    KaigiFrame::RecordingNotice(stop),
                )
                .await
                .expect("recording stop should pass in soak loop");
            }

            drain_all_messages(&mut receivers);
        }

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.participants.len(), 2);
        assert_eq!(room.host_conn_id, Some(1));
        assert_eq!(room.host_role_owner.as_deref(), Some("host@sora"));
        assert!(!room.anonymous_mode);
        assert_eq!(room.e2ee_epochs.get(&1), Some(&1));
        assert_eq!(room.e2ee_epochs.get(&2), Some(&alice_epoch));
        assert!(
            room.participants
                .values()
                .all(|participant| participant.state.hello_seen
                    && !participant.state.waiting_room_pending)
        );
    }

    #[tokio::test]
    async fn moderation_signed_handle_frame_rejects_issued_by_mismatch() {
        let room_id = [8u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.mic_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let signed = ModerationSignedFrame {
            sent_at_ms: 11,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableMic,
            issued_by: "mallory@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "moderation",
                "mallory@sora|participant:alice@sora|disable_mic|11",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ModerationSigned(signed),
        )
        .await
        .expect("frame handling should not error");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(host_frames.iter().any(|frame| {
            matches!(
                frame,
                KaigiFrame::Error(ErrorFrame { message })
                    if message == "issued_by must match sender participant_id"
            )
        }));
        assert!(alice_frames.is_empty(), "target should not receive frames");

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            room.participants
                .get(&2)
                .expect("alice exists")
                .state
                .mic_enabled
        );
    }

    #[tokio::test]
    async fn moderation_signed_handle_frame_rejects_bad_signature() {
        let room_id = [9u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.mic_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let signed = ModerationSignedFrame {
            sent_at_ms: 12,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableMic,
            issued_by: "host@sora".to_string(),
            signature_hex: "aa".repeat(32),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ModerationSigned(signed),
        )
        .await
        .expect("frame handling should not error");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(host_frames.iter().any(|frame| {
            matches!(
                frame,
                KaigiFrame::Error(ErrorFrame { message })
                    if message == "signature_hex failed moderation_signed verification"
            )
        }));
        assert!(alice_frames.is_empty(), "target should not receive frames");

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            room.participants
                .get(&2)
                .expect("alice exists")
                .state
                .mic_enabled
        );
    }

    #[tokio::test]
    async fn moderation_signed_handle_frame_rejects_replay_sent_at_ms() {
        let room_id = [10u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (mut alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        alice.state.mic_enabled = true;
        alice.state.video_enabled = true;
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let first = ModerationSignedFrame {
            sent_at_ms: 20,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableMic,
            issued_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "moderation",
                "host@sora|participant:alice@sora|disable_mic|20",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ModerationSigned(first.clone()),
        )
        .await
        .expect("first moderation should pass");

        let replay = ModerationSignedFrame {
            sent_at_ms: 20,
            target: ModerationTarget::Participant("alice@sora".to_string()),
            action: ModerationAction::DisableVideo,
            issued_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "moderation",
                "host@sora|participant:alice@sora|disable_video|20",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ModerationSigned(replay),
        )
        .await
        .expect("replay should be handled with error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        let signed_audit_count = host_frames
            .iter()
            .filter(|frame| matches!(frame, KaigiFrame::ModerationSigned(_)))
            .count();
        assert_eq!(signed_audit_count, 1, "only first action should be audited");
        assert!(host_frames.iter().any(|frame| {
            matches!(
                frame,
                KaigiFrame::Error(ErrorFrame { message })
                    if message.contains("replay/stale rejected")
            )
        }));
        assert!(
            !alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::Error(_))),
            "target should not receive sender-side replay errors"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        let alice_state = &room.participants.get(&2).expect("alice exists").state;
        assert!(!alice_state.mic_enabled, "first moderation should apply");
        assert!(
            alice_state.video_enabled,
            "replayed moderation should not mutate video state"
        );
    }

    #[tokio::test]
    async fn role_grant_handle_frame_rejects_replay_issued_at_ms() {
        let room_id = [12u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let first = RoleGrantFrame {
            issued_at_ms: 30,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_grant",
                "host@sora|alice@sora|cohost|30",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(first.clone()),
        )
        .await
        .expect("first role grant should pass");

        let replay = RoleGrantFrame {
            issued_at_ms: 30,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_grant",
                "host@sora|alice@sora|cohost|30",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(replay),
        )
        .await
        .expect("replay should be converted to error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        let grant_audit_count = host_frames
            .iter()
            .filter(|frame| matches!(frame, KaigiFrame::RoleGrant(_)))
            .count();
        assert_eq!(
            grant_audit_count, 1,
            "only first role grant should be audited"
        );
        assert!(contains_error_message(
            &host_frames,
            "replay/stale rejected"
        ));
        assert!(
            !contains_error_message(&alice_frames, "replay/stale rejected"),
            "only sender should receive replay rejection"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(room.co_host_conn_ids.contains(&2));
    }

    #[tokio::test]
    async fn role_grant_handle_frame_rejects_bad_signature() {
        let room_id = [15u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let grant = RoleGrantFrame {
            issued_at_ms: 31,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: "aa".repeat(32),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(grant),
        )
        .await
        .expect("bad signature should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(contains_error_message(
            &host_frames,
            "signature_hex failed role_grant verification"
        ));
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleGrant(_))),
            "invalid role grant should not be audited"
        );
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::ParticipantPresenceDelta(_))),
            "invalid role grant should not emit presence delta"
        );
        assert!(
            alice_frames.is_empty(),
            "target should not receive sender-side signature errors"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(!room.co_host_conn_ids.contains(&2));
        assert!(!room.co_host_role_owners.contains("alice@sora"));
    }

    #[tokio::test]
    async fn role_grant_handle_frame_applies_and_broadcasts_audit() {
        let room_id = [16u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let grant = RoleGrantFrame {
            issued_at_ms: 31,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_grant",
                "host@sora|alice@sora|cohost|31",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleGrant(grant.clone()),
        )
        .await
        .expect("role grant should pass");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleGrant(value) if value == &grant)),
            "host should receive role grant audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        role_changes,
                        ..
                    }) if role_changes == &vec![RoleChangeEntry {
                        participant_id: "alice@sora".to_string(),
                        role: RoleKind::CoHost,
                        granted: true,
                    }]
                )
            }),
            "host should receive role-change presence delta"
        );
        assert!(
            alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleGrant(value) if value == &grant)),
            "target should receive role grant audit frame"
        );
        assert!(
            alice_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        role_changes,
                        ..
                    }) if role_changes == &vec![RoleChangeEntry {
                        participant_id: "alice@sora".to_string(),
                        role: RoleKind::CoHost,
                        granted: true,
                    }]
                )
            }),
            "target should receive role-change presence delta"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(room.co_host_conn_ids.contains(&2));
    }

    #[tokio::test]
    async fn role_revoke_handle_frame_applies_and_broadcasts_audit() {
        let room_id = [17u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.co_host_conn_ids.insert(2);
        let state = test_hub_state_with_room(room_id, room);

        let revoke = RoleRevokeFrame {
            issued_at_ms: 41,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_revoke",
                "host@sora|alice@sora|cohost|41",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleRevoke(revoke.clone()),
        )
        .await
        .expect("role revoke should pass");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleRevoke(value) if value == &revoke)),
            "host should receive role revoke audit frame"
        );
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        role_changes,
                        ..
                    }) if role_changes == &vec![RoleChangeEntry {
                        participant_id: "alice@sora".to_string(),
                        role: RoleKind::CoHost,
                        granted: false,
                    }]
                )
            }),
            "host should receive role-change presence delta"
        );
        assert!(
            alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleRevoke(value) if value == &revoke)),
            "target should receive role revoke audit frame"
        );
        assert!(
            alice_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::ParticipantPresenceDelta(ParticipantPresenceDeltaFrame {
                        role_changes,
                        ..
                    }) if role_changes == &vec![RoleChangeEntry {
                        participant_id: "alice@sora".to_string(),
                        role: RoleKind::CoHost,
                        granted: false,
                    }]
                )
            }),
            "target should receive role-change presence delta"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(!room.co_host_conn_ids.contains(&2));
    }

    #[tokio::test]
    async fn role_revoke_handle_frame_rejects_replay_issued_at_ms() {
        let room_id = [19u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.co_host_conn_ids.insert(2);
        let state = test_hub_state_with_room(room_id, room);

        let first = RoleRevokeFrame {
            issued_at_ms: 51,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_revoke",
                "host@sora|alice@sora|cohost|51",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleRevoke(first.clone()),
        )
        .await
        .expect("first role revoke should pass");

        let replay = RoleRevokeFrame {
            issued_at_ms: 51,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "role_revoke",
                "host@sora|alice@sora|cohost|51",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleRevoke(replay),
        )
        .await
        .expect("replay should be converted to error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        let revoke_audit_count = host_frames
            .iter()
            .filter(|frame| matches!(frame, KaigiFrame::RoleRevoke(_)))
            .count();
        assert_eq!(
            revoke_audit_count, 1,
            "only first role revoke should be audited"
        );
        assert!(contains_error_message(
            &host_frames,
            "replay/stale rejected"
        ));
        assert!(
            !contains_error_message(&alice_frames, "replay/stale rejected"),
            "only sender should receive replay rejection"
        );
    }

    #[tokio::test]
    async fn role_revoke_handle_frame_rejects_bad_signature() {
        let room_id = [20u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.co_host_conn_ids.insert(2);
        room.co_host_role_owners.insert("alice@sora".to_string());
        let state = test_hub_state_with_room(room_id, room);

        let revoke = RoleRevokeFrame {
            issued_at_ms: 61,
            target_participant_id: "alice@sora".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host@sora".to_string(),
            signature_hex: "ff".repeat(32),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::RoleRevoke(revoke),
        )
        .await
        .expect("bad signature should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(contains_error_message(
            &host_frames,
            "signature_hex failed role_revoke verification"
        ));
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RoleRevoke(_))),
            "invalid role revoke should not be audited"
        );
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::ParticipantPresenceDelta(_))),
            "invalid role revoke should not emit presence delta"
        );
        assert!(
            alice_frames.is_empty(),
            "target should not receive sender-side signature errors"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(room.co_host_conn_ids.contains(&2));
        assert!(room.co_host_role_owners.contains("alice@sora"));
    }

    #[tokio::test]
    async fn session_policy_handle_frame_rejects_replay_updated_at_ms() {
        let room_id = [13u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        room.participants.insert(1, host);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let first = SessionPolicyFrame {
            updated_at_ms: 40,
            room_lock: true,
            waiting_room_enabled: true,
            guest_join_allowed: false,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 200,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|true|true|false|false|true|200|1|40",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(first.clone()),
        )
        .await
        .expect("first policy update should pass");

        let replay = SessionPolicyFrame {
            updated_at_ms: 40,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: 300,
            policy_epoch: 2,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|false|false|true|true|true|300|2|40",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(replay),
        )
        .await
        .expect("replay should be converted to error frame");

        let host_frames = drain_frames(&mut host_rx);
        assert!(contains_error_message(
            &host_frames,
            "replay/stale rejected"
        ));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.policy_epoch, 1);
        assert!(room.room_lock);
        assert!(!room.local_recording_allowed);
        assert_eq!(room.max_participants, 200);
    }

    #[tokio::test]
    async fn session_policy_handle_frame_rejects_bad_signature() {
        let room_id = [20u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        room.participants.insert(1, host);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let update = SessionPolicyFrame {
            updated_at_ms: 76,
            room_lock: true,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 220,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: "aa".repeat(32),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(update),
        )
        .await
        .expect("bad signature should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        assert!(contains_error_message(
            &host_frames,
            "signature_hex failed session_policy verification"
        ));
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::SessionPolicy(_))),
            "invalid policy update should not be broadcast"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.policy_epoch, 0);
        assert_eq!(room.policy_updated_at_ms, 0);
        assert!(!room.room_lock);
        assert!(room.local_recording_allowed);
    }

    #[tokio::test]
    async fn session_policy_handle_frame_broadcast_preserves_signed_updated_at_ms() {
        let room_id = [18u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        room.participants.insert(1, host);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let update = SessionPolicyFrame {
            updated_at_ms: 77,
            room_lock: true,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 250,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|true|false|true|false|true|250|1|77",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(update.clone()),
        )
        .await
        .expect("policy update should pass");

        let host_frames = drain_frames(&mut host_rx);
        let Some(broadcast_policy) = host_frames.iter().find_map(|frame| {
            if let KaigiFrame::SessionPolicy(policy) = frame {
                Some(policy.clone())
            } else {
                None
            }
        }) else {
            panic!("expected broadcast SessionPolicy frame");
        };
        assert_eq!(broadcast_policy.updated_at_ms, update.updated_at_ms);
        assert_eq!(broadcast_policy.signature_hex, update.signature_hex);
        assert!(session_policy_signature_is_valid(&broadcast_policy));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.policy_updated_at_ms, 77);
        assert_eq!(room.policy_epoch, 1);
    }

    #[tokio::test]
    async fn permissions_snapshot_handle_frame_rejects_client_injection() {
        let room_id = [48u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let injected = PermissionsSnapshotFrame {
            at_ms: 500,
            participant_id: "host@sora".to_string(),
            host: true,
            co_host: false,
            can_moderate: true,
            can_record_local: true,
            epoch: 9,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::PermissionsSnapshot(injected),
        )
        .await
        .expect("client-injected permissions snapshot should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(contains_error_message(
            &host_frames,
            "permissions snapshots are hub-managed"
        ));
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::PermissionsSnapshot(_))),
            "injected permissions snapshot should never broadcast"
        );
        assert!(
            !alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::PermissionsSnapshot(_))),
            "other participants should not receive injected permissions snapshot"
        );
    }

    #[tokio::test]
    async fn participant_presence_delta_handle_frame_rejects_client_injection() {
        let room_id = [49u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let injected = ParticipantPresenceDeltaFrame {
            at_ms: 600,
            sequence: 7,
            joined: Vec::new(),
            left: Vec::new(),
            role_changes: vec![RoleChangeEntry {
                participant_id: "alice@sora".to_string(),
                role: RoleKind::CoHost,
                granted: true,
            }],
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::ParticipantPresenceDelta(injected),
        )
        .await
        .expect("client-injected presence delta should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(contains_error_message(
            &host_frames,
            "presence deltas are hub-managed"
        ));
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::ParticipantPresenceDelta(_))),
            "injected presence delta should never broadcast"
        );
        assert!(
            !alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::ParticipantPresenceDelta(_))),
            "other participants should not receive injected presence delta"
        );
    }

    #[tokio::test]
    async fn recording_notice_handle_frame_broadcasts_when_policy_allows() {
        let room_id = [24u8; 32];
        let mut room = test_room_state(false);
        room.local_recording_allowed = true;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let notice = RecordingNoticeFrame {
            at_ms: 70,
            participant_id: "alice@sora".to_string(),
            state: RecordingState::Started,
            local_recording: true,
            policy_basis: Some("host-allowed".to_string()),
            issued_by: "alice@sora".to_string(),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::RecordingNotice(notice.clone()),
        )
        .await
        .expect("recording notice should pass");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::RecordingNotice(value) if value == &notice)
            ),
            "host should receive recording notice"
        );
        assert!(
            alice_frames.iter().any(
                |frame| matches!(frame, KaigiFrame::RecordingNotice(value) if value == &notice)
            ),
            "issuer should receive recording notice broadcast"
        );

        let stop = RecordingNoticeFrame {
            at_ms: 72,
            participant_id: "alice@sora".to_string(),
            state: RecordingState::Stopped,
            local_recording: true,
            policy_basis: Some("user-stopped".to_string()),
            issued_by: "alice@sora".to_string(),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::RecordingNotice(stop.clone()),
        )
        .await
        .expect("recording stop notice should pass");
        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RecordingNotice(value) if value == &stop)),
            "host should receive recording stop notice"
        );
    }

    #[tokio::test]
    async fn recording_notice_handle_frame_rejects_start_when_policy_disallows() {
        let room_id = [25u8; 32];
        let mut room = test_room_state(false);
        room.local_recording_allowed = false;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let notice = RecordingNoticeFrame {
            at_ms: 71,
            participant_id: "alice@sora".to_string(),
            state: RecordingState::Started,
            local_recording: true,
            policy_basis: Some("host-blocked".to_string()),
            issued_by: "alice@sora".to_string(),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::RecordingNotice(notice),
        )
        .await
        .expect("policy-blocked recording should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::RecordingNotice(_))),
            "policy-blocked start should not broadcast recording notice"
        );
        assert!(contains_error_message(
            &alice_frames,
            "local recording is disabled by room policy"
        ));
    }

    #[tokio::test]
    async fn media_profile_handle_frame_falls_back_to_sdr_without_hdr_capabilities() {
        let room_id = [32u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let negotiation = MediaProfileNegotiationFrame {
            at_ms: 88,
            participant_id: "alice@sora".to_string(),
            requested_profile: MediaProfileKind::Hdr,
            negotiated_profile: MediaProfileKind::Hdr,
            codec: "av1".to_string(),
            epoch: 1,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::MediaProfileNegotiation(negotiation),
        )
        .await
        .expect("media profile negotiation should not fail");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::MediaProfileNegotiation(MediaProfileNegotiationFrame {
                        participant_id,
                        requested_profile: MediaProfileKind::Hdr,
                        negotiated_profile: MediaProfileKind::Sdr,
                        ..
                    }) if participant_id == "alice@sora"
                )
            }),
            "host should observe SDR fallback when HDR capabilities are insufficient"
        );
        assert!(
            alice_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::MediaProfileNegotiation(MediaProfileNegotiationFrame {
                        participant_id,
                        negotiated_profile: MediaProfileKind::Sdr,
                        ..
                    }) if participant_id == "alice@sora"
                )
            }),
            "sender should observe SDR fallback broadcast"
        );
    }

    #[tokio::test]
    async fn media_profile_handle_frame_preserves_hdr_with_sender_and_remote_support() {
        let room_id = [33u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let host_cap = DeviceCapabilityFrame {
            reported_at_ms: 90,
            participant_id: "host@sora".to_string(),
            codecs: vec!["av1".to_string()],
            hdr_capture: false,
            hdr_render: true,
            max_video_streams: 2,
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::DeviceCapability(host_cap),
        )
        .await
        .expect("host capability should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        let alice_cap = DeviceCapabilityFrame {
            reported_at_ms: 91,
            participant_id: "alice@sora".to_string(),
            codecs: vec!["av1".to_string()],
            hdr_capture: true,
            hdr_render: true,
            max_video_streams: 2,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::DeviceCapability(alice_cap),
        )
        .await
        .expect("alice capability should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        let negotiation = MediaProfileNegotiationFrame {
            at_ms: 92,
            participant_id: "alice@sora".to_string(),
            requested_profile: MediaProfileKind::Hdr,
            negotiated_profile: MediaProfileKind::Hdr,
            codec: "av1".to_string(),
            epoch: 1,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::MediaProfileNegotiation(negotiation),
        )
        .await
        .expect("media profile negotiation should pass");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::MediaProfileNegotiation(MediaProfileNegotiationFrame {
                        participant_id,
                        requested_profile: MediaProfileKind::Hdr,
                        negotiated_profile: MediaProfileKind::Hdr,
                        ..
                    }) if participant_id == "alice@sora"
                )
            }),
            "HDR should be preserved when sender capture and remote render support are present"
        );
    }

    #[tokio::test]
    async fn chat_handle_frame_requires_e2ee_key_epoch_when_policy_enabled() {
        let room_id = [28u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::Chat(ChatFrame {
                sent_at_ms: 90,
                from_participant_id: "alice@sora".to_string(),
                from_display_name: Some("Alice".to_string()),
                text: "before-e2ee".to_string(),
            }),
        )
        .await
        .expect("pre-e2ee chat should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.is_empty(),
            "chat should not broadcast while e2ee key epoch is missing"
        );
        assert!(contains_error_message(
            &alice_frames,
            "e2ee required: publish E2EEKeyEpoch before plaintext control"
        ));

        let epoch = E2EEKeyEpochFrame {
            sent_at_ms: 91,
            participant_id: "alice@sora".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice@sora|1|91"),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(epoch),
        )
        .await
        .expect("e2ee key epoch should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::Chat(ChatFrame {
                sent_at_ms: 92,
                from_participant_id: "alice@sora".to_string(),
                from_display_name: Some("Alice".to_string()),
                text: "after-e2ee".to_string(),
            }),
        )
        .await
        .expect("chat should pass after e2ee key epoch bootstrap");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Chat(ChatFrame {
                        from_participant_id,
                        text,
                        ..
                    }) if from_participant_id == "alice@sora" && text == "after-e2ee"
                )
            }),
            "post-e2ee chat should broadcast"
        );
    }

    #[tokio::test]
    async fn chat_handle_frame_allows_plaintext_after_e2ee_policy_disable() {
        let room_id = [30u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let policy = SessionPolicyFrame {
            updated_at_ms: 95,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: false,
            max_participants: 500,
            policy_epoch: 1,
            updated_by: "host@sora".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host@sora|false|false|true|true|false|500|1|95",
            ),
        };
        handle_frame(
            &state,
            room_id,
            1,
            test_peer(),
            KaigiFrame::SessionPolicy(policy),
        )
        .await
        .expect("host policy update should disable e2ee requirement");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::Chat(ChatFrame {
                sent_at_ms: 96,
                from_participant_id: "alice@sora".to_string(),
                from_display_name: Some("Alice".to_string()),
                text: "plaintext-allowed".to_string(),
            }),
        )
        .await
        .expect("chat should pass with e2ee_required=false");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Chat(ChatFrame {
                        from_participant_id,
                        text,
                        ..
                    }) if from_participant_id == "alice@sora" && text == "plaintext-allowed"
                )
            }),
            "chat should broadcast after e2ee policy disable"
        );
    }

    #[tokio::test]
    async fn participant_state_handle_frame_requires_e2ee_key_epoch_when_policy_enabled() {
        let room_id = [29u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 100,
                mic_enabled: Some(true),
                video_enabled: None,
                screen_share_enabled: None,
            }),
        )
        .await
        .expect("pre-e2ee participant state should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames.is_empty(),
            "state update should not broadcast while e2ee key epoch is missing"
        );
        assert!(contains_error_message(
            &alice_frames,
            "e2ee required: publish E2EEKeyEpoch before plaintext control"
        ));

        let epoch = E2EEKeyEpochFrame {
            sent_at_ms: 101,
            participant_id: "alice@sora".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice@sora|1|101"),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(epoch),
        )
        .await
        .expect("e2ee key epoch should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 102,
                mic_enabled: Some(true),
                video_enabled: None,
                screen_share_enabled: None,
            }),
        )
        .await
        .expect("state update should pass after e2ee key epoch bootstrap");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "alice@sora" && snapshot.mic_enabled
                )
            }),
            "post-e2ee participant state should broadcast"
        );
    }

    #[tokio::test]
    async fn participant_state_screen_share_respects_max_screen_shares_limit() {
        let room_id = [34u8; 32];
        let mut room = test_room_state(false);
        room.e2ee_required = false;
        room.max_screen_shares = 1;
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        let (bob, mut bob_rx) = test_joined_participant_with_rx("bob@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.participants.insert(3, bob);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 200,
                mic_enabled: None,
                video_enabled: None,
                screen_share_enabled: Some(true),
            }),
        )
        .await
        .expect("first screen share should pass");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);
        let _ = drain_frames(&mut bob_rx);

        handle_frame(
            &state,
            room_id,
            3,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 201,
                mic_enabled: None,
                video_enabled: None,
                screen_share_enabled: Some(true),
            }),
        )
        .await
        .expect("concurrent screen share should return sender error frame");

        let bob_frames = drain_frames(&mut bob_rx);
        assert!(contains_error_message(
            &bob_frames,
            "screen share denied: max_screen_shares=1 already in use"
        ));
        {
            let rooms = state.rooms.lock().await;
            let room = rooms.get(&room_id).expect("room exists");
            let alice_state = &room.participants.get(&2).expect("alice exists").state;
            let bob_state = &room.participants.get(&3).expect("bob exists").state;
            assert!(alice_state.screen_share_enabled);
            assert!(!bob_state.screen_share_enabled);
        }
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);

        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 202,
                mic_enabled: None,
                video_enabled: None,
                screen_share_enabled: Some(false),
            }),
        )
        .await
        .expect("alice should stop share");
        let _ = drain_frames(&mut host_rx);
        let _ = drain_frames(&mut alice_rx);
        let _ = drain_frames(&mut bob_rx);

        handle_frame(
            &state,
            room_id,
            3,
            test_peer(),
            KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: 203,
                mic_enabled: None,
                video_enabled: None,
                screen_share_enabled: Some(true),
            }),
        )
        .await
        .expect("second participant should share after slot release");

        let host_frames = drain_frames(&mut host_rx);
        assert!(
            host_frames.iter().any(|frame| {
                matches!(
                    frame,
                    KaigiFrame::Event(RoomEventFrame::StateUpdated(snapshot))
                        if snapshot.participant_id == "bob@sora" && snapshot.screen_share_enabled
                )
            }),
            "host should see bob screen share enabled after slot release"
        );
        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert!(
            room.participants
                .get(&3)
                .expect("bob exists")
                .state
                .screen_share_enabled
        );
    }

    #[tokio::test]
    async fn e2ee_key_epoch_handle_frame_rejects_replay_sent_at_ms() {
        let room_id = [14u8; 32];
        let mut room = test_room_state(false);
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(2, alice);
        let state = test_hub_state_with_room(room_id, room);

        let first = E2EEKeyEpochFrame {
            sent_at_ms: 50,
            participant_id: "alice@sora".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice@sora|1|50"),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(first.clone()),
        )
        .await
        .expect("first e2ee key epoch should pass");

        let replay = E2EEKeyEpochFrame {
            sent_at_ms: 50,
            participant_id: "alice@sora".to_string(),
            epoch: 2,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|2"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice@sora|2|50"),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(replay),
        )
        .await
        .expect("replay should be converted to error frame");

        let alice_frames = drain_frames(&mut alice_rx);
        assert!(contains_error_message(
            &alice_frames,
            "replay/stale rejected"
        ));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.e2ee_epochs.get(&2), Some(&1));
    }

    #[tokio::test]
    async fn e2ee_key_epoch_handle_frame_rejects_bad_signature() {
        let room_id = [43u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        let state = test_hub_state_with_room(room_id, room);

        let bad = E2EEKeyEpochFrame {
            sent_at_ms: 61,
            participant_id: "alice@sora".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice@sora|1"),
            signature_hex: "aa".repeat(32),
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::E2EEKeyEpoch(bad),
        )
        .await
        .expect("bad signature should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::E2EEKeyEpoch(_))),
            "invalid key epoch should not broadcast"
        );
        assert!(contains_error_message(
            &alice_frames,
            "e2ee key epoch signature_hex failed verification"
        ));

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.e2ee_epochs.get(&2), None);
    }

    #[tokio::test]
    async fn key_rotation_ack_handle_frame_broadcasts_when_ack_within_sender_epoch() {
        let room_id = [26u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.e2ee_epochs.insert(2, 3);
        let state = test_hub_state_with_room(room_id, room);

        let ack = KeyRotationAckFrame {
            received_at_ms: 80,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 3,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(ack.clone()),
        )
        .await
        .expect("valid key rotation ack should pass");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::KeyRotationAck(value) if value == &ack)),
            "host should receive key rotation ack broadcast"
        );
        assert!(
            alice_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::KeyRotationAck(value) if value == &ack)),
            "sender should receive key rotation ack broadcast"
        );
    }

    #[tokio::test]
    async fn key_rotation_ack_handle_frame_rejects_ack_above_sender_epoch() {
        let room_id = [27u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.e2ee_epochs.insert(2, 2);
        let state = test_hub_state_with_room(room_id, room);

        let ack = KeyRotationAckFrame {
            received_at_ms: 81,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 3,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(ack),
        )
        .await
        .expect("invalid key rotation ack should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        assert!(
            !host_frames
                .iter()
                .any(|frame| matches!(frame, KaigiFrame::KeyRotationAck(_))),
            "invalid ack should not broadcast to room"
        );
        assert!(contains_error_message(
            &alice_frames,
            "ack_epoch exceeds sender key epoch"
        ));
    }

    #[tokio::test]
    async fn key_rotation_ack_handle_frame_rejects_replay_or_stale_ack_epoch() {
        let room_id = [28u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.e2ee_epochs.insert(2, 3);
        let state = test_hub_state_with_room(room_id, room);

        let first = KeyRotationAckFrame {
            received_at_ms: 82,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 2,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(first.clone()),
        )
        .await
        .expect("first key rotation ack should pass");

        let replay = KeyRotationAckFrame {
            received_at_ms: 83,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 2,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(replay),
        )
        .await
        .expect("replayed ack epoch should return sender error frame");

        let stale = KeyRotationAckFrame {
            received_at_ms: 84,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 1,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(stale),
        )
        .await
        .expect("stale ack epoch should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        let ack_broadcast_count = host_frames
            .iter()
            .filter(|frame| matches!(frame, KaigiFrame::KeyRotationAck(_)))
            .count();
        assert_eq!(
            ack_broadcast_count, 1,
            "only first key rotation ack should broadcast"
        );
        assert!(
            contains_error_message(&alice_frames, "ack_epoch must increase"),
            "sender should receive stale/replay ack epoch rejection"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.key_rotation_ack_epochs.get(&2), Some(&2));
    }

    #[tokio::test]
    async fn key_rotation_ack_handle_frame_rejects_replay_received_at_ms() {
        let room_id = [29u8; 32];
        let mut room = test_room_state(false);
        let (host, mut host_rx) = test_joined_participant_with_rx("host@sora");
        let (alice, mut alice_rx) = test_joined_participant_with_rx("alice@sora");
        room.participants.insert(1, host);
        room.participants.insert(2, alice);
        room.host_conn_id = Some(1);
        room.e2ee_epochs.insert(2, 3);
        let state = test_hub_state_with_room(room_id, room);

        let first = KeyRotationAckFrame {
            received_at_ms: 90,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 1,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(first.clone()),
        )
        .await
        .expect("first key rotation ack should pass");

        let replay_clock = KeyRotationAckFrame {
            received_at_ms: 90,
            participant_id: "alice@sora".to_string(),
            ack_epoch: 2,
        };
        handle_frame(
            &state,
            room_id,
            2,
            test_peer(),
            KaigiFrame::KeyRotationAck(replay_clock),
        )
        .await
        .expect("non-monotonic received_at_ms should return sender error frame");

        let host_frames = drain_frames(&mut host_rx);
        let alice_frames = drain_frames(&mut alice_rx);
        let ack_broadcast_count = host_frames
            .iter()
            .filter(|frame| matches!(frame, KaigiFrame::KeyRotationAck(_)))
            .count();
        assert_eq!(
            ack_broadcast_count, 1,
            "non-monotonic ack timestamp should not broadcast"
        );
        assert!(
            contains_error_message(&alice_frames, "replay/stale rejected"),
            "sender should receive replay/stale clock rejection"
        );

        let rooms = state.rooms.lock().await;
        let room = rooms.get(&room_id).expect("room exists");
        assert_eq!(room.key_rotation_ack_epochs.get(&2), Some(&1));
    }

    #[test]
    fn role_grant_promotes_cohost_and_revoke_drops_it() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.host_conn_id = Some(1);

        let grant = RoleGrantFrame {
            issued_at_ms: 1,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_grant", "host|alice|cohost|1"),
        };
        let outcome = apply_role_grant(&mut room, 1, &grant).expect("grant should pass");
        assert!(!outcome.host_changed);
        assert!(outcome.permissions_changed);
        assert!(room.co_host_conn_ids.contains(&2));

        let snapshot = permissions_snapshot_for_conn(&room, 2, 42).expect("snapshot");
        assert!(snapshot.co_host);
        assert!(snapshot.can_moderate);

        let revoke = RoleRevokeFrame {
            issued_at_ms: 2,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_revoke", "host|alice|cohost|2"),
        };
        let outcome = apply_role_revoke(&mut room, 1, &revoke).expect("revoke should pass");
        assert!(!outcome.host_changed);
        assert!(outcome.permissions_changed);
        assert!(!room.co_host_conn_ids.contains(&2));
    }

    #[test]
    fn role_grant_rejects_non_host_sender() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.host_conn_id = Some(1);

        let grant = RoleGrantFrame {
            issued_at_ms: 1,
            target_participant_id: "host".to_string(),
            role: RoleKind::CoHost,
            granted_by: "alice".to_string(),
            signature_hex: deterministic_signature_hex("role_grant", "alice|host|cohost|1"),
        };
        let err = apply_role_grant(&mut room, 2, &grant).expect_err("must reject");
        assert_eq!(err, "host only");
    }

    #[test]
    fn role_grant_rejects_bad_signature() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.host_conn_id = Some(1);

        let grant = RoleGrantFrame {
            issued_at_ms: 1,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host".to_string(),
            signature_hex: "ab".repeat(32),
        };
        let err = apply_role_grant(&mut room, 1, &grant).expect_err("must reject");
        assert_eq!(err, "signature_hex failed role_grant verification");
    }

    #[test]
    fn role_grant_rejects_replay_and_stale_issued_at() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.participants.insert(3, test_joined_participant("bob"));
        room.host_conn_id = Some(1);

        let first = RoleGrantFrame {
            issued_at_ms: 2,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_grant", "host|alice|cohost|2"),
        };
        apply_role_grant(&mut room, 1, &first).expect("first grant should pass");

        let replay = apply_role_grant(&mut room, 1, &first).expect_err("replay must fail");
        assert!(replay.contains("replay/stale rejected"));

        let stale = RoleGrantFrame {
            issued_at_ms: 1,
            target_participant_id: "bob".to_string(),
            role: RoleKind::CoHost,
            granted_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_grant", "host|bob|cohost|1"),
        };
        let stale_err = apply_role_grant(&mut room, 1, &stale).expect_err("stale must fail");
        assert!(stale_err.contains("must increase"));
    }

    #[test]
    fn role_revoke_rejects_bad_signature() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.host_conn_id = Some(1);
        room.co_host_conn_ids.insert(2);

        let revoke = RoleRevokeFrame {
            issued_at_ms: 2,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host".to_string(),
            signature_hex: "00".repeat(32),
        };
        let err = apply_role_revoke(&mut room, 1, &revoke).expect_err("must reject");
        assert_eq!(err, "signature_hex failed role_revoke verification");
    }

    #[test]
    fn role_revoke_rejects_replay_and_stale_issued_at() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.participants
            .insert(2, test_joined_participant("alice"));
        room.participants.insert(3, test_joined_participant("bob"));
        room.host_conn_id = Some(1);
        room.co_host_conn_ids.insert(2);

        let first = RoleRevokeFrame {
            issued_at_ms: 2,
            target_participant_id: "alice".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_revoke", "host|alice|cohost|2"),
        };
        apply_role_revoke(&mut room, 1, &first).expect("first revoke should pass");

        let replay = apply_role_revoke(&mut room, 1, &first).expect_err("replay must fail");
        assert!(replay.contains("replay/stale rejected"));

        let stale = RoleRevokeFrame {
            issued_at_ms: 1,
            target_participant_id: "bob".to_string(),
            role: RoleKind::CoHost,
            revoked_by: "host".to_string(),
            signature_hex: deterministic_signature_hex("role_revoke", "host|bob|cohost|1"),
        };
        let stale_err = apply_role_revoke(&mut room, 1, &stale).expect_err("stale must fail");
        assert!(stale_err.contains("must increase"));
    }

    #[test]
    fn session_policy_update_requires_strict_epoch_increase() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);
        room.policy_epoch = 2;

        let stale = SessionPolicyFrame {
            updated_at_ms: 10,
            room_lock: true,
            waiting_room_enabled: true,
            guest_join_allowed: false,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 250,
            policy_epoch: 2,
            updated_by: "host".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host|true|true|false|false|true|250|2|10",
            ),
        };
        let err = apply_session_policy_update(&mut room, 1, &stale).expect_err("stale");
        assert!(err.contains("policy_epoch must increase"));

        let next = SessionPolicyFrame {
            policy_epoch: 3,
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host|true|true|false|false|true|250|3|10",
            ),
            ..stale
        };
        apply_session_policy_update(&mut room, 1, &next).expect("epoch+1 should pass");
        assert!(room.room_lock);
        assert_eq!(room.policy_epoch, 3);
        assert_eq!(room.max_participants, 250);
    }

    #[test]
    fn session_policy_update_rejects_bad_signature() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);

        let update = SessionPolicyFrame {
            updated_at_ms: 10,
            room_lock: true,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: 200,
            policy_epoch: 1,
            updated_by: "host".to_string(),
            signature_hex: "aa".repeat(32),
        };
        let err = apply_session_policy_update(&mut room, 1, &update).expect_err("must reject");
        assert_eq!(err, "signature_hex failed session_policy verification");
    }

    #[test]
    fn session_policy_update_rejects_replay_updated_at_ms() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);

        let first = SessionPolicyFrame {
            updated_at_ms: 10,
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: 500,
            policy_epoch: 1,
            updated_by: "host".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host|false|false|true|true|true|500|1|10",
            ),
        };
        apply_session_policy_update(&mut room, 1, &first).expect("first update should pass");

        let replay = SessionPolicyFrame {
            updated_at_ms: 10,
            room_lock: true,
            waiting_room_enabled: true,
            guest_join_allowed: false,
            local_recording_allowed: false,
            e2ee_required: true,
            max_participants: 400,
            policy_epoch: 2,
            updated_by: "host".to_string(),
            signature_hex: deterministic_signature_hex(
                "session_policy",
                "host|true|true|false|false|true|400|2|10",
            ),
        };
        let err = apply_session_policy_update(&mut room, 1, &replay).expect_err("replay must fail");
        assert!(err.contains("replay/stale rejected"));
    }

    #[test]
    fn join_policy_rejects_locked_or_full_room() {
        let mut room = test_room_state(false);
        room.participants.insert(1, test_joined_participant("host"));
        room.host_conn_id = Some(1);
        room.room_lock = true;
        room.max_participants = 2;

        room.participants.insert(2, test_pending_participant());
        assert_eq!(
            validate_join_allowed(&room, 2, "guest-2"),
            Some("room is locked".to_string())
        );

        room.room_lock = false;
        room.participants.insert(3, test_joined_participant("bob"));
        room.participants.insert(4, test_pending_participant());
        assert_eq!(
            validate_join_allowed(&room, 4, "guest-4"),
            Some("room is full: max_participants=2".to_string())
        );
    }

    #[test]
    fn join_policy_rejects_guest_when_guest_policy_disabled() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.host_conn_id = Some(1);
        room.guest_join_allowed = false;
        room.participants.insert(2, test_pending_participant());

        assert_eq!(
            validate_join_allowed(&room, 2, "guest-user"),
            Some("guest participants are not allowed by room policy".to_string())
        );
        assert_eq!(validate_join_allowed(&room, 2, "alice@sora"), None);
    }

    #[test]
    fn join_policy_rejects_duplicate_participant_id() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.participants
            .insert(2, test_joined_participant("alice@sora"));
        room.host_conn_id = Some(1);
        room.participants.insert(3, test_pending_participant());

        assert_eq!(
            validate_join_allowed(&room, 3, "alice@sora"),
            Some("participant_id already in use: alice@sora".to_string())
        );

        let mut waiting = test_pending_participant();
        waiting.state.participant_id = "guest-user".to_string();
        waiting.state.waiting_room_pending = true;
        room.participants.insert(4, waiting);
        room.participants.insert(5, test_pending_participant());
        assert_eq!(
            validate_join_allowed(&room, 5, "guest-user"),
            Some("participant_id already in use: guest-user".to_string())
        );
    }

    #[test]
    fn join_policy_rehello_rejects_participant_id_change() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.participants
            .insert(2, test_joined_participant("alice@sora"));
        room.host_conn_id = Some(1);

        assert_eq!(
            validate_join_allowed(&room, 2, "bob@sora"),
            Some("participant_id cannot change after Hello".to_string())
        );
        assert_eq!(validate_join_allowed(&room, 2, "alice@sora"), None);
    }

    #[test]
    fn guest_classification_matches_account_id_heuristic() {
        assert!(participant_id_is_guest("alice"));
        assert!(!participant_id_is_guest("alice@sora"));
    }

    #[test]
    fn admit_waiting_participant_transitions_to_joined_state() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.host_conn_id = Some(1);

        let mut pending = test_pending_participant();
        pending.state.participant_id = "guest-user".to_string();
        pending.state.waiting_room_pending = true;
        room.participants.insert(2, pending);

        let outcome = admit_waiting_participant(&mut room, 2).expect("admit should succeed");
        assert_eq!(outcome.conn_id, 2);
        assert_eq!(outcome.snapshot.participant_id, "guest-user");
        assert_eq!(outcome.presence_delta.joined.len(), 1);
        assert_eq!(outcome.presence_delta.left.len(), 0);
        assert!(
            room.participants
                .get(&2)
                .expect("participant should remain")
                .state
                .hello_seen
        );
        assert!(
            !room
                .participants
                .get(&2)
                .expect("participant should remain")
                .state
                .waiting_room_pending
        );
        assert_eq!(room.e2ee_epochs.get(&2), Some(&0));
    }

    #[test]
    fn deny_waiting_participant_removes_pending_participant() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.host_conn_id = Some(1);

        let mut pending = test_pending_participant();
        pending.state.participant_id = "guest-user".to_string();
        pending.state.waiting_room_pending = true;
        room.participants.insert(2, pending);

        let denied_id = deny_waiting_participant(&mut room, 2).expect("deny should succeed");
        assert_eq!(denied_id, "guest-user");
        assert!(!room.participants.contains_key(&2));
        assert_eq!(room.host_conn_id, Some(1));
    }

    #[test]
    fn waiting_room_actions_reject_non_pending_targets() {
        let mut room = test_room_state(false);
        room.participants
            .insert(1, test_joined_participant("host@sora"));
        room.host_conn_id = Some(1);
        room.participants
            .insert(2, test_joined_participant("member@sora"));

        let admit_err =
            admit_waiting_participant(&mut room, 2).expect_err("admit must reject non-pending");
        assert!(admit_err.contains("is not waiting"));

        let deny_err =
            deny_waiting_participant(&mut room, 2).expect_err("deny must reject non-pending");
        assert!(deny_err.contains("is not waiting"));
    }

    #[test]
    fn device_capability_validation_requires_codecs_and_streams() {
        let mut cap = DeviceCapabilityFrame {
            reported_at_ms: 1,
            participant_id: "alice".to_string(),
            codecs: vec!["av1".to_string()],
            hdr_capture: true,
            hdr_render: true,
            max_video_streams: 2,
        };
        assert_eq!(validate_device_capability_frame(&cap), None);

        cap.codecs.clear();
        assert_eq!(
            validate_device_capability_frame(&cap),
            Some("device capability codecs must be non-empty".to_string())
        );

        cap.codecs = vec!["av1".to_string()];
        cap.max_video_streams = 0;
        assert_eq!(
            validate_device_capability_frame(&cap),
            Some("device capability max_video_streams must be >= 1".to_string())
        );
    }

    #[test]
    fn media_profile_validation_requires_epoch_and_codec() {
        let mut profile = MediaProfileNegotiationFrame {
            at_ms: 1,
            participant_id: "alice".to_string(),
            requested_profile: kaigi_wire::MediaProfileKind::Hdr,
            negotiated_profile: kaigi_wire::MediaProfileKind::Sdr,
            codec: "av1".to_string(),
            epoch: 1,
        };
        assert_eq!(validate_media_profile_negotiation_frame(&profile), None);

        profile.codec = String::new();
        assert_eq!(
            validate_media_profile_negotiation_frame(&profile),
            Some("media profile codec must be non-empty".to_string())
        );

        profile.codec = "av1".to_string();
        profile.epoch = 0;
        assert_eq!(
            validate_media_profile_negotiation_frame(&profile),
            Some("media profile epoch must be >= 1".to_string())
        );
    }

    #[test]
    fn recording_notice_validation_requires_identity_fields() {
        let mut notice = RecordingNoticeFrame {
            at_ms: 1,
            participant_id: "alice".to_string(),
            state: RecordingState::Started,
            local_recording: true,
            policy_basis: Some("host-allowed".to_string()),
            issued_by: "alice".to_string(),
        };
        assert_eq!(validate_recording_notice_frame(&notice), None);
        notice.participant_id = String::new();
        assert_eq!(
            validate_recording_notice_frame(&notice),
            Some("recording notice participant_id must be non-empty".to_string())
        );
    }

    #[test]
    fn e2ee_key_epoch_validation_checks_hex_and_epoch() {
        let mut key_epoch = E2EEKeyEpochFrame {
            sent_at_ms: 1,
            participant_id: "alice".to_string(),
            epoch: 1,
            public_key_hex: deterministic_signature_hex("e2ee_public_key", "alice|1"),
            signature_hex: deterministic_signature_hex("e2ee_key_epoch", "alice|1|1"),
        };
        assert_eq!(validate_e2ee_key_epoch_frame(&key_epoch), None);

        key_epoch.epoch = 0;
        assert_eq!(
            validate_e2ee_key_epoch_frame(&key_epoch),
            Some("e2ee key epoch must be >= 1".to_string())
        );

        key_epoch.epoch = 1;
        key_epoch.signature_hex = "zz".to_string();
        assert_eq!(
            validate_e2ee_key_epoch_frame(&key_epoch),
            Some("e2ee key epoch signature_hex must be 32-byte hex".to_string())
        );

        key_epoch.signature_hex = deterministic_signature_hex("e2ee_key_epoch", "alice|1|1");
        key_epoch.public_key_hex = "11".repeat(32);
        assert_eq!(
            validate_e2ee_key_epoch_frame(&key_epoch),
            Some("e2ee key epoch public_key_hex failed verification".to_string())
        );
    }

    #[test]
    fn anon_capacity_normalization_clamps_minimum() {
        assert_eq!(normalize_anon_max_participants(0), 1);
        assert_eq!(normalize_anon_max_participants(42), 42);
    }

    #[test]
    fn anon_capacity_warn_threshold_only_warns_above_limit() {
        assert!(!should_warn_high_anon_capacity(
            WARN_ANON_MAX_PARTICIPANTS_THRESHOLD
        ));
        assert!(should_warn_high_anon_capacity(
            WARN_ANON_MAX_PARTICIPANTS_THRESHOLD + 1
        ));
    }

    #[test]
    fn escrow_stale_warn_threshold_only_warns_above_limit() {
        assert!(!should_warn_high_anon_escrow_stale_secs(
            WARN_ANON_ESCROW_PROOF_STALE_SECS_THRESHOLD
        ));
        assert!(should_warn_high_anon_escrow_stale_secs(
            WARN_ANON_ESCROW_PROOF_STALE_SECS_THRESHOLD + 1
        ));
    }

    #[test]
    fn escrow_stale_disabled_only_for_zero() {
        assert!(is_anon_escrow_stale_enforcement_disabled(0));
        assert!(!is_anon_escrow_stale_enforcement_disabled(1));
    }

    #[test]
    fn anon_hello_key_rotation_requires_group_key_update() {
        let mut state = ParticipantState {
            participant_id: "p".to_string(),
            display_name: None,
            participant_handle: Some("anon-a".to_string()),
            x25519_pubkey_hex: Some("11".repeat(32)),
            x25519_epoch: 3,
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            anonymous_mode: true,
            waiting_room_pending: false,
            hello_seen: true,
            last_billed_at_ms: 0,
            billed_nano_xor: 0,
            billing_remainder_mod_60k: 0,
            paid_nano_xor: 0,
            last_payment_at_ms: None,
            last_escrow_proof_at_ms: None,
            escrow_id: None,
        };

        let hello = AnonHelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_handle: "anon-a".to_string(),
            x25519_pubkey_hex: "22".repeat(32),
        };
        let err = apply_anon_hello_state(&mut state, &hello, 10).expect_err("must reject");
        assert!(err.contains("GroupKeyUpdate"));
    }

    #[test]
    fn anon_hello_does_not_count_as_escrow_proof() {
        let mut state = test_pending_participant().state;
        let hello = AnonHelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_handle: "anon-a".to_string(),
            x25519_pubkey_hex: "11".repeat(32),
        };
        apply_anon_hello_state(&mut state, &hello, 10).expect("anon hello should apply");
        assert_eq!(state.last_billed_at_ms, 10);
        assert_eq!(state.last_escrow_proof_at_ms, None);
    }

    #[test]
    fn anon_rehello_preserves_escrow_timing_cursor() {
        let mut state = test_participant_with_handle(Some("anon-a"), true).state;
        state.x25519_pubkey_hex = Some("11".repeat(32));
        state.x25519_epoch = 2;
        state.last_billed_at_ms = 50;
        state.last_escrow_proof_at_ms = Some(70);

        let hello = AnonHelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_handle: "anon-a".to_string(),
            x25519_pubkey_hex: "11".repeat(32),
        };
        apply_anon_hello_state(&mut state, &hello, 999).expect("rehello should apply");
        assert_eq!(state.last_billed_at_ms, 50);
        assert_eq!(state.last_escrow_proof_at_ms, Some(70));
    }

    #[test]
    fn group_key_update_policy_rejects_stale_and_same_epoch_rotation() {
        let stale = should_apply_group_key_update(5, Some(&"11".repeat(32)), 4, &"22".repeat(32))
            .expect_err("stale epoch should be rejected");
        assert!(stale.contains("stale"));

        let same_epoch_rotation =
            should_apply_group_key_update(5, Some(&"11".repeat(32)), 5, &"22".repeat(32))
                .expect_err("same epoch key change should be rejected");
        assert!(same_epoch_rotation.contains("strictly increasing"));
    }

    #[test]
    fn group_key_update_policy_allows_idempotent_and_newer_updates() {
        assert!(
            !should_apply_group_key_update(5, Some(&"11".repeat(32)), 5, &"11".repeat(32))
                .expect("same epoch + same key should be noop")
        );
        assert!(
            should_apply_group_key_update(5, Some(&"11".repeat(32)), 6, &"22".repeat(32))
                .expect("newer epoch should apply")
        );
    }

    #[test]
    fn group_key_update_validation_rejects_empty_handle() {
        let frame = test_group_key_update(" ", &"11".repeat(32), 1);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some("group key participant_handle must be non-empty".to_string())
        );
    }

    #[test]
    fn group_key_update_validation_rejects_whitespace_handle() {
        let frame = test_group_key_update("anon a", &"11".repeat(32), 1);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some(
                "group key participant_handle must not contain whitespace/control chars"
                    .to_string()
            )
        );
    }

    #[test]
    fn group_key_update_validation_rejects_non_ascii_handle() {
        let frame = test_group_key_update("匿名", &"11".repeat(32), 1);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some("group key participant_handle must be ASCII".to_string())
        );
    }

    #[test]
    fn group_key_update_validation_rejects_account_like_handle() {
        let frame = test_group_key_update("alice@sora", &"11".repeat(32), 1);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some("group key participant_handle must not contain '@'".to_string())
        );
    }

    #[test]
    fn group_key_update_validation_rejects_oversized_handle() {
        let frame = test_group_key_update(
            &"a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1),
            &"11".repeat(32),
            1,
        );
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some(format!(
                "group key participant_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ))
        );
    }

    #[test]
    fn group_key_update_validation_rejects_invalid_pubkey() {
        let frame = test_group_key_update("anon-a", "zz", 1);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some("x25519_pubkey_hex must be 32-byte hex".to_string())
        );
    }

    #[test]
    fn group_key_update_validation_rejects_zero_epoch() {
        let frame = test_group_key_update("anon-a", &"11".repeat(32), 0);
        assert_eq!(
            validate_group_key_update_frame(&frame),
            Some("group key epoch must be >= 1".to_string())
        );
    }

    #[test]
    fn group_key_update_validation_accepts_well_formed_frame() {
        let frame = test_group_key_update("anon-a", &"11".repeat(32), 1);
        assert_eq!(validate_group_key_update_frame(&frame), None);
    }

    #[test]
    fn encrypted_control_validation_rejects_empty_sender() {
        let frame = test_encrypted_control(" ");
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted sender handle must be non-empty".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_whitespace_sender() {
        let frame = test_encrypted_control("anon a");
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted sender handle must not contain whitespace/control chars".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_non_ascii_sender() {
        let frame = test_encrypted_control("匿名");
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted sender handle must be ASCII".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_account_like_sender() {
        let frame = test_encrypted_control("alice@sora");
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted sender handle must not contain '@'".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_oversized_sender() {
        let frame = test_encrypted_control(&"a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1));
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(format!(
                "encrypted sender handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ))
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_zero_epoch() {
        let mut frame = test_encrypted_control("anon-a");
        frame.epoch = 0;
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted control epoch must be >= 1".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_oversized_recipient() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].recipient_handle = "a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1);
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(format!(
                "encrypted recipient handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ))
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_whitespace_recipient() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].recipient_handle = "anon b".to_string();
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(
                "encrypted recipient handle must not contain whitespace/control chars".to_string()
            )
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_non_ascii_recipient() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].recipient_handle = "匿名".to_string();
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted recipient handle must be ASCII".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_account_like_recipient() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].recipient_handle = "alice@sora".to_string();
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted recipient handle must not contain '@'".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_duplicate_recipients() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads.push(test_encrypted_payload("anon-b"));
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted payload recipients must be unique".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_non_hex_ciphertext() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].ciphertext_hex = "zz".to_string();
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some("encrypted ciphertext must be valid hex".to_string())
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_too_short_ciphertext() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].ciphertext_hex = "aa".repeat((MIN_ENCRYPTED_CIPHERTEXT_HEX_LEN / 2) - 1);
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(format!(
                "encrypted ciphertext too short: min {MIN_ENCRYPTED_CIPHERTEXT_HEX_LEN} hex chars"
            ))
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_oversized_ciphertext() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].ciphertext_hex =
            "aa".repeat((MAX_ENCRYPTED_CIPHERTEXT_HEX_LEN / 2).saturating_add(1));
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(format!(
                "encrypted ciphertext too long: max {MAX_ENCRYPTED_CIPHERTEXT_HEX_LEN} hex chars"
            ))
        );
    }

    #[test]
    fn encrypted_control_validation_rejects_too_many_recipients() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads = (0..(MAX_ENCRYPTED_RECIPIENTS_PER_FRAME + 1))
            .map(|i| test_encrypted_payload(&format!("anon-{i:03}")))
            .collect();
        assert_eq!(
            validate_encrypted_control_frame(&frame),
            Some(format!(
                "encrypted payload fanout too large: max {MAX_ENCRYPTED_RECIPIENTS_PER_FRAME} recipients"
            ))
        );
    }

    #[test]
    fn encrypted_control_validation_accepts_max_recipient_cap() {
        let mut frame = test_encrypted_control("anon-a");
        frame.payloads = (0..MAX_ENCRYPTED_RECIPIENTS_PER_FRAME)
            .map(|i| test_encrypted_payload(&format!("anon-{i:03}")))
            .collect();
        assert_eq!(validate_encrypted_control_frame(&frame), None);
    }

    #[test]
    fn encrypted_control_epoch_validation_rejects_mismatch() {
        assert_eq!(
            validate_encrypted_control_epoch(2, 3),
            Some("encrypted control epoch mismatch: expected 3 got 2".to_string())
        );
    }

    #[test]
    fn encrypted_control_epoch_validation_rejects_uninitialized_sender() {
        assert_eq!(
            validate_encrypted_control_epoch(1, 0),
            Some("sender key epoch is not initialized".to_string())
        );
    }

    #[test]
    fn encrypted_control_epoch_validation_accepts_matching_epoch() {
        assert_eq!(validate_encrypted_control_epoch(3, 3), None);
    }

    #[test]
    fn encrypted_control_room_recipients_reject_unknown_handle() {
        let mut room = test_room_state(true);
        room.participants
            .insert(1, test_participant_with_handle(Some("anon-a"), true));
        room.participants
            .insert(2, test_participant_with_handle(Some("anon-b"), true));

        let mut frame = test_encrypted_control("anon-a");
        frame.payloads[0].recipient_handle = "anon-c".to_string();
        assert_eq!(
            validate_encrypted_control_room_recipients(&frame, &room),
            Some("encrypted recipient handle is not in anonymous roster: anon-c".to_string())
        );
    }

    #[test]
    fn encrypted_control_room_recipients_accept_known_handles() {
        let mut room = test_room_state(true);
        room.participants
            .insert(1, test_participant_with_handle(Some("anon-a"), true));
        room.participants
            .insert(2, test_participant_with_handle(Some("anon-b"), true));

        let frame = test_encrypted_control("anon-a");
        assert_eq!(
            validate_encrypted_control_room_recipients(&frame, &room),
            None
        );
    }

    #[test]
    fn encrypted_control_validation_accepts_well_formed_frame() {
        let frame = test_encrypted_control("anon-a");
        assert_eq!(validate_encrypted_control_frame(&frame), None);
    }

    #[test]
    fn escrow_proof_validation_rejects_empty_id() {
        let proof = test_escrow_proof(" ", "");
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("escrow_id must be non-empty".to_string())
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_invalid_payer_handle() {
        let mut proof = test_escrow_proof("escrow-1", &"ab".repeat(16));
        proof.payer_handle = " ".to_string();
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("payer_handle must be non-empty".to_string())
        );

        proof.payer_handle = "alice@sora".to_string();
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("payer_handle must not contain '@'".to_string())
        );

        proof.payer_handle = "anon a".to_string();
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("payer_handle must not contain whitespace/control chars".to_string())
        );

        proof.payer_handle = "匿名".to_string();
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("payer_handle must be ASCII".to_string())
        );

        proof.payer_handle = "a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1);
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some(format!(
                "payer_handle too long: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
            ))
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_whitespace_id() {
        let proof = test_escrow_proof("escrow id", &"ab".repeat(16));
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("escrow_id must not contain whitespace/control chars".to_string())
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_non_ascii_id() {
        let proof = test_escrow_proof("匿名", &"ab".repeat(16));
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("escrow_id must be ASCII".to_string())
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_account_like_id() {
        let proof = test_escrow_proof("escrow@sora", &"ab".repeat(16));
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("escrow_id must not contain '@'".to_string())
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_oversized_id() {
        let proof = test_escrow_proof(&"x".repeat(MAX_ESCROW_ID_LEN + 1), "");
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some(format!("escrow_id too long: max {MAX_ESCROW_ID_LEN} chars"))
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_empty_proof_hex() {
        let proof = test_escrow_proof("escrow-1", "");
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some("proof_hex must be valid hex".to_string())
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_oversized_proof_hex() {
        let proof = test_escrow_proof(
            "escrow-1",
            &"ab".repeat((MAX_ESCROW_PROOF_HEX_LEN / 2).saturating_add(1)),
        );
        assert_eq!(
            validate_escrow_proof_frame(&proof),
            Some(format!(
                "proof_hex too long: max {MAX_ESCROW_PROOF_HEX_LEN} hex chars"
            ))
        );
    }

    #[test]
    fn escrow_proof_validation_rejects_invalid_hex_and_accepts_valid() {
        let bad = test_escrow_proof("escrow-1", "zz");
        assert_eq!(
            validate_escrow_proof_frame(&bad),
            Some("proof_hex must be valid hex".to_string())
        );

        let good = test_escrow_proof("escrow-2", &"ab".repeat(16));
        assert_eq!(validate_escrow_proof_frame(&good), None);
    }

    #[test]
    fn escrow_proof_validation_accepts_uppercase_prefixed_hex() {
        let proof = test_escrow_proof("escrow-2", "0XAB");
        assert_eq!(validate_escrow_proof_frame(&proof), None);
    }

    #[test]
    fn escrow_id_consistency_rejects_session_mismatch() {
        assert_eq!(
            validate_escrow_id_consistency(Some("escrow-a"), "escrow-b"),
            Some("escrow_id must remain stable for a participant session".to_string())
        );
    }

    #[test]
    fn escrow_id_consistency_accepts_first_and_same_id() {
        assert_eq!(validate_escrow_id_consistency(None, "escrow-a"), None);
        assert_eq!(
            validate_escrow_id_consistency(Some("escrow-a"), "escrow-a"),
            None
        );
    }

    #[test]
    fn tx_hash_hex_validation_accepts_32_byte_hex() {
        let valid = "ab".repeat(32);
        assert!(is_valid_tx_hash_hex(&valid));
        let prefixed = format!("0x{valid}");
        assert!(is_valid_tx_hash_hex(&prefixed));
        let prefixed_upper = format!("0X{valid}");
        assert!(is_valid_tx_hash_hex(&prefixed_upper));
    }

    #[test]
    fn tx_hash_hex_validation_rejects_bad_values() {
        assert!(!is_valid_tx_hash_hex("abc"));
        assert!(!is_valid_tx_hash_hex(&"zz".repeat(32)));
    }

    #[test]
    fn hex_len_validation_works_for_x25519_key() {
        assert!(is_valid_hex_len(&"11".repeat(32), 32));
        assert!(is_valid_hex_len(&format!("0X{}", "11".repeat(32)), 32));
        assert!(!is_valid_hex_len("11", 32));
        assert!(!is_valid_hex_len(&"zz".repeat(32), 32));
    }

    #[test]
    fn escrow_stale_detection_respects_threshold() {
        assert!(!escrow_proof_stale(1_000, 1_999, 1));
        assert!(!escrow_proof_stale(1_000, 2_000, 1));
        assert!(escrow_proof_stale(1_000, 2_001, 1));
    }

    #[test]
    fn anonymous_hello_timeout_respects_threshold() {
        assert!(!anonymous_hello_timed_out(1_000, 1_999, 1));
        assert!(!anonymous_hello_timed_out(1_000, 2_000, 1));
        assert!(anonymous_hello_timed_out(1_000, 2_001, 1));
    }

    #[test]
    fn all_target_includes_sender_only_for_kick() {
        assert!(include_sender_in_all_target(&ModerationAction::Kick));
        assert!(!include_sender_in_all_target(&ModerationAction::DisableMic));
        assert!(!include_sender_in_all_target(
            &ModerationAction::DisableVideo
        ));
        assert!(!include_sender_in_all_target(
            &ModerationAction::DisableScreenShare
        ));
    }

    #[test]
    fn rate_policy_clamps_zero_without_free_override() {
        assert_eq!(normalize_rate_for_policy(0, false), 1);
        assert_eq!(normalize_rate_for_policy(10, false), 10);
    }

    #[test]
    fn initial_rate_rejects_zero_without_free_override() {
        let err = normalize_initial_rate(0, false).expect_err("zero rate rejected");
        assert!(err.to_string().contains("--allow-free-calls"));
    }

    #[test]
    fn initial_rate_allows_zero_with_free_override() {
        assert_eq!(
            normalize_initial_rate(0, true).expect("zero rate allowed"),
            0
        );
    }

    #[test]
    fn hub_cli_defaults_zero_anonymous_surcharge() {
        let args = Args::parse_from(["kaigi-hub-echo"]);
        assert_eq!(args.anon_zk_extra_fee_per_minute_nano, 0);
    }

    #[test]
    fn hub_cli_parses_anonymous_surcharge_flag() {
        let args = Args::parse_from([
            "kaigi-hub-echo",
            "--anon-zk-extra-fee-per-minute-nano",
            "42",
        ]);
        assert_eq!(args.anon_zk_extra_fee_per_minute_nano, 42);
    }

    #[test]
    fn anonymous_zk_surcharge_keeps_rate_when_zero() {
        assert_eq!(
            apply_anonymous_zk_surcharge(1_000, 0).expect("zero surcharge should be no-op"),
            1_000
        );
    }

    #[test]
    fn anonymous_zk_surcharge_adds_extra_fee() {
        assert_eq!(
            apply_anonymous_zk_surcharge(1_000, 250).expect("surcharge should apply"),
            1_250
        );
    }

    #[test]
    fn anonymous_zk_surcharge_rejects_overflow() {
        let err = apply_anonymous_zk_surcharge(u64::MAX, 1).expect_err("overflow must fail");
        assert!(err.contains("overflow"));
    }

    #[test]
    fn join_media_defaults_force_all_media_off() {
        let mut hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "p".to_string(),
            display_name: Some("n".to_string()),
            mic_enabled: true,
            video_enabled: true,
            screen_share_enabled: true,
            hdr_display: true,
            hdr_capture: false,
        };

        let forced = enforce_join_media_defaults(&mut hello);
        assert!(forced);
        assert!(!hello.mic_enabled);
        assert!(!hello.video_enabled);
        assert!(!hello.screen_share_enabled);
        assert!(hello.hdr_display);
    }

    #[test]
    fn join_media_defaults_noop_when_already_off() {
        let mut hello = HelloFrame {
            protocol_version: PROTOCOL_VERSION,
            participant_id: "p".to_string(),
            display_name: None,
            mic_enabled: false,
            video_enabled: false,
            screen_share_enabled: false,
            hdr_display: false,
            hdr_capture: true,
        };

        let forced = enforce_join_media_defaults(&mut hello);
        assert!(!forced);
        assert!(!hello.mic_enabled);
        assert!(!hello.video_enabled);
        assert!(!hello.screen_share_enabled);
        assert!(hello.hdr_capture);
    }
}
