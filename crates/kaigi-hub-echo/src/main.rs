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
    AnonHelloFrame, AnonRosterEntry, AnonRosterFrame, ChatFrame, EncryptedControlFrame, ErrorFrame,
    EscrowAckFrame, EscrowProofFrame, FrameDecoder, GroupKeyUpdateFrame, HelloFrame, KaigiFrame,
    MAX_ANON_PARTICIPANT_HANDLE_LEN, MAX_ESCROW_ID_LEN, MAX_ESCROW_PROOF_HEX_LEN, ModerationAction,
    ModerationTarget, PROTOCOL_VERSION, ParticipantLeftFrame, ParticipantSnapshot, PaymentAckFrame,
    RoomConfigFrame, RoomEventFrame, RosterEntry, RosterFrame, encode_framed,
};
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
    host_conn_id: Option<ConnId>,
    rate_per_minute_nano: u64,
    max_screen_shares: u8,
    anonymous_mode: bool,
    anon_admission_rejections: u64,
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
            host_conn_id: None,
            rate_per_minute_nano: default_rate,
            max_screen_shares: 1,
            anonymous_mode: false,
            anon_admission_rejections: 0,
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
    let (left_participant_id, left_participant_handle, remove_room, host_changed, anonymous_mode) = {
        let mut rooms = state.rooms.lock().await;
        let Some(room) = rooms.get_mut(&room_id) else {
            return Ok(());
        };
        let left = room.participants.remove(&conn_id);
        let left_id = left.as_ref().map(|p| p.state.participant_id.clone());
        let left_handle = left
            .as_ref()
            .and_then(|p| p.state.participant_handle.clone());
        let remove_room = room.participants.is_empty();
        let anonymous_mode = room.anonymous_mode;
        if remove_room {
            rooms.remove(&room_id);
            (left_id, left_handle, true, false, anonymous_mode)
        } else {
            let mut host_changed = false;
            if !room.anonymous_mode && room.host_conn_id == Some(conn_id) {
                let new_host = room.participants.keys().min().copied();
                room.host_conn_id = new_host;
                host_changed = true;
            }
            (left_id, left_handle, false, host_changed, anonymous_mode)
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
    } else if let Some(participant_id) = left_participant_id {
        let at_ms = now_ms();
        let event = KaigiFrame::Event(RoomEventFrame::Left(ParticipantLeftFrame {
            at_ms,
            participant_id,
        }));
        broadcast_frame(state, &room_id, &event).await?;
    }

    if host_changed && !anonymous_mode {
        let cfg = room_config_frame(state, room_id).await?;
        broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;
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
            let (roster, update, mode_error, surcharge_applied) = {
                let mut rooms = state.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else {
                    return Ok(());
                };
                if let Some(handle_error) =
                    validate_anon_participant_handle(room, conn_id, &hello.participant_handle)
                {
                    (None, None, Some(handle_error), None)
                } else if let Some(cap_error) =
                    validate_anonymous_room_capacity(room, conn_id, state.anon_max_participants)
                {
                    anon_cap_rejection_count = Some(record_anon_admission_rejection(room));
                    (None, None, Some(cap_error), None)
                } else if !room.anonymous_mode {
                    let transparent_hello_seen = room
                        .participants
                        .values()
                        .any(|p| p.state.hello_seen && !p.state.anonymous_mode);
                    if transparent_hello_seen {
                        (
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
                            Err(err) => (None, None, Some(err), None),
                            Ok(effective_rate) => {
                                let Some(participant) = room.participants.get_mut(&conn_id) else {
                                    return Ok(());
                                };
                                let now = now_ms();
                                if let Err(err) =
                                    apply_anon_hello_state(&mut participant.state, &hello, now)
                                {
                                    (None, None, Some(err), None)
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
                                    (
                                        Some(roster),
                                        Some(update),
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
                        (None, None, Some(err), None)
                    } else {
                        let epoch = participant.state.x25519_epoch;
                        let roster = anon_roster_frame_locked(room);
                        let update = GroupKeyUpdateFrame {
                            sent_at_ms: now,
                            participant_handle: hello.participant_handle.clone(),
                            x25519_pubkey_hex: hello.x25519_pubkey_hex.clone(),
                            epoch,
                        };
                        (Some(roster), Some(update), None, None)
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

            let (snapshot, mode_conflict) = {
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
                    )
                } else {
                    if room.host_conn_id.is_none() {
                        room.host_conn_id = Some(conn_id);
                    }
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
                    participant.state.hello_seen = true;
                    participant.state.last_escrow_proof_at_ms = None;
                    participant.state.escrow_id = None;
                    (
                        participant_snapshot_from_state(&participant.state, now_ms()),
                        false,
                    )
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

            // Broadcast room config (host, rate, etc).
            let cfg = room_config_frame(state, room_id).await?;
            broadcast_frame(state, &room_id, &KaigiFrame::RoomConfig(cfg)).await?;

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
        KaigiFrame::Moderation(moderation) => {
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
                if room.host_conn_id != Some(conn_id) {
                    drop(rooms);
                    send_error(state, room_id, conn_id, "host only".to_string()).await?;
                    return Ok(());
                }

                match &target {
                    ModerationTarget::All => {
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
                    let Some(target) = room.participants.get_mut(target_conn_id) else {
                        continue;
                    };
                    match &action {
                        ModerationAction::Kick => {
                            close_senders.push(target_tx.clone());
                        }
                        ModerationAction::DisableMic => {
                            target.state.mic_enabled = false;
                            snapshots.push(participant_snapshot_from_state(&target.state, now));
                        }
                        ModerationAction::DisableVideo => {
                            target.state.video_enabled = false;
                            snapshots.push(participant_snapshot_from_state(&target.state, now));
                        }
                        ModerationAction::DisableScreenShare => {
                            target.state.screen_share_enabled = false;
                            snapshots.push(participant_snapshot_from_state(&target.state, now));
                        }
                    }
                }
            }

            for snap in snapshots {
                broadcast_frame(
                    state,
                    &room_id,
                    &KaigiFrame::Event(RoomEventFrame::StateUpdated(snap)),
                )
                .await?;
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
        }
        KaigiFrame::RoomConfig(_) => {}
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

    for tx in senders {
        let _ = tx.try_send(Message::Binary(bytes.clone()));
    }

    Ok(())
}

fn now_ms() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.as_millis()).unwrap_or(u64::MAX)
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

    #[test]
    fn anon_handle_validation_rejects_empty_and_duplicate() {
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
        assert_eq!(record_anon_admission_rejection(&mut room), 1);
        assert_eq!(record_anon_admission_rejection(&mut room), 2);
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
        let mut room = RoomState {
            participants: HashMap::new(),
            host_conn_id: None,
            rate_per_minute_nano: 1,
            max_screen_shares: 1,
            anonymous_mode: true,
            anon_admission_rejections: 0,
        };
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
