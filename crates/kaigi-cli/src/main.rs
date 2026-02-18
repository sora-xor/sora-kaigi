use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
        Mutex as StdMutex, OnceLock,
    },
};

#[cfg(target_os = "macos")]
use std::process::Command as OsCommand;

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;
use chacha20poly1305::{
    KeyInit as _, XChaCha20Poly1305, XNonce,
    aead::{Aead as _, Payload},
};
use clap::{Parser, Subcommand};
use kaigi_platform_contract::{all_platform_contracts, platform_contract, TargetPlatform};
use kaigi_soranet_client::{
    HandshakeParams, RelayConnectOptions, connect_and_handshake, decode_hex_32, decode_hex_vec,
    derive_kaigi_room_id, fetch_handshake_params_from_torii, open_kaigi_stream,
};
use kaigi_wire::{
    AnonHelloFrame, AnonRosterFrame, ChatFrame, DeviceCapabilityFrame, E2EEKeyEpochFrame,
    EncryptedControlFrame, EncryptedControlKind, EncryptedRecipientPayload, EscrowProofFrame,
    FrameDecoder, GroupKeyUpdateFrame, HelloFrame, KaigiFrame, KeyRotationAckFrame,
    MAX_ANON_PARTICIPANT_HANDLE_LEN, MAX_ESCROW_ID_LEN, MAX_ESCROW_PROOF_HEX_LEN,
    MediaProfileKind, MediaProfileNegotiationFrame, ModerationAction, ModerationSignedFrame,
    ModerationTarget, PROTOCOL_VERSION, ParticipantStateFrame, PaymentFrame,
    PermissionsSnapshotFrame, PingFrame, RecordingNoticeFrame, RecordingState, RoleGrantFrame,
    RoleKind, RoleRevokeFrame, RoomConfigUpdateFrame, SessionPolicyFrame, encode_framed,
};
use norito::{
    streaming::{
        PrivacyRouteUpdate, SoranetAccessKind, SoranetChannelId, SoranetRoute, SoranetStreamTag,
    },
    to_bytes,
};
use rand::RngCore as _;
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Duration;
use tracing::info;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};

#[derive(Parser, Debug)]
#[command(
    name = "kaigi-cli",
    version,
    about = "Kaigi over SoraNet relay CLI (dev)"
)]
struct Cli {
    /// Log level (defaults to `info`).
    #[arg(long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    cmd: Command,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Fetch handshake params from an Iroha Torii endpoint (/v1/config).
    FetchHandshake {
        /// Torii base URL (e.g. http://127.0.0.1:8080).
        #[arg(long)]
        torii: String,
    },

    /// Connect to a relay, open a Kaigi stream, send bytes, and read echoed bytes.
    RelayEcho(RelayEchoArgs),

    /// Connect to a relay, open a Kaigi stream, and run an interactive room chat (dev).
    RoomChat(RoomChatArgs),

    /// Transfer XOR on-ledger (utility; uses the upstream `iroha` CLI).
    XorTransfer(XorTransferArgs),

    /// Build a shareable Kaigi join link.
    MakeJoinLink(MakeJoinLinkArgs),

    /// Decode and inspect a Kaigi join link.
    DecodeJoinLink(DecodeJoinLinkArgs),

    /// Print the frozen platform/browser parity contract as JSON.
    PlatformContract(PlatformContractArgs),

    /// Submit Kaigi lifecycle instructions via the upstream `iroha` CLI.
    KaigiLifecycle(KaigiLifecycleArgs),

    /// List and decode Kaigi route updates from a relay spool directory.
    ListRoutes(ListRoutesArgs),

    /// Write a Kaigi PrivacyRouteUpdate (.norito) into a relay spool catalog.
    ///
    /// This is a dev harness helper used to provision a `channel_id` -> exit route mapping
    /// for `soranet-relay`, which currently reads routes from disk.
    WriteRouteUpdate(WriteRouteUpdateArgs),
}

#[derive(Parser, Debug)]
struct RelayEchoArgs {
    /// Relay QUIC listen address (host:port).
    #[arg(long)]
    relay: SocketAddr,

    /// TLS SNI name (defaults to `localhost`).
    #[arg(long)]
    server_name: Option<String>,

    /// Accept any relay TLS certificate (dev-only).
    #[arg(long)]
    insecure: bool,

    /// Trust this PEM bundle as a root CA (required unless --insecure).
    #[arg(long)]
    ca_cert_pem_path: Option<PathBuf>,

    /// Torii base URL to fetch handshake params from (/v1/config). If omitted, fixture defaults
    /// are used unless explicit --descriptor-commit-hex / --capabilities flags are provided.
    #[arg(long)]
    torii: Option<String>,

    /// Descriptor commit (hex, 32 bytes).
    #[arg(long)]
    descriptor_commit_hex: Option<String>,
    /// Client capability TLVs (hex).
    #[arg(long)]
    client_capabilities_hex: Option<String>,
    /// Relay capability TLVs (hex).
    #[arg(long)]
    relay_capabilities_hex: Option<String>,
    /// Negotiated ML-KEM id.
    #[arg(long)]
    kem_id: Option<u8>,
    /// Negotiated signature suite id.
    #[arg(long)]
    sig_id: Option<u8>,
    /// Optional resume hash (hex).
    #[arg(long)]
    resume_hash_hex: Option<String>,

    /// Optional raw handshake prelude frame (hex). Only send ONE prelude frame.
    #[arg(long)]
    handshake_prelude_hex: Option<String>,

    /// Kaigi channel id (hex, 32 bytes).
    #[arg(long)]
    channel: String,

    /// Set the authenticated flag in the RouteOpenFrame.
    #[arg(long)]
    authenticated: bool,

    /// UTF-8 message to send (default: "ping").
    #[arg(long)]
    message: Option<String>,

    /// Message bytes to send as hex (overrides --message).
    #[arg(long)]
    message_hex: Option<String>,
}

#[derive(Parser, Debug, Clone)]
struct RoomChatArgs {
    /// Relay QUIC listen address (host:port). Required unless `--join-link` is provided.
    #[arg(long)]
    relay: Option<SocketAddr>,

    /// Shareable join link generated via `make-join-link`.
    #[arg(long)]
    join_link: Option<String>,

    /// TLS SNI name (defaults to `localhost`).
    #[arg(long)]
    server_name: Option<String>,

    /// Accept any relay TLS certificate (dev-only).
    #[arg(long)]
    insecure: bool,

    /// Trust this PEM bundle as a root CA (required unless --insecure).
    #[arg(long)]
    ca_cert_pem_path: Option<PathBuf>,

    /// Torii base URL to fetch handshake params from (/v1/config).
    ///
    /// Required by default for Nexus-routed calls unless `--allow-local-handshake` is set.
    #[arg(long)]
    torii: Option<String>,

    /// Allow bypassing Torii handshake discovery (local dev harness only).
    #[arg(long)]
    allow_local_handshake: bool,

    /// Descriptor commit (hex, 32 bytes).
    #[arg(long)]
    descriptor_commit_hex: Option<String>,
    /// Client capability TLVs (hex).
    #[arg(long)]
    client_capabilities_hex: Option<String>,
    /// Relay capability TLVs (hex).
    #[arg(long)]
    relay_capabilities_hex: Option<String>,
    /// Negotiated ML-KEM id.
    #[arg(long)]
    kem_id: Option<u8>,
    /// Negotiated signature suite id.
    #[arg(long)]
    sig_id: Option<u8>,
    /// Optional resume hash (hex).
    #[arg(long)]
    resume_hash_hex: Option<String>,

    /// Optional raw handshake prelude frame (hex). Only send ONE prelude frame.
    #[arg(long)]
    handshake_prelude_hex: Option<String>,

    /// Kaigi channel id (hex, 32 bytes). Required unless `--join-link` is provided.
    #[arg(long)]
    channel: Option<String>,

    /// Set the authenticated flag in the RouteOpenFrame.
    #[arg(long)]
    authenticated: bool,

    /// Participant id (defaults to random).
    #[arg(long)]
    participant_id: Option<String>,

    /// Display name (optional).
    #[arg(long)]
    display_name: Option<String>,

    /// Hint: the local display supports HDR.
    #[arg(long)]
    hdr_display: bool,

    /// Hint: local capture can produce HDR frames.
    #[arg(long)]
    hdr_capture: bool,

    /// Disable automatic HDR display detection.
    #[arg(long)]
    no_hdr_auto: bool,

    /// Pay rate in nano-XOR per minute (1e-9 XOR). If non-zero, the client will periodically send
    /// `Payment` frames while connected (dev harness).
    #[arg(long, default_value_t = 0)]
    pay_rate_per_minute_nano: u64,

    /// Automatically follow the hub's `RoomConfig` rate (enabled by default).
    #[arg(long)]
    pay_auto: bool,

    /// Disable automatic rate-following from hub `RoomConfig`.
    #[arg(long, conflicts_with = "pay_auto")]
    no_pay_auto: bool,

    /// How often to emit payment frames (seconds).
    #[arg(long, default_value_t = 15)]
    pay_interval_secs: u64,

    /// Optional path to the upstream `iroha` CLI binary (defaults to `../iroha/target/release/iroha`).
    #[arg(long)]
    pay_iroha_bin: Option<String>,

    /// Optional Iroha client config TOML to submit real XOR transfers for the pay loop.
    #[arg(long)]
    pay_iroha_config: Option<PathBuf>,

    /// Optional payer account id override (e.g. `<public_key>@sora`). If omitted, inferred from `--pay-iroha-config`.
    #[arg(long)]
    pay_from: Option<String>,

    /// Destination account id for XOR transfers.
    #[arg(long)]
    pay_to: Option<String>,

    /// XOR asset definition id (defaults to `xor#sora`).
    #[arg(long, default_value = "xor#sora")]
    pay_asset_def: String,

    /// Wait until each payment transfer is applied (slower but deterministic).
    #[arg(long)]
    pay_blocking: bool,

    /// Allow dev-only payment frames without submitting real XOR transfers on-ledger.
    #[arg(long)]
    allow_unsettled_payments: bool,

    /// Optional path to the upstream `iroha` CLI binary for Kaigi lifecycle mirroring.
    #[arg(long)]
    kaigi_iroha_bin: Option<String>,

    /// Optional Iroha client config TOML for Kaigi lifecycle mirroring.
    ///
    /// When set together with `--kaigi-domain --kaigi-call-name --kaigi-participant`,
    /// `room-chat` will submit `join` on connect and `leave` on disconnect.
    /// If `/end` is used, it also submits `end` before disconnecting.
    #[arg(long)]
    kaigi_iroha_config: Option<PathBuf>,

    /// Kaigi domain for lifecycle mirroring.
    #[arg(long)]
    kaigi_domain: Option<String>,

    /// Kaigi call name for lifecycle mirroring.
    #[arg(long)]
    kaigi_call_name: Option<String>,

    /// Kaigi privacy mode (`transparent`, `zk`, `zk_roster_v1`, or `zk-roster-v1`).
    /// `zk` enables anonymous control-plane mode.
    #[arg(long)]
    kaigi_privacy_mode: Option<String>,

    /// Account id used by `iroha app kaigi join/leave --participant`.
    #[arg(long)]
    kaigi_participant: Option<String>,

    /// Join commitment hash (hex) for privacy mode joins.
    #[arg(long)]
    kaigi_join_commitment_hex: Option<String>,

    /// Optional commitment alias for privacy mode joins.
    #[arg(long)]
    kaigi_join_commitment_alias: Option<String>,

    /// Join nullifier hash (hex) for privacy mode joins.
    #[arg(long)]
    kaigi_join_nullifier_hex: Option<String>,

    /// Leave commitment hash (hex) for privacy mode leaves.
    #[arg(long)]
    kaigi_leave_commitment_hex: Option<String>,

    /// Leave nullifier hash (hex) for privacy mode leaves.
    #[arg(long)]
    kaigi_leave_nullifier_hex: Option<String>,

    /// Nullifier timestamp used for join/leave privacy mode operations.
    #[arg(long)]
    kaigi_nullifier_issued_at_ms: Option<u64>,

    /// Roster root hash (hex) bound into privacy proofs.
    #[arg(long)]
    kaigi_roster_root_hex: Option<String>,

    /// Join proof bytes (hex) for privacy mode joins.
    #[arg(long)]
    kaigi_join_proof_hex: Option<String>,

    /// Leave proof bytes (hex) for privacy mode leaves.
    #[arg(long)]
    kaigi_leave_proof_hex: Option<String>,

    /// Also submit `record-usage` on disconnect.
    #[arg(long)]
    kaigi_record_usage: bool,

    /// Billed gas value to pass to `record-usage` (defaults to 0).
    #[arg(long, default_value_t = 0)]
    kaigi_billed_gas: u64,

    /// Usage commitment hash (hex) for privacy mode `record-usage`.
    #[arg(long)]
    kaigi_usage_commitment_hex: Option<String>,

    /// Usage proof bytes (hex) for privacy mode `record-usage`.
    #[arg(long)]
    kaigi_usage_proof_hex: Option<String>,

    /// Shielded XOR prepay amount (nano units) for anonymous (`privacy=zk`) rooms.
    #[arg(long, default_value_t = 0)]
    anon_escrow_prepay_nano: u64,

    /// Additional zk feature fee charged in nano-XOR per minute for anonymous (`privacy=zk`) rooms.
    #[arg(long, default_value_t = 0)]
    anon_zk_extra_fee_per_minute_nano: u64,

    /// Expected session duration used to estimate zk surcharge in prepay calculation.
    #[arg(long, default_value_t = 60)]
    anon_expected_duration_secs: u64,

    /// Interval for anonymous escrow proof heartbeats (seconds). Set to 0 to disable periodic proofs.
    #[arg(long, default_value_t = 20)]
    anon_escrow_proof_interval_secs: u64,

    /// Explicit escrow id for anonymous room proofs. Random when omitted.
    #[arg(long)]
    anon_escrow_id: Option<String>,

    /// Opaque escrow proof bytes (hex). Random when omitted.
    #[arg(long)]
    anon_escrow_proof_hex: Option<String>,

    /// Attempt to unshield remaining escrow on disconnect (requires args below).
    #[arg(long)]
    anon_unshield_on_exit: bool,

    /// Recipient account for optional unshield on disconnect.
    #[arg(long)]
    anon_unshield_to: Option<String>,

    /// Optional comma-separated spent nullifiers (hex32 list) for unshield.
    #[arg(long)]
    anon_unshield_inputs: Option<String>,

    /// JSON proof attachment file for unshield.
    #[arg(long)]
    anon_unshield_proof_json: Option<PathBuf>,

    /// Optional root hint hash (hex32) for unshield.
    #[arg(long)]
    anon_unshield_root_hint_hex: Option<String>,
}

#[derive(Parser, Debug)]
struct XorTransferArgs {
    /// Optional path to the upstream `iroha` CLI binary (defaults to `../iroha/target/release/iroha`).
    #[arg(long)]
    iroha_bin: Option<String>,

    /// Path to an Iroha client config TOML.
    #[arg(long)]
    iroha_config: PathBuf,

    /// Optional payer account id override (e.g. `<public_key>@sora`). If omitted, inferred from `--iroha-config`.
    #[arg(long)]
    from: Option<String>,

    /// Destination account id.
    #[arg(long)]
    to: String,

    /// Amount in nano-XOR (1e-9 XOR).
    #[arg(long)]
    amount_nano_xor: u64,

    /// XOR asset definition id (defaults to `xor#sora`).
    #[arg(long, default_value = "xor#sora")]
    asset_def: String,

    /// Wait until the transfer is applied.
    #[arg(long)]
    blocking: bool,
}

#[derive(Parser, Debug)]
struct MakeJoinLinkArgs {
    /// Relay QUIC listen address (host:port).
    #[arg(long)]
    relay: SocketAddr,

    /// Kaigi channel id (hex, 32 bytes).
    #[arg(long)]
    channel: String,

    /// TLS SNI name (optional, defaults to `localhost` in consumers).
    #[arg(long)]
    server_name: Option<String>,

    /// Set the authenticated flag in the RouteOpenFrame.
    #[arg(long)]
    authenticated: bool,

    /// Accept any relay TLS certificate (dev-only).
    #[arg(long)]
    insecure: bool,

    /// Optional Torii base URL for handshake config discovery.
    ///
    /// Required by default for Nexus-routed calls unless `--allow-local-handshake` is set.
    #[arg(long)]
    torii: Option<String>,

    /// Optional destination account id for XOR transfers (billing).
    #[arg(long)]
    pay_to: Option<String>,

    /// Optional Kaigi domain to embed for lifecycle mirroring.
    #[arg(long)]
    kaigi_domain: Option<String>,

    /// Optional Kaigi call name to embed for lifecycle mirroring.
    #[arg(long)]
    kaigi_call_name: Option<String>,

    /// Optional Kaigi privacy mode (`transparent`, `zk`, `zk_roster_v1`, or `zk-roster-v1`) to embed.
    #[arg(long)]
    kaigi_privacy_mode: Option<String>,

    /// Expiration window in seconds for signed join links (`v=2`).
    #[arg(long, default_value_t = DEFAULT_JOIN_LINK_EXPIRES_IN_SECS)]
    expires_in_secs: u64,

    /// Emit legacy unsigned link format (`v=1`) for backward compatibility.
    #[arg(long)]
    legacy_v1: bool,

    /// Allow generating a join link without Torii routing metadata (local dev harness only).
    #[arg(long)]
    allow_local_handshake: bool,
}

#[derive(Parser, Debug)]
struct DecodeJoinLinkArgs {
    /// Join link to decode.
    #[arg(long)]
    link: String,
}

#[derive(Parser, Debug)]
struct PlatformContractArgs {
    /// Optional target platform selector.
    ///
    /// Allowed values: web-chromium, web-safari, web-firefox, macos, ios, ipados,
    /// windows, android, linux.
    #[arg(long)]
    platform: Option<String>,

    /// Print human-readable JSON.
    #[arg(long)]
    pretty: bool,
}

#[derive(Parser, Debug)]
struct KaigiLifecycleArgs {
    /// Optional path to the upstream `iroha` CLI binary (defaults to `../iroha/target/release/iroha`).
    #[arg(long)]
    iroha_bin: Option<String>,

    /// Path to an Iroha client config TOML.
    #[arg(long)]
    iroha_config: PathBuf,

    #[command(subcommand)]
    cmd: KaigiLifecycleCommand,
}

#[derive(Subcommand, Debug)]
enum KaigiLifecycleCommand {
    /// Create a Kaigi call on-ledger.
    Create {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        call_name: String,
        #[arg(long)]
        host: String,
        #[arg(long, default_value = "authenticated")]
        room_policy: String,
        #[arg(long, default_value = "transparent")]
        privacy_mode: String,
        #[arg(long, default_value_t = 0)]
        gas_rate_per_minute: u64,
        #[arg(long, default_value_t = 0)]
        zk_extra_fee_per_minute_nano: u64,
    },
    /// Join a Kaigi call on-ledger.
    Join {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        call_name: String,
        #[arg(long)]
        participant: String,
        #[arg(long)]
        commitment_hex: Option<String>,
        #[arg(long)]
        commitment_alias: Option<String>,
        #[arg(long)]
        nullifier_hex: Option<String>,
        #[arg(long)]
        nullifier_issued_at_ms: Option<u64>,
        #[arg(long)]
        roster_root_hex: Option<String>,
        #[arg(long)]
        proof_hex: Option<String>,
    },
    /// Leave a Kaigi call on-ledger.
    Leave {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        call_name: String,
        #[arg(long)]
        participant: String,
        #[arg(long)]
        commitment_hex: Option<String>,
        #[arg(long)]
        nullifier_hex: Option<String>,
        #[arg(long)]
        nullifier_issued_at_ms: Option<u64>,
        #[arg(long)]
        roster_root_hex: Option<String>,
        #[arg(long)]
        proof_hex: Option<String>,
    },
    /// End a Kaigi call on-ledger.
    End {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        call_name: String,
        #[arg(long)]
        ended_at_ms: Option<u64>,
    },
    /// Record Kaigi usage on-ledger.
    RecordUsage {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        call_name: String,
        #[arg(long)]
        duration_ms: u64,
        #[arg(long, default_value_t = 0)]
        billed_gas: u64,
        #[arg(long)]
        usage_commitment_hex: Option<String>,
        #[arg(long)]
        proof_hex: Option<String>,
    },
}

#[derive(Parser, Debug)]
struct WriteRouteUpdateArgs {
    /// Base spool dir configured as `kaigi_stream.spool_dir` in `soranet-relay` config.
    #[arg(long)]
    spool_dir: PathBuf,

    /// Relay id (hex, 32 bytes) used to scope the on-disk catalog directory: exit-<relay-id>/kaigi-stream/
    #[arg(long)]
    relay_id: String,

    /// Kaigi channel id (hex, 32 bytes). If omitted, a random id is generated.
    #[arg(long)]
    channel: Option<String>,

    /// Exit multiaddr (e.g. /ip4/127.0.0.1/tcp/9000/ws).
    #[arg(long)]
    exit_multiaddr: String,

    /// Optional relay QUIC address for generating a ready-to-share join link.
    #[arg(long)]
    relay: Option<SocketAddr>,

    /// TLS SNI name to embed in generated join link (defaults to `localhost`).
    #[arg(long)]
    server_name: Option<String>,

    /// Embed `insecure=1` in generated join link.
    #[arg(long)]
    insecure: bool,

    /// Optional Torii base URL to embed in generated join link.
    #[arg(long)]
    torii: Option<String>,

    /// Optional destination account id for XOR transfers (billing).
    #[arg(long)]
    pay_to: Option<String>,

    /// Optional Kaigi domain to embed for lifecycle mirroring.
    #[arg(long)]
    kaigi_domain: Option<String>,

    /// Optional Kaigi call name to embed for lifecycle mirroring.
    #[arg(long)]
    kaigi_call_name: Option<String>,

    /// Optional Kaigi privacy mode (`transparent`, `zk`, `zk_roster_v1`, or `zk-roster-v1`) to embed.
    #[arg(long)]
    kaigi_privacy_mode: Option<String>,

    /// Expiration window in seconds for generated join links (`v=2`).
    #[arg(long, default_value_t = DEFAULT_JOIN_LINK_EXPIRES_IN_SECS)]
    join_link_expires_in_secs: u64,

    /// Emit legacy unsigned join link format (`v=1`) for backward compatibility.
    #[arg(long)]
    join_link_legacy_v1: bool,

    /// Allow generating join links without Torii metadata (local dev harness only).
    #[arg(long)]
    allow_local_handshake: bool,

    /// Access kind: public (read_only) or authenticated.
    #[arg(long, value_parser = ["public", "authenticated"], default_value = "public")]
    access_kind: String,

    /// Optional padding budget in milliseconds.
    #[arg(long)]
    padding_budget_ms: Option<u16>,

    /// Segment window: route is valid from this segment (default: 0).
    #[arg(long, default_value_t = 0)]
    valid_from_segment: u64,

    /// Segment window: route is valid until this segment (default: u64::MAX).
    #[arg(long, default_value_t = u64::MAX)]
    valid_until_segment: u64,
}

#[derive(Parser, Debug)]
struct ListRoutesArgs {
    /// Base spool dir configured as `kaigi_stream.spool_dir` in `soranet-relay` config.
    #[arg(long)]
    spool_dir: PathBuf,

    /// Optional relay id (hex, 32 bytes) to scope to a single directory: exit-<relay-id>/kaigi-stream/
    #[arg(long)]
    relay_id: Option<String>,

    /// Maximum number of route update files to print (newest first).
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Clone, Debug)]
struct JoinLinkPayload {
    version: u8,
    relay: SocketAddr,
    channel: [u8; 32],
    authenticated: bool,
    insecure: bool,
    server_name: Option<String>,
    torii: Option<String>,
    pay_to: Option<String>,
    kaigi_domain: Option<String>,
    kaigi_call_name: Option<String>,
    kaigi_privacy_mode: Option<String>,
    expires_at_ms: Option<u64>,
    nonce_hex: Option<String>,
    signature_hex: Option<String>,
}

#[derive(Clone, Debug)]
struct AutoKaigiLifecycle {
    iroha_bin: String,
    iroha_config: PathBuf,
    domain: String,
    call_name: String,
    participant: String,
    join_commitment_hex: Option<String>,
    join_commitment_alias: Option<String>,
    join_nullifier_hex: Option<String>,
    leave_commitment_hex: Option<String>,
    leave_nullifier_hex: Option<String>,
    nullifier_issued_at_ms: Option<u64>,
    roster_root_hex: Option<String>,
    join_proof_hex: Option<String>,
    leave_proof_hex: Option<String>,
    record_usage: bool,
    billed_gas: u64,
    usage_commitment_hex: Option<String>,
    usage_proof_hex: Option<String>,
}

const HOST_STATE_UNKNOWN: u8 = 0;
const HOST_STATE_SELF: u8 = 1;
const HOST_STATE_OTHER: u8 = 2;
const MAX_ANON_UNSHIELD_INPUTS: usize = 256;
const DEFAULT_POLICY_MAX_PARTICIPANTS: u32 = 500;
const JOIN_LINK_VERSION_LEGACY: u8 = 1;
const JOIN_LINK_VERSION_SIGNED: u8 = 2;
const JOIN_LINK_NONCE_BYTES: usize = 16;
const DEFAULT_JOIN_LINK_EXPIRES_IN_SECS: u64 = 3600;
const MAX_JOIN_LINK_EXPIRES_IN_SECS: u64 = 7 * 24 * 3600;
const MAX_JOIN_LINK_NONCE_CACHE_ENTRIES: usize = 4096;

static SEEN_JOIN_LINK_NONCES: OnceLock<StdMutex<HashMap<String, u64>>> = OnceLock::new();

#[derive(Clone, Debug)]
struct LocalSessionPolicyState {
    room_lock: bool,
    waiting_room_enabled: bool,
    guest_join_allowed: bool,
    local_recording_allowed: bool,
    e2ee_required: bool,
    max_participants: u32,
    policy_epoch: u64,
}

impl Default for LocalSessionPolicyState {
    fn default() -> Self {
        Self {
            room_lock: false,
            waiting_room_enabled: false,
            guest_join_allowed: true,
            local_recording_allowed: true,
            e2ee_required: true,
            max_participants: DEFAULT_POLICY_MAX_PARTICIPANTS,
            policy_epoch: 0,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log_level)?;

    match cli.cmd {
        Command::FetchHandshake { torii } => {
            let params = fetch_handshake_params_from_torii(&torii).await?;
            print_handshake(&params);
        }
        Command::RelayEcho(args) => {
            relay_echo(args).await?;
        }
        Command::RoomChat(args) => {
            room_chat(args).await?;
        }
        Command::XorTransfer(args) => {
            xor_transfer(args).await?;
        }
        Command::MakeJoinLink(args) => {
            make_join_link(args)?;
        }
        Command::DecodeJoinLink(args) => {
            decode_join_link(args)?;
        }
        Command::PlatformContract(args) => {
            print_platform_contract(args)?;
        }
        Command::KaigiLifecycle(args) => {
            kaigi_lifecycle(args)?;
        }
        Command::ListRoutes(args) => {
            list_routes(args)?;
        }
        Command::WriteRouteUpdate(args) => {
            write_route_update(args)?;
        }
    }

    Ok(())
}

fn make_join_link(args: MakeJoinLinkArgs) -> Result<()> {
    validate_nexus_routing_requirement(args.allow_local_handshake, args.torii.as_deref())?;
    validate_join_link_call_metadata(
        args.kaigi_domain.as_deref(),
        args.kaigi_call_name.as_deref(),
    )?;
    validate_privacy_mode_arg(args.kaigi_privacy_mode.as_deref())?;
    let mut payload = JoinLinkPayload {
        version: if args.legacy_v1 {
            JOIN_LINK_VERSION_LEGACY
        } else {
            JOIN_LINK_VERSION_SIGNED
        },
        relay: args.relay,
        channel: decode_hex_32(&args.channel)?,
        authenticated: args.authenticated,
        insecure: args.insecure,
        server_name: args.server_name,
        torii: args.torii,
        pay_to: args.pay_to,
        kaigi_domain: args.kaigi_domain,
        kaigi_call_name: args.kaigi_call_name,
        kaigi_privacy_mode: normalize_privacy_mode(args.kaigi_privacy_mode),
        expires_at_ms: None,
        nonce_hex: None,
        signature_hex: None,
    };
    if payload.version == JOIN_LINK_VERSION_SIGNED {
        populate_join_link_security_fields(&mut payload, args.expires_in_secs)?;
    }
    println!("{}", render_join_link(&payload));
    Ok(())
}

fn decode_join_link(args: DecodeJoinLinkArgs) -> Result<()> {
    let payload = parse_join_link_for_inspection(&args.link)?;
    println!("version={}", payload.version);
    println!("relay={}", payload.relay);
    println!("channel_id_hex={}", hex::encode(payload.channel));
    println!("authenticated={}", payload.authenticated);
    println!("insecure={}", payload.insecure);
    println!(
        "server_name={}",
        payload.server_name.as_deref().unwrap_or("<none>")
    );
    println!("torii={}", payload.torii.as_deref().unwrap_or("<none>"));
    println!("pay_to={}", payload.pay_to.as_deref().unwrap_or("<none>"));
    println!(
        "kaigi_domain={}",
        payload.kaigi_domain.as_deref().unwrap_or("<none>")
    );
    println!(
        "kaigi_call_name={}",
        payload.kaigi_call_name.as_deref().unwrap_or("<none>")
    );
    println!(
        "kaigi_privacy_mode={}",
        payload.kaigi_privacy_mode.as_deref().unwrap_or("<none>")
    );
    println!(
        "expires_at_ms={}",
        payload
            .expires_at_ms
            .map(|value| value.to_string())
            .as_deref()
            .unwrap_or("<none>")
    );
    println!(
        "nonce_hex={}",
        payload.nonce_hex.as_deref().unwrap_or("<none>")
    );
    println!(
        "signature_hex={}",
        payload.signature_hex.as_deref().unwrap_or("<none>")
    );
    println!("normalized_link={}", render_join_link(&payload));
    Ok(())
}

fn print_platform_contract(args: PlatformContractArgs) -> Result<()> {
    let payload = if let Some(requested_platform) = args.platform.as_deref() {
        let platform = parse_target_platform(requested_platform)?;
        serde_json::json!({
            "schema": "kaigi-platform-contract/v1",
            "frozen_at": "2026-02-15",
            "contract": platform_contract(platform),
        })
    } else {
        serde_json::json!({
            "schema": "kaigi-platform-contract/v1",
            "frozen_at": "2026-02-15",
            "contracts": all_platform_contracts(),
        })
    };
    if args.pretty {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{}", serde_json::to_string(&payload)?);
    }
    Ok(())
}

fn parse_target_platform(raw: &str) -> Result<TargetPlatform> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    let platform = match normalized.as_str() {
        "web-chromium" | "chromium" => TargetPlatform::WebChromium,
        "web-safari" | "safari" => TargetPlatform::WebSafari,
        "web-firefox" | "firefox" => TargetPlatform::WebFirefox,
        "macos" | "mac-os" => TargetPlatform::MacOS,
        "ios" => TargetPlatform::IOS,
        "ipados" | "ipad-os" | "ipad" => TargetPlatform::IPadOS,
        "windows" | "win" => TargetPlatform::Windows,
        "android" => TargetPlatform::Android,
        "linux" => TargetPlatform::Linux,
        _ => {
            return Err(anyhow!(
                "unsupported --platform value `{raw}`; expected one of web-chromium, web-safari, web-firefox, macos, ios, ipados, windows, android, linux"
            ));
        }
    };
    Ok(platform)
}

fn kaigi_lifecycle(args: KaigiLifecycleArgs) -> Result<()> {
    let iroha_bin = args
        .iroha_bin
        .unwrap_or_else(|| ledger::default_iroha_bin().to_string());
    let cmd = build_kaigi_lifecycle_cli_args(args.cmd)?;

    let output = run_iroha_json_cli(&iroha_bin, &args.iroha_config, &cmd)?;
    println!("{output}");
    Ok(())
}

fn build_kaigi_lifecycle_cli_args(cmd: KaigiLifecycleCommand) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec!["app".to_string(), "kaigi".to_string()];
    match cmd {
        KaigiLifecycleCommand::Create {
            domain,
            call_name,
            host,
            room_policy,
            privacy_mode,
            gas_rate_per_minute,
            zk_extra_fee_per_minute_nano,
        } => {
            let room_policy = normalize_room_policy_arg(room_policy)?;
            let (privacy_mode, effective_gas_rate) = resolve_lifecycle_create_pricing(
                privacy_mode,
                gas_rate_per_minute,
                zk_extra_fee_per_minute_nano,
            )?;
            args.extend([
                "create".to_string(),
                "--domain".to_string(),
                domain,
                "--call-name".to_string(),
                call_name,
                "--host".to_string(),
                host,
                "--room-policy".to_string(),
                room_policy,
                "--privacy-mode".to_string(),
                privacy_mode,
                "--gas-rate-per-minute".to_string(),
                effective_gas_rate.to_string(),
            ]);
        }
        KaigiLifecycleCommand::Join {
            domain,
            call_name,
            participant,
            commitment_hex,
            commitment_alias,
            nullifier_hex,
            nullifier_issued_at_ms,
            roster_root_hex,
            proof_hex,
        } => {
            args.extend([
                "join".to_string(),
                "--domain".to_string(),
                domain,
                "--call-name".to_string(),
                call_name,
                "--participant".to_string(),
                participant,
            ]);
            validate_commitment_alias_dependency(
                commitment_hex.as_deref(),
                commitment_alias.as_deref(),
                "--commitment-hex",
                "--commitment-alias",
            )?;
            validate_nullifier_timestamp_dependency(
                nullifier_hex.as_deref(),
                nullifier_issued_at_ms,
                "--nullifier-hex",
                "--nullifier-issued-at-ms",
            )?;
            validate_optional_hex_arg(commitment_hex.as_deref(), "--commitment-hex")?;
            validate_optional_hex_arg(nullifier_hex.as_deref(), "--nullifier-hex")?;
            validate_optional_hex_arg(roster_root_hex.as_deref(), "--roster-root-hex")?;
            validate_optional_hex_arg(proof_hex.as_deref(), "--proof-hex")?;
            let commitment_hex = normalize_optional_hex_owned(commitment_hex);
            let nullifier_hex = normalize_optional_hex_owned(nullifier_hex);
            let roster_root_hex = normalize_optional_hex_owned(roster_root_hex);
            let proof_hex = normalize_optional_hex_owned(proof_hex);
            if let Some(value) = commitment_hex {
                args.extend(["--commitment-hex".to_string(), value]);
            }
            if let Some(value) = commitment_alias {
                args.extend(["--commitment-alias".to_string(), value]);
            }
            if let Some(value) = nullifier_hex {
                args.extend(["--nullifier-hex".to_string(), value]);
            }
            if let Some(value) = nullifier_issued_at_ms {
                args.extend(["--nullifier-issued-at-ms".to_string(), value.to_string()]);
            }
            if let Some(value) = roster_root_hex {
                args.extend(["--roster-root-hex".to_string(), value]);
            }
            if let Some(value) = proof_hex {
                args.extend(["--proof-hex".to_string(), value]);
            }
        }
        KaigiLifecycleCommand::Leave {
            domain,
            call_name,
            participant,
            commitment_hex,
            nullifier_hex,
            nullifier_issued_at_ms,
            roster_root_hex,
            proof_hex,
        } => {
            args.extend([
                "leave".to_string(),
                "--domain".to_string(),
                domain,
                "--call-name".to_string(),
                call_name,
                "--participant".to_string(),
                participant,
            ]);
            validate_nullifier_timestamp_dependency(
                nullifier_hex.as_deref(),
                nullifier_issued_at_ms,
                "--nullifier-hex",
                "--nullifier-issued-at-ms",
            )?;
            validate_optional_hex_arg(commitment_hex.as_deref(), "--commitment-hex")?;
            validate_optional_hex_arg(nullifier_hex.as_deref(), "--nullifier-hex")?;
            validate_optional_hex_arg(roster_root_hex.as_deref(), "--roster-root-hex")?;
            validate_optional_hex_arg(proof_hex.as_deref(), "--proof-hex")?;
            let commitment_hex = normalize_optional_hex_owned(commitment_hex);
            let nullifier_hex = normalize_optional_hex_owned(nullifier_hex);
            let roster_root_hex = normalize_optional_hex_owned(roster_root_hex);
            let proof_hex = normalize_optional_hex_owned(proof_hex);
            if let Some(value) = commitment_hex {
                args.extend(["--commitment-hex".to_string(), value]);
            }
            if let Some(value) = nullifier_hex {
                args.extend(["--nullifier-hex".to_string(), value]);
            }
            if let Some(value) = nullifier_issued_at_ms {
                args.extend(["--nullifier-issued-at-ms".to_string(), value.to_string()]);
            }
            if let Some(value) = roster_root_hex {
                args.extend(["--roster-root-hex".to_string(), value]);
            }
            if let Some(value) = proof_hex {
                args.extend(["--proof-hex".to_string(), value]);
            }
        }
        KaigiLifecycleCommand::End {
            domain,
            call_name,
            ended_at_ms,
        } => {
            args.extend([
                "end".to_string(),
                "--domain".to_string(),
                domain,
                "--call-name".to_string(),
                call_name,
            ]);
            if let Some(ended_at_ms) = ended_at_ms {
                args.extend(["--ended-at-ms".to_string(), ended_at_ms.to_string()]);
            }
        }
        KaigiLifecycleCommand::RecordUsage {
            domain,
            call_name,
            duration_ms,
            billed_gas,
            usage_commitment_hex,
            proof_hex,
        } => {
            validate_optional_hex_arg(usage_commitment_hex.as_deref(), "--usage-commitment-hex")?;
            validate_optional_hex_arg(proof_hex.as_deref(), "--proof-hex")?;
            let usage_commitment_hex = normalize_optional_hex_owned(usage_commitment_hex);
            let proof_hex = normalize_optional_hex_owned(proof_hex);
            args.extend([
                "record-usage".to_string(),
                "--domain".to_string(),
                domain,
                "--call-name".to_string(),
                call_name,
                "--duration-ms".to_string(),
                duration_ms.to_string(),
                "--billed-gas".to_string(),
                billed_gas.to_string(),
            ]);
            if let Some(value) = usage_commitment_hex {
                args.extend(["--usage-commitment-hex".to_string(), value]);
            }
            if let Some(value) = proof_hex {
                args.extend(["--proof-hex".to_string(), value]);
            }
        }
    }
    Ok(args)
}

fn run_iroha_json_cli(iroha_bin: &str, iroha_config: &PathBuf, args: &[String]) -> Result<String> {
    let mut command = std::process::Command::new(iroha_bin);
    command
        .arg("-c")
        .arg(iroha_config)
        .arg("--output-format")
        .arg("json");
    for arg in args {
        command.arg(arg);
    }

    let output = command
        .output()
        .with_context(|| format!("spawn iroha cli ({iroha_bin})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "iroha cli command failed (exit={:?}): {}",
            output.status.code(),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(stdout)
}

fn render_join_link(payload: &JoinLinkPayload) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("v".to_string(), payload.version.to_string()),
        ("relay".to_string(), payload.relay.to_string()),
        ("channel".to_string(), hex::encode(payload.channel)),
        (
            "authenticated".to_string(),
            if payload.authenticated { "1" } else { "0" }.to_string(),
        ),
        (
            "insecure".to_string(),
            if payload.insecure { "1" } else { "0" }.to_string(),
        ),
    ];
    if let Some(sni) = payload.server_name.as_deref() {
        params.push(("sni".to_string(), sni.to_string()));
    }
    if let Some(torii) = payload.torii.as_deref() {
        params.push(("torii".to_string(), torii.to_string()));
    }
    if let Some(pay_to) = payload.pay_to.as_deref() {
        params.push(("pay_to".to_string(), pay_to.to_string()));
    }
    if let Some(domain) = payload.kaigi_domain.as_deref() {
        params.push(("kaigi_domain".to_string(), domain.to_string()));
    }
    if let Some(call_name) = payload.kaigi_call_name.as_deref() {
        params.push(("kaigi_call_name".to_string(), call_name.to_string()));
    }
    if let Some(mode) = payload.kaigi_privacy_mode.as_deref() {
        params.push(("kaigi_privacy_mode".to_string(), mode.to_string()));
    }
    if let Some(expires_at_ms) = payload.expires_at_ms {
        params.push(("exp".to_string(), expires_at_ms.to_string()));
    }
    if let Some(nonce_hex) = payload.nonce_hex.as_deref() {
        params.push(("nonce".to_string(), nonce_hex.to_string()));
    }
    if let Some(signature_hex) = payload.signature_hex.as_deref() {
        params.push(("sig".to_string(), signature_hex.to_string()));
    }

    let encoded = params
        .into_iter()
        .map(|(k, v)| format!("{}={}", pct_encode(&k), pct_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("kaigi://join?{encoded}")
}

fn populate_join_link_security_fields(payload: &mut JoinLinkPayload, expires_in_secs: u64) -> Result<()> {
    if payload.version != JOIN_LINK_VERSION_SIGNED {
        return Ok(());
    }
    if expires_in_secs == 0 {
        return Err(anyhow!("--expires-in-secs must be >= 1"));
    }
    if expires_in_secs > MAX_JOIN_LINK_EXPIRES_IN_SECS {
        return Err(anyhow!(
            "--expires-in-secs must be <= {}",
            MAX_JOIN_LINK_EXPIRES_IN_SECS
        ));
    }
    let expires_delta_ms = expires_in_secs
        .checked_mul(1000)
        .ok_or_else(|| anyhow!("--expires-in-secs overflow"))?;
    let expires_at_ms = now_ms()
        .checked_add(expires_delta_ms)
        .ok_or_else(|| anyhow!("join link expiration overflow"))?;

    let mut nonce = [0u8; JOIN_LINK_NONCE_BYTES];
    rand::rng().fill_bytes(&mut nonce);
    payload.expires_at_ms = Some(expires_at_ms);
    payload.nonce_hex = Some(hex::encode(nonce));
    payload.signature_hex = Some(join_link_signature_hex(payload)?);
    Ok(())
}

fn join_link_signature_payload(payload: &JoinLinkPayload) -> Result<String> {
    let expires_at_ms = payload
        .expires_at_ms
        .ok_or_else(|| anyhow!("join link v2 missing exp"))?;
    let nonce_hex = payload
        .nonce_hex
        .as_deref()
        .ok_or_else(|| anyhow!("join link v2 missing nonce"))?;
    Ok(format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        payload.version,
        payload.relay,
        hex::encode(payload.channel),
        if payload.authenticated { "1" } else { "0" },
        if payload.insecure { "1" } else { "0" },
        payload.server_name.as_deref().unwrap_or_default(),
        payload.torii.as_deref().unwrap_or_default(),
        payload.pay_to.as_deref().unwrap_or_default(),
        payload.kaigi_domain.as_deref().unwrap_or_default(),
        payload.kaigi_call_name.as_deref().unwrap_or_default(),
        payload.kaigi_privacy_mode.as_deref().unwrap_or_default(),
        expires_at_ms,
        normalize_hex_arg(nonce_hex)
    ))
}

fn join_link_signature_hex(payload: &JoinLinkPayload) -> Result<String> {
    let canonical = join_link_signature_payload(payload)?;
    Ok(deterministic_signature_hex("join_link_v2", &canonical))
}

fn validate_join_link_security(payload: &JoinLinkPayload, consume_nonce_replay: bool) -> Result<()> {
    match payload.version {
        JOIN_LINK_VERSION_LEGACY => {
            if payload.expires_at_ms.is_some()
                || payload.nonce_hex.is_some()
                || payload.signature_hex.is_some()
            {
                return Err(anyhow!(
                    "join link v1 must not include exp/nonce/sig fields"
                ));
            }
        }
        JOIN_LINK_VERSION_SIGNED => {
            let expires_at_ms = payload
                .expires_at_ms
                .ok_or_else(|| anyhow!("join link v2 missing exp"))?;
            let nonce_hex = payload
                .nonce_hex
                .as_deref()
                .ok_or_else(|| anyhow!("join link v2 missing nonce"))?;
            let signature_hex = payload
                .signature_hex
                .as_deref()
                .ok_or_else(|| anyhow!("join link v2 missing sig"))?;

            let now = now_ms();
            if expires_at_ms <= now {
                return Err(anyhow!("join link expired"));
            }
            let max_delta_ms = MAX_JOIN_LINK_EXPIRES_IN_SECS
                .checked_mul(1000)
                .ok_or_else(|| anyhow!("join link max expiry overflow"))?;
            if expires_at_ms > now.saturating_add(max_delta_ms) {
                return Err(anyhow!(
                    "join link exp exceeds max future window ({} seconds)",
                    MAX_JOIN_LINK_EXPIRES_IN_SECS
                ));
            }
            if strip_hex_prefix(nonce_hex).len() != JOIN_LINK_NONCE_BYTES * 2
                || hex::decode(strip_hex_prefix(nonce_hex)).is_err()
            {
                return Err(anyhow!(
                    "join link nonce must be {}-byte hex",
                    JOIN_LINK_NONCE_BYTES
                ));
            }
            if strip_hex_prefix(signature_hex).len() != 64
                || hex::decode(strip_hex_prefix(signature_hex)).is_err()
            {
                return Err(anyhow!("join link sig must be 32-byte hex"));
            }
            let expected = join_link_signature_hex(payload)?;
            if expected != normalize_hex_arg(signature_hex) {
                return Err(anyhow!("join link signature verification failed"));
            }
            if consume_nonce_replay {
                register_join_link_nonce_once(nonce_hex, expires_at_ms)?;
            }
        }
        version => return Err(anyhow!("unsupported join link version: {version}")),
    }
    Ok(())
}

fn seen_join_link_nonce_cache() -> &'static StdMutex<HashMap<String, u64>> {
    SEEN_JOIN_LINK_NONCES.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn register_join_link_nonce_once(nonce_hex: &str, expires_at_ms: u64) -> Result<()> {
    let now = now_ms();
    let mut guard = seen_join_link_nonce_cache()
        .lock()
        .map_err(|_| anyhow!("join link nonce cache lock poisoned"))?;
    guard.retain(|_, exp| *exp > now);
    let normalized = normalize_hex_arg(nonce_hex);
    if guard.contains_key(&normalized) {
        return Err(anyhow!("join link replay detected"));
    }
    if guard.len() >= MAX_JOIN_LINK_NONCE_CACHE_ENTRIES {
        return Err(anyhow!(
            "join link nonce cache is full (max {}), retry after active links expire",
            MAX_JOIN_LINK_NONCE_CACHE_ENTRIES
        ));
    }
    guard.insert(normalized, expires_at_ms);
    Ok(())
}

fn parse_join_link_with_mode(link: &str, consume_nonce_replay: bool) -> Result<JoinLinkPayload> {
    let (scheme, query) = link
        .trim()
        .split_once('?')
        .ok_or_else(|| anyhow!("join link must contain query params"))?;
    if !scheme.eq_ignore_ascii_case("kaigi://join") {
        return Err(anyhow!("unsupported join link scheme: {scheme}"));
    }

    let mut relay: Option<SocketAddr> = None;
    let mut channel: Option<[u8; 32]> = None;
    let mut authenticated = false;
    let mut insecure = false;
    let mut server_name: Option<String> = None;
    let mut torii: Option<String> = None;
    let mut pay_to: Option<String> = None;
    let mut kaigi_domain: Option<String> = None;
    let mut kaigi_call_name: Option<String> = None;
    let mut kaigi_privacy_mode: Option<String> = None;
    let mut expires_at_ms: Option<u64> = None;
    let mut nonce_hex: Option<String> = None;
    let mut signature_hex: Option<String> = None;
    let mut version = JOIN_LINK_VERSION_LEGACY;

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_key, raw_value) = pair.split_once('=').map_or((pair, ""), |(k, v)| (k, v));
        let key = pct_decode(raw_key)?;
        let value = pct_decode(raw_value)?;
        match key.as_str() {
            "v" => {
                version = value
                    .parse::<u8>()
                    .map_err(|_| anyhow!("invalid join link version in join link"))?;
            }
            "relay" => {
                relay = Some(
                    value
                        .parse::<SocketAddr>()
                        .map_err(|_| anyhow!("invalid relay socket address in join link"))?,
                );
            }
            "channel" => channel = Some(decode_hex_32(&value)?),
            "authenticated" => {
                authenticated = parse_boolish(&value)
                    .ok_or_else(|| anyhow!("invalid authenticated flag in join link"))?;
            }
            "insecure" => {
                insecure = parse_boolish(&value)
                    .ok_or_else(|| anyhow!("invalid insecure flag in join link"))?;
            }
            "sni" => {
                if !value.is_empty() {
                    server_name = Some(value);
                }
            }
            "torii" => {
                if !value.is_empty() {
                    torii = Some(value);
                }
            }
            "pay_to" => {
                if !value.is_empty() {
                    pay_to = Some(value);
                }
            }
            "kaigi_domain" => {
                if !value.is_empty() {
                    kaigi_domain = Some(value);
                }
            }
            "kaigi_call_name" => {
                if !value.is_empty() {
                    kaigi_call_name = Some(value);
                }
            }
            "kaigi_privacy_mode" => {
                if !value.is_empty() {
                    kaigi_privacy_mode = Some(value);
                }
            }
            "exp" => {
                if !value.is_empty() {
                    expires_at_ms = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| anyhow!("invalid exp in join link"))?,
                    );
                }
            }
            "nonce" => {
                if !value.is_empty() {
                    nonce_hex = Some(value);
                }
            }
            "sig" => {
                if !value.is_empty() {
                    signature_hex = Some(value);
                }
            }
            _ => {}
        }
    }

    if kaigi_domain.is_some() != kaigi_call_name.is_some() {
        return Err(anyhow!(
            "join link must include both kaigi_domain and kaigi_call_name (or neither)"
        ));
    }
    validate_privacy_mode_arg(kaigi_privacy_mode.as_deref())?;
    let payload = JoinLinkPayload {
        version,
        relay: relay.ok_or_else(|| anyhow!("join link missing relay"))?,
        channel: channel.ok_or_else(|| anyhow!("join link missing channel"))?,
        authenticated,
        insecure,
        server_name,
        torii,
        pay_to,
        kaigi_domain,
        kaigi_call_name,
        kaigi_privacy_mode: normalize_privacy_mode(kaigi_privacy_mode),
        expires_at_ms,
        nonce_hex: nonce_hex.map(|value| normalize_hex_arg(&value)),
        signature_hex: signature_hex.map(|value| normalize_hex_arg(&value)),
    };
    validate_join_link_security(&payload, consume_nonce_replay)?;
    Ok(payload)
}

fn parse_join_link(link: &str) -> Result<JoinLinkPayload> {
    parse_join_link_with_mode(link, true)
}

fn parse_join_link_for_inspection(link: &str) -> Result<JoinLinkPayload> {
    parse_join_link_with_mode(link, false)
}

fn pct_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
            out.push(char::from(b"0123456789ABCDEF"[(b & 0x0F) as usize]));
        }
    }
    out
}

fn pct_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(anyhow!("truncated percent-encoding"));
                }
                let hi =
                    hex_nibble(bytes[i + 1]).ok_or_else(|| anyhow!("invalid percent-encoding"))?;
                let lo =
                    hex_nibble(bytes[i + 2]).ok_or_else(|| anyhow!("invalid percent-encoding"))?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).context("join link percent-decoded bytes are not UTF-8")
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + b - b'a'),
        b'A'..=b'F' => Some(10 + b - b'A'),
        _ => None,
    }
}

fn parse_boolish(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn has_auto_kaigi_lifecycle_overrides(args: &RoomChatArgs) -> bool {
    args.kaigi_iroha_bin.is_some()
        || args.kaigi_participant.is_some()
        || args.kaigi_join_commitment_hex.is_some()
        || args.kaigi_join_commitment_alias.is_some()
        || args.kaigi_join_nullifier_hex.is_some()
        || args.kaigi_leave_commitment_hex.is_some()
        || args.kaigi_leave_nullifier_hex.is_some()
        || args.kaigi_nullifier_issued_at_ms.is_some()
        || args.kaigi_roster_root_hex.is_some()
        || args.kaigi_join_proof_hex.is_some()
        || args.kaigi_leave_proof_hex.is_some()
        || args.kaigi_record_usage
        || args.kaigi_billed_gas > 0
        || args.kaigi_usage_commitment_hex.is_some()
        || args.kaigi_usage_proof_hex.is_some()
}

fn build_auto_kaigi_lifecycle(args: &RoomChatArgs) -> Result<Option<AutoKaigiLifecycle>> {
    if args.kaigi_iroha_config.is_none() {
        if has_auto_kaigi_lifecycle_overrides(args) {
            return Err(anyhow!(
                "--kaigi-iroha-config is required when using Kaigi lifecycle mirroring flags"
            ));
        }
        return Ok(None);
    }

    let provided = [
        args.kaigi_domain.is_some(),
        args.kaigi_call_name.is_some(),
        args.kaigi_participant.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();

    if provided != 3 {
        return Err(anyhow!(
            "for Kaigi lifecycle mirroring, set all of: --kaigi-iroha-config --kaigi-domain --kaigi-call-name --kaigi-participant"
        ));
    }
    validate_commitment_alias_dependency(
        args.kaigi_join_commitment_hex.as_deref(),
        args.kaigi_join_commitment_alias.as_deref(),
        "--kaigi-join-commitment-hex",
        "--kaigi-join-commitment-alias",
    )?;
    validate_optional_hex_arg(
        args.kaigi_join_commitment_hex.as_deref(),
        "--kaigi-join-commitment-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_join_nullifier_hex.as_deref(),
        "--kaigi-join-nullifier-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_leave_commitment_hex.as_deref(),
        "--kaigi-leave-commitment-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_leave_nullifier_hex.as_deref(),
        "--kaigi-leave-nullifier-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_roster_root_hex.as_deref(),
        "--kaigi-roster-root-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_join_proof_hex.as_deref(),
        "--kaigi-join-proof-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_leave_proof_hex.as_deref(),
        "--kaigi-leave-proof-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_usage_commitment_hex.as_deref(),
        "--kaigi-usage-commitment-hex",
    )?;
    validate_optional_hex_arg(
        args.kaigi_usage_proof_hex.as_deref(),
        "--kaigi-usage-proof-hex",
    )?;
    if args.kaigi_nullifier_issued_at_ms.is_some()
        && args.kaigi_join_nullifier_hex.is_none()
        && args.kaigi_leave_nullifier_hex.is_none()
    {
        return Err(anyhow!(
            "--kaigi-nullifier-issued-at-ms requires --kaigi-join-nullifier-hex or --kaigi-leave-nullifier-hex"
        ));
    }
    if !args.kaigi_record_usage
        && (args.kaigi_billed_gas > 0
            || args.kaigi_usage_commitment_hex.is_some()
            || args.kaigi_usage_proof_hex.is_some())
    {
        return Err(anyhow!(
            "--kaigi-billed-gas/--kaigi-usage-commitment-hex/--kaigi-usage-proof-hex require --kaigi-record-usage"
        ));
    }

    Ok(Some(AutoKaigiLifecycle {
        iroha_bin: args
            .kaigi_iroha_bin
            .clone()
            .unwrap_or_else(|| ledger::default_iroha_bin().to_string()),
        iroha_config: args
            .kaigi_iroha_config
            .clone()
            .ok_or_else(|| anyhow!("missing --kaigi-iroha-config"))?,
        domain: args
            .kaigi_domain
            .clone()
            .ok_or_else(|| anyhow!("missing --kaigi-domain"))?,
        call_name: args
            .kaigi_call_name
            .clone()
            .ok_or_else(|| anyhow!("missing --kaigi-call-name"))?,
        participant: args
            .kaigi_participant
            .clone()
            .ok_or_else(|| anyhow!("missing --kaigi-participant"))?,
        join_commitment_hex: normalize_optional_hex_owned(args.kaigi_join_commitment_hex.clone()),
        join_commitment_alias: args.kaigi_join_commitment_alias.clone(),
        join_nullifier_hex: normalize_optional_hex_owned(args.kaigi_join_nullifier_hex.clone()),
        leave_commitment_hex: normalize_optional_hex_owned(args.kaigi_leave_commitment_hex.clone()),
        leave_nullifier_hex: normalize_optional_hex_owned(args.kaigi_leave_nullifier_hex.clone()),
        nullifier_issued_at_ms: args.kaigi_nullifier_issued_at_ms,
        roster_root_hex: normalize_optional_hex_owned(args.kaigi_roster_root_hex.clone()),
        join_proof_hex: normalize_optional_hex_owned(args.kaigi_join_proof_hex.clone()),
        leave_proof_hex: normalize_optional_hex_owned(args.kaigi_leave_proof_hex.clone()),
        record_usage: args.kaigi_record_usage,
        billed_gas: args.kaigi_billed_gas,
        usage_commitment_hex: normalize_optional_hex_owned(args.kaigi_usage_commitment_hex.clone()),
        usage_proof_hex: normalize_optional_hex_owned(args.kaigi_usage_proof_hex.clone()),
    }))
}

fn run_auto_kaigi_join(lc: &AutoKaigiLifecycle) -> Result<String> {
    let mut cmd = vec![
        "app".to_string(),
        "kaigi".to_string(),
        "join".to_string(),
        "--domain".to_string(),
        lc.domain.clone(),
        "--call-name".to_string(),
        lc.call_name.clone(),
        "--participant".to_string(),
        lc.participant.clone(),
    ];
    if let Some(value) = lc.join_commitment_hex.as_deref() {
        cmd.extend(["--commitment-hex".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.join_commitment_alias.as_deref() {
        cmd.extend(["--commitment-alias".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.join_nullifier_hex.as_deref() {
        cmd.extend(["--nullifier-hex".to_string(), value.to_string()]);
    }
    if lc.join_nullifier_hex.is_some()
        && let Some(value) = lc.nullifier_issued_at_ms
    {
        cmd.extend(["--nullifier-issued-at-ms".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.roster_root_hex.as_deref() {
        cmd.extend(["--roster-root-hex".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.join_proof_hex.as_deref() {
        cmd.extend(["--proof-hex".to_string(), value.to_string()]);
    }
    run_iroha_json_cli(&lc.iroha_bin, &lc.iroha_config, &cmd)
}

fn run_auto_kaigi_leave(lc: &AutoKaigiLifecycle) -> Result<String> {
    let mut cmd = vec![
        "app".to_string(),
        "kaigi".to_string(),
        "leave".to_string(),
        "--domain".to_string(),
        lc.domain.clone(),
        "--call-name".to_string(),
        lc.call_name.clone(),
        "--participant".to_string(),
        lc.participant.clone(),
    ];
    if let Some(value) = lc.leave_commitment_hex.as_deref() {
        cmd.extend(["--commitment-hex".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.leave_nullifier_hex.as_deref() {
        cmd.extend(["--nullifier-hex".to_string(), value.to_string()]);
    }
    if lc.leave_nullifier_hex.is_some()
        && let Some(value) = lc.nullifier_issued_at_ms
    {
        cmd.extend(["--nullifier-issued-at-ms".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.roster_root_hex.as_deref() {
        cmd.extend(["--roster-root-hex".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.leave_proof_hex.as_deref() {
        cmd.extend(["--proof-hex".to_string(), value.to_string()]);
    }
    run_iroha_json_cli(&lc.iroha_bin, &lc.iroha_config, &cmd)
}

fn run_auto_kaigi_end(lc: &AutoKaigiLifecycle, ended_at_ms: u64) -> Result<String> {
    let cmd = vec![
        "app".to_string(),
        "kaigi".to_string(),
        "end".to_string(),
        "--domain".to_string(),
        lc.domain.clone(),
        "--call-name".to_string(),
        lc.call_name.clone(),
        "--ended-at-ms".to_string(),
        ended_at_ms.to_string(),
    ];
    run_iroha_json_cli(&lc.iroha_bin, &lc.iroha_config, &cmd)
}

fn run_auto_kaigi_record_usage(lc: &AutoKaigiLifecycle, duration_ms: u64) -> Result<String> {
    let mut cmd = vec![
        "app".to_string(),
        "kaigi".to_string(),
        "record-usage".to_string(),
        "--domain".to_string(),
        lc.domain.clone(),
        "--call-name".to_string(),
        lc.call_name.clone(),
        "--duration-ms".to_string(),
        duration_ms.to_string(),
        "--billed-gas".to_string(),
        lc.billed_gas.to_string(),
    ];
    if let Some(value) = lc.usage_commitment_hex.as_deref() {
        cmd.extend(["--usage-commitment-hex".to_string(), value.to_string()]);
    }
    if let Some(value) = lc.usage_proof_hex.as_deref() {
        cmd.extend(["--proof-hex".to_string(), value.to_string()]);
    }
    run_iroha_json_cli(&lc.iroha_bin, &lc.iroha_config, &cmd)
}

fn validate_commitment_alias_dependency(
    commitment_hex: Option<&str>,
    commitment_alias: Option<&str>,
    commitment_arg: &str,
    alias_arg: &str,
) -> Result<()> {
    if commitment_alias.is_some() && commitment_hex.is_none() {
        return Err(anyhow!("{alias_arg} requires {commitment_arg}"));
    }
    Ok(())
}

fn validate_nullifier_timestamp_dependency(
    nullifier_hex: Option<&str>,
    nullifier_issued_at_ms: Option<u64>,
    nullifier_arg: &str,
    nullifier_issued_at_ms_arg: &str,
) -> Result<()> {
    if nullifier_issued_at_ms.is_some() && nullifier_hex.is_none() {
        return Err(anyhow!(
            "{nullifier_issued_at_ms_arg} requires {nullifier_arg}"
        ));
    }
    Ok(())
}

fn validate_optional_hex_arg(value: Option<&str>, arg_name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let normalized = strip_hex_prefix(value.trim());
    if normalized.is_empty()
        || !normalized.len().is_multiple_of(2)
        || hex::decode(normalized).is_err()
    {
        return Err(anyhow!("{arg_name} must be valid non-empty hex"));
    }
    Ok(())
}

fn normalize_optional_hex_owned(value: Option<String>) -> Option<String> {
    value.map(|raw| normalize_hex_arg(&raw))
}

fn normalize_room_policy_arg(room_policy: String) -> Result<String> {
    let normalized = room_policy.trim().to_ascii_lowercase();
    if normalized == "public" {
        return Ok(normalized);
    }
    if normalized == "authenticated" || normalized == "auth" {
        return Ok("authenticated".to_string());
    }
    Err(anyhow!(
        "unsupported room policy `{room_policy}`; expected public|authenticated|auth"
    ))
}

fn classify_host_state(host_participant_id: Option<&str>, local_participant_id: &str) -> u8 {
    match host_participant_id {
        Some(host_id) if host_id == local_participant_id => HOST_STATE_SELF,
        Some(_) => HOST_STATE_OTHER,
        None => HOST_STATE_UNKNOWN,
    }
}

fn host_state_label(state: u8) -> &'static str {
    match state {
        HOST_STATE_SELF => "host",
        HOST_STATE_OTHER => "participant",
        _ => "unknown",
    }
}

fn end_command_gate_message(host_state: u8) -> Option<&'static str> {
    match host_state {
        HOST_STATE_SELF => None,
        HOST_STATE_OTHER => Some("error: /end is host-only"),
        _ => Some("error: host role unknown; wait for room_config before /end"),
    }
}

fn detect_hdr_display() -> bool {
    if let Ok(value) = std::env::var("KAIGI_HDR_DISPLAY")
        && let Some(parsed) = parse_boolish(&value)
    {
        return parsed;
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = OsCommand::new("system_profiler")
            .arg("SPDisplaysDataType")
            .arg("-json")
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
            if text.contains("hdr") || text.contains("high dynamic range") {
                return true;
            }
        }
    }

    false
}

async fn relay_echo(args: RelayEchoArgs) -> Result<()> {
    let handshake = if let Some(torii) = args.torii.as_deref() {
        fetch_handshake_params_from_torii(torii).await?
    } else if args.descriptor_commit_hex.is_some()
        || args.client_capabilities_hex.is_some()
        || args.relay_capabilities_hex.is_some()
        || args.kem_id.is_some()
        || args.sig_id.is_some()
        || args.resume_hash_hex.is_some()
    {
        let descriptor_commit_hex = args
            .descriptor_commit_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--descriptor-commit-hex is required"))?;
        let client_capabilities_hex = args
            .client_capabilities_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--client-capabilities-hex is required"))?;
        let relay_capabilities_hex = args
            .relay_capabilities_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--relay-capabilities-hex is required"))?;
        let kem_id = args.kem_id.ok_or_else(|| anyhow!("--kem-id is required"))?;
        let sig_id = args.sig_id.ok_or_else(|| anyhow!("--sig-id is required"))?;

        HandshakeParams {
            descriptor_commit: decode_hex_32(descriptor_commit_hex)?,
            client_capabilities: decode_hex_vec(client_capabilities_hex)?,
            relay_capabilities: decode_hex_vec(relay_capabilities_hex)?,
            kem_id,
            sig_id,
            resume_hash: args
                .resume_hash_hex
                .as_deref()
                .map(decode_hex_vec)
                .transpose()?,
        }
    } else {
        HandshakeParams::fixture_defaults()
    };

    let handshake_prelude_frame = args
        .handshake_prelude_hex
        .as_deref()
        .map(decode_hex_vec)
        .transpose()?;

    let opts = RelayConnectOptions {
        relay_addr: args.relay,
        server_name: args.server_name.unwrap_or_else(|| "localhost".to_string()),
        insecure: args.insecure,
        ca_cert_pem_path: args.ca_cert_pem_path,
        handshake_prelude_frame,
        handshake,
    };

    let session = connect_and_handshake(opts).await?;
    info!(
        transcript = %hex::encode(session.secrets.transcript_hash),
        "handshake complete"
    );

    let channel_id = decode_hex_32(&args.channel)?;
    let (mut send, mut recv) =
        open_kaigi_stream(&session.connection, channel_id, args.authenticated)
            .await
            .context("open kaigi stream")?;

    let participant_id = {
        let mut bytes = [0u8; 8];
        rand::rng().fill_bytes(&mut bytes);
        format!("echo-{}", hex::encode(bytes))
    };

    let text = if let Some(hex) = args.message_hex.as_deref() {
        let bytes = decode_hex_vec(hex)?;
        match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(_) => format!("0x{hex}"),
        }
    } else {
        args.message.unwrap_or_else(|| "ping".to_string())
    };

    let hello = KaigiFrame::Hello(HelloFrame {
        protocol_version: PROTOCOL_VERSION,
        participant_id: participant_id.clone(),
        display_name: Some("relay-echo".to_string()),
        mic_enabled: false,
        video_enabled: false,
        screen_share_enabled: false,
        hdr_display: false,
        hdr_capture: false,
    });
    send.write_all(&encode_framed(&hello)?)
        .await
        .context("send hello")?;

    if !text.is_empty() {
        let chat = KaigiFrame::Chat(ChatFrame {
            sent_at_ms: now_ms(),
            from_participant_id: participant_id.clone(),
            from_display_name: Some("relay-echo".to_string()),
            text,
        });
        send.write_all(&encode_framed(&chat)?)
            .await
            .context("send chat")?;
    }

    let ping = KaigiFrame::Ping(PingFrame { nonce: now_ms() });
    send.write_all(&encode_framed(&ping)?)
        .await
        .context("send ping")?;
    send.finish().context("finish payload stream")?;

    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match recv.read(&mut buf).await.context("recv")? {
            Some(n) if n > 0 => {
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.try_next()? {
                    println!("frame={frame:?}");
                }
            }
            _ => break,
        }
    }

    Ok(())
}

async fn xor_transfer(args: XorTransferArgs) -> Result<()> {
    let iroha_bin = args
        .iroha_bin
        .unwrap_or_else(|| ledger::default_iroha_bin().to_string());
    let iroha_config = args.iroha_config;
    let from = args.from;
    let to = args.to;
    let asset_def = args.asset_def;
    let amount_nano_xor = args.amount_nano_xor;
    let blocking = args.blocking;

    let tx_hash = tokio::task::spawn_blocking(move || {
        ledger::transfer_xor_nano_via_cli(
            &iroha_bin,
            &iroha_config,
            from.as_deref(),
            &to,
            amount_nano_xor,
            &asset_def,
            blocking,
        )
    })
    .await
    .context("xor transfer task")??;

    println!("tx_hash={tx_hash}");
    Ok(())
}

async fn room_chat(args: RoomChatArgs) -> Result<()> {
    let session_started_at_ms = now_ms();
    let mut args = args;

    let join_link = args
        .join_link
        .as_deref()
        .map(parse_join_link_for_inspection)
        .transpose()?;
    apply_room_chat_join_link_defaults(&mut args, join_link.as_ref());
    apply_room_chat_env_defaults(&mut args);
    args.kaigi_privacy_mode = normalize_privacy_mode(args.kaigi_privacy_mode.take());
    validate_privacy_mode_arg(args.kaigi_privacy_mode.as_deref())?;
    let anonymous_mode = is_anonymous_mode_enabled(args.kaigi_privacy_mode.as_deref());
    validate_anonymous_escrow_settings(&args, anonymous_mode)?;
    let auto_kaigi_lifecycle = build_auto_kaigi_lifecycle(&args)?;

    let relay_addr = args
        .relay
        .ok_or_else(|| anyhow!("--relay is required unless --join-link is provided"))?;
    let channel_hex = args
        .channel
        .clone()
        .ok_or_else(|| anyhow!("--channel is required unless --join-link is provided"))?;
    let server_name = args
        .server_name
        .clone()
        .unwrap_or_else(|| "localhost".to_string());
    let authenticated = args.authenticated;
    let insecure = args.insecure;

    validate_nexus_routing_requirement(args.allow_local_handshake, args.torii.as_deref())?;
    if let Some(link) = args.join_link.as_deref() {
        // Consume nonce replay window only after argument/default validation succeeds.
        let _ = parse_join_link(link)?;
    }

    let handshake = if let Some(torii) = args.torii.as_deref() {
        fetch_handshake_params_from_torii(torii).await?
    } else if args.descriptor_commit_hex.is_some()
        || args.client_capabilities_hex.is_some()
        || args.relay_capabilities_hex.is_some()
        || args.kem_id.is_some()
        || args.sig_id.is_some()
        || args.resume_hash_hex.is_some()
    {
        let descriptor_commit_hex = args
            .descriptor_commit_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--descriptor-commit-hex is required"))?;
        let client_capabilities_hex = args
            .client_capabilities_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--client-capabilities-hex is required"))?;
        let relay_capabilities_hex = args
            .relay_capabilities_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--relay-capabilities-hex is required"))?;
        let kem_id = args.kem_id.ok_or_else(|| anyhow!("--kem-id is required"))?;
        let sig_id = args.sig_id.ok_or_else(|| anyhow!("--sig-id is required"))?;

        HandshakeParams {
            descriptor_commit: decode_hex_32(descriptor_commit_hex)?,
            client_capabilities: decode_hex_vec(client_capabilities_hex)?,
            relay_capabilities: decode_hex_vec(relay_capabilities_hex)?,
            kem_id,
            sig_id,
            resume_hash: args
                .resume_hash_hex
                .as_deref()
                .map(decode_hex_vec)
                .transpose()?,
        }
    } else {
        HandshakeParams::fixture_defaults()
    };

    let handshake_prelude_frame = args
        .handshake_prelude_hex
        .as_deref()
        .map(decode_hex_vec)
        .transpose()?;

    let opts = RelayConnectOptions {
        relay_addr,
        server_name,
        insecure,
        ca_cert_pem_path: args.ca_cert_pem_path.clone(),
        handshake_prelude_frame,
        handshake,
    };

    let session = connect_and_handshake(opts).await?;
    info!(
        transcript = %hex::encode(session.secrets.transcript_hash),
        "handshake complete"
    );

    let channel_id = decode_hex_32(&channel_hex)?;
    let (mut send, mut recv) = open_kaigi_stream(&session.connection, channel_id, authenticated)
        .await
        .context("open kaigi stream")?;

    if anonymous_mode {
        let participant_handle = args
            .participant_id
            .clone()
            .unwrap_or_else(random_anon_handle);
        validate_anonymous_participant_handle(&participant_handle)?;
        println!("anonymous_mode=enabled privacy_mode=zk");
        if let Some(ref lc) = auto_kaigi_lifecycle {
            let lc = lc.clone();
            match tokio::task::spawn_blocking(move || run_auto_kaigi_join(&lc)).await {
                Ok(Ok(payload)) => {
                    println!("kaigi_join={payload}");
                }
                Ok(Err(err)) => {
                    eprintln!("kaigi lifecycle join failed: {err}");
                }
                Err(err) => {
                    eprintln!("kaigi lifecycle join task failed: {err}");
                }
            }
        }
        let end_requested = run_room_chat_anonymous(&args, participant_handle, send, recv).await?;
        finalize_auto_kaigi_lifecycle(auto_kaigi_lifecycle, end_requested, session_started_at_ms)
            .await;
        return Ok(());
    }

    let participant_id = args.participant_id.clone().unwrap_or_else(|| {
        let mut bytes = [0u8; 8];
        rand::rng().fill_bytes(&mut bytes);
        format!("p-{}", hex::encode(bytes))
    });

    let auto_hdr_display = if args.no_hdr_auto {
        false
    } else {
        detect_hdr_display()
    };
    let hdr_display = args.hdr_display || auto_hdr_display;
    let hdr_capture = args.hdr_capture;

    let hello = KaigiFrame::Hello(HelloFrame {
        protocol_version: PROTOCOL_VERSION,
        participant_id: participant_id.clone(),
        display_name: args.display_name.clone(),
        mic_enabled: false,
        video_enabled: false,
        screen_share_enabled: false,
        hdr_display,
        hdr_capture,
    });

    let (out_tx, mut out_rx) = mpsc::channel::<KaigiFrame>(256);
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let bytes = encode_framed(&frame)?;
            send.write_all(&bytes).await.context("send frame")?;
        }
        send.finish().context("finish send stream")?;
        Ok::<(), anyhow::Error>(())
    });

    out_tx
        .send(hello)
        .await
        .map_err(|_| anyhow!("send channel closed"))?;

    if let Some(ref lc) = auto_kaigi_lifecycle {
        let lc = lc.clone();
        match tokio::task::spawn_blocking(move || run_auto_kaigi_join(&lc)).await {
            Ok(Ok(payload)) => {
                println!("kaigi_join={payload}");
            }
            Ok(Err(err)) => {
                eprintln!("kaigi lifecycle join failed: {err}");
            }
            Err(err) => {
                eprintln!("kaigi lifecycle join task failed: {err}");
            }
        }
    }

    println!("connected participant_id={participant_id}");
    if auto_hdr_display && !args.hdr_display {
        println!("hdr_display auto-detected=true");
    }
    println!(
        "commands: /mic on|off, /video on|off, /share on|off, /rate <nano_per_min>, /maxshare <u8>, /mute <id>, /muteall, /videooff <id>, /shareoff <id>, /kick <id>, /admit <id>, /deny <id>, /cohost <id>, /uncohost <id>, /host <id>, /lock on|off, /waiting on|off, /guests on|off, /recordlocal on|off, /e2eerequired on|off, /maxparticipants <u32>, /devicecap, /profile sdr|hdr, /recordstart, /recordstop, /e2eekey <epoch>, /e2eeack <epoch>, /end (host), /pay <nano>, /quit"
    );
    println!("audio path is active on join; no separate connect-audio step");

    let pay_auto_enabled = is_pay_auto_enabled(&args);

    if pay_auto_enabled && args.pay_rate_per_minute_nano > 0 {
        return Err(anyhow!(
            "automatic pay-rate tracking requires --pay-rate-per-minute-nano to be 0; use --no-pay-auto for fixed-rate mode"
        ));
    }

    let rate_per_minute_nano = Arc::new(AtomicU64::new(args.pay_rate_per_minute_nano));
    let policy_state = Arc::new(Mutex::new(LocalSessionPolicyState::default()));

    #[derive(Clone)]
    struct PayLedgerCli {
        iroha_bin: String,
        iroha_config: PathBuf,
        from: Option<String>,
        to: String,
        asset_def: String,
        blocking: bool,
    }

    let pay_loop_enabled = args.pay_rate_per_minute_nano > 0 || pay_auto_enabled;
    let pay_ledger = match (args.pay_iroha_config.clone(), args.pay_to.clone()) {
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(anyhow!(
                "--pay-iroha-config and --pay-to must be set together"
            ));
        }
        (Some(iroha_config), Some(to)) => {
            if !pay_loop_enabled {
                return Err(anyhow!(
                    "--pay-rate-per-minute-nano or --pay-auto is required when using ledger payments"
                ));
            }
            let iroha_bin = args
                .pay_iroha_bin
                .clone()
                .unwrap_or_else(|| ledger::default_iroha_bin().to_string());
            Some(PayLedgerCli {
                iroha_bin,
                iroha_config,
                from: args.pay_from.clone(),
                to,
                asset_def: args.pay_asset_def.clone(),
                blocking: args.pay_blocking,
            })
        }
    };

    if pay_loop_enabled && pay_ledger.is_none() && !args.allow_unsettled_payments {
        return Err(anyhow!(
            "ledger-backed XOR payments are required: set --pay-iroha-config and --pay-to, or pass --allow-unsettled-payments for dev-only frame signalling"
        ));
    }

    if pay_loop_enabled {
        let tx = out_tx.clone();
        let interval = Duration::from_secs(args.pay_interval_secs.max(1));
        let pay_ledger = pay_ledger.clone();
        let rate_per_minute_nano = rate_per_minute_nano.clone();
        tokio::spawn(async move {
            let mut last_ms = now_ms();
            let mut remainder: u128 = 0;
            let mut total_billed_nano: u128 = 0;
            let mut total_paid_nano: u128 = 0;
            loop {
                tokio::time::sleep(interval).await;
                let now = now_ms();
                let elapsed_ms = now.saturating_sub(last_ms);
                last_ms = now;

                let rate = rate_per_minute_nano.load(Ordering::Relaxed) as u128;
                let numerator = rate
                    .saturating_mul(elapsed_ms as u128)
                    .saturating_add(remainder);
                let delta_billed = numerator / 60_000u128;
                remainder = numerator % 60_000u128;
                total_billed_nano = total_billed_nano.saturating_add(delta_billed);

                if total_billed_nano > total_paid_nano {
                    let delta = (total_billed_nano - total_paid_nano).min(u64::MAX as u128) as u64;
                    let tx_hash_hex = if let Some(ref ledger_cfg) = pay_ledger {
                        let iroha_bin = ledger_cfg.iroha_bin.clone();
                        let iroha_config = ledger_cfg.iroha_config.clone();
                        let from = ledger_cfg.from.clone();
                        let to = ledger_cfg.to.clone();
                        let asset_def = ledger_cfg.asset_def.clone();
                        let blocking = ledger_cfg.blocking;
                        match tokio::task::spawn_blocking(move || {
                            ledger::transfer_xor_nano_via_cli(
                                &iroha_bin,
                                &iroha_config,
                                from.as_deref(),
                                &to,
                                delta,
                                &asset_def,
                                blocking,
                            )
                        })
                        .await
                        {
                            Ok(Ok(hash)) => Some(hash),
                            Ok(Err(err)) => {
                                eprintln!("payment transfer failed: {err}");
                                continue;
                            }
                            Err(err) => {
                                eprintln!("payment transfer task failed: {err}");
                                continue;
                            }
                        }
                    } else {
                        None
                    };

                    let frame = KaigiFrame::Payment(PaymentFrame {
                        sent_at_ms: now,
                        amount_nano_xor: delta,
                        tx_hash_hex,
                    });
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                    total_paid_nano = total_paid_nano.saturating_add(u128::from(delta));
                }
            }
        });
    }

    let rate_for_reader = rate_per_minute_nano.clone();
    let host_state = Arc::new(AtomicU8::new(HOST_STATE_UNKNOWN));
    let host_state_for_reader = host_state.clone();
    let pay_auto = pay_auto_enabled;
    let fixed_rate = args.pay_rate_per_minute_nano > 0;
    let participant_id_for_reader = participant_id.clone();
    let policy_state_for_reader = policy_state.clone();
    let reader = tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        let mut decoder = FrameDecoder::new();
        loop {
            match recv.read(&mut buf).await.context("recv")? {
                Some(n) if n > 0 => {
                    decoder.push(&buf[..n]);
                    while let Some(frame) = decoder.try_next()? {
                        match frame {
                            KaigiFrame::Roster(roster) => {
                                println!(
                                    "roster at_ms={} count={}",
                                    roster.at_ms,
                                    roster.participants.len()
                                );
                                for p in roster.participants {
                                    println!(
                                        "  {} ({}) mic={} video={} share={}",
                                        p.participant_id,
                                        p.display_name.as_deref().unwrap_or(""),
                                        p.mic_enabled,
                                        p.video_enabled,
                                        p.screen_share_enabled
                                    );
                                }
                            }
                            KaigiFrame::Event(ev) => {
                                println!("event={ev:?}");
                            }
                            KaigiFrame::Chat(chat) => {
                                let name = chat.from_display_name.as_deref().unwrap_or("");
                                println!("[{} {}] {}", chat.from_participant_id, name, chat.text);
                            }
                            KaigiFrame::RoomConfig(cfg) => {
                                println!(
                                    "room_config updated_at_ms={} host={} rate_per_minute_nano={} grace_secs={} max_screen_shares={}",
                                    cfg.updated_at_ms,
                                    cfg.host_participant_id.as_deref().unwrap_or(""),
                                    cfg.rate_per_minute_nano,
                                    cfg.billing_grace_secs,
                                    cfg.max_screen_shares,
                                );
                                if pay_auto && !fixed_rate {
                                    let prev = rate_for_reader
                                        .swap(cfg.rate_per_minute_nano, Ordering::Relaxed);
                                    if prev != cfg.rate_per_minute_nano {
                                        println!(
                                            "pay_rate updated: {} -> {} (nano-xor/min)",
                                            prev, cfg.rate_per_minute_nano
                                        );
                                    }
                                }
                                let new_host_state = classify_host_state(
                                    cfg.host_participant_id.as_deref(),
                                    &participant_id_for_reader,
                                );
                                let prev_host_state =
                                    host_state_for_reader.swap(new_host_state, Ordering::Relaxed);
                                if prev_host_state != new_host_state {
                                    println!("local_role={}", host_state_label(new_host_state));
                                }
                            }
                            KaigiFrame::SessionPolicy(policy) => {
                                {
                                    let mut state = policy_state_for_reader.lock().await;
                                    state.room_lock = policy.room_lock;
                                    state.waiting_room_enabled = policy.waiting_room_enabled;
                                    state.guest_join_allowed = policy.guest_join_allowed;
                                    state.local_recording_allowed = policy.local_recording_allowed;
                                    state.e2ee_required = policy.e2ee_required;
                                    state.max_participants = policy.max_participants;
                                    state.policy_epoch = policy.policy_epoch;
                                }
                                println!(
                                    "session_policy updated_at_ms={} lock={} waiting={} guests={} local_record={} e2ee_required={} max_participants={} epoch={} updated_by={}",
                                    policy.updated_at_ms,
                                    policy.room_lock,
                                    policy.waiting_room_enabled,
                                    policy.guest_join_allowed,
                                    policy.local_recording_allowed,
                                    policy.e2ee_required,
                                    policy.max_participants,
                                    policy.policy_epoch,
                                    policy.updated_by,
                                );
                            }
                            KaigiFrame::PermissionsSnapshot(PermissionsSnapshotFrame {
                                participant_id,
                                host,
                                co_host,
                                can_moderate,
                                can_record_local,
                                epoch,
                                ..
                            }) => {
                                println!(
                                    "permissions participant={} host={} co_host={} moderate={} record_local={} epoch={}",
                                    participant_id, host, co_host, can_moderate, can_record_local, epoch
                                );
                            }
                            KaigiFrame::ModerationSigned(moderation) => {
                                println!(
                                    "moderation_signed target={:?} action={:?} issued_by={} sent_at_ms={}",
                                    moderation.target,
                                    moderation.action,
                                    moderation.issued_by,
                                    moderation.sent_at_ms
                                );
                            }
                            KaigiFrame::Moderation(moderation) => {
                                println!(
                                    "moderation target={:?} action={:?} sent_at_ms={}",
                                    moderation.target, moderation.action, moderation.sent_at_ms
                                );
                            }
                            KaigiFrame::RoleGrant(grant) => {
                                println!(
                                    "role_grant target={} role={:?} granted_by={} issued_at_ms={}",
                                    grant.target_participant_id, grant.role, grant.granted_by, grant.issued_at_ms
                                );
                            }
                            KaigiFrame::RoleRevoke(revoke) => {
                                println!(
                                    "role_revoke target={} role={:?} revoked_by={} issued_at_ms={}",
                                    revoke.target_participant_id, revoke.role, revoke.revoked_by, revoke.issued_at_ms
                                );
                            }
                            KaigiFrame::DeviceCapability(cap) => {
                                println!(
                                    "device_capability participant={} codecs={:?} hdr_capture={} hdr_render={} max_streams={} reported_at_ms={}",
                                    cap.participant_id,
                                    cap.codecs,
                                    cap.hdr_capture,
                                    cap.hdr_render,
                                    cap.max_video_streams,
                                    cap.reported_at_ms
                                );
                            }
                            KaigiFrame::MediaProfileNegotiation(profile) => {
                                println!(
                                    "media_profile participant={} requested={:?} negotiated={:?} codec={} epoch={} at_ms={}",
                                    profile.participant_id,
                                    profile.requested_profile,
                                    profile.negotiated_profile,
                                    profile.codec,
                                    profile.epoch,
                                    profile.at_ms
                                );
                            }
                            KaigiFrame::RecordingNotice(notice) => {
                                println!(
                                    "recording_notice participant={} state={:?} local={} issued_by={} at_ms={}",
                                    notice.participant_id,
                                    notice.state,
                                    notice.local_recording,
                                    notice.issued_by,
                                    notice.at_ms
                                );
                            }
                            KaigiFrame::E2EEKeyEpoch(epoch) => {
                                println!(
                                    "e2ee_key_epoch participant={} epoch={} sent_at_ms={}",
                                    epoch.participant_id, epoch.epoch, epoch.sent_at_ms
                                );
                            }
                            KaigiFrame::KeyRotationAck(ack) => {
                                println!(
                                    "key_rotation_ack participant={} ack_epoch={} received_at_ms={}",
                                    ack.participant_id, ack.ack_epoch, ack.received_at_ms
                                );
                            }
                            KaigiFrame::ParticipantPresenceDelta(delta) => {
                                println!(
                                    "presence_delta seq={} joined={} left={} role_changes={}",
                                    delta.sequence,
                                    delta.joined.len(),
                                    delta.left.len(),
                                    delta.role_changes.len()
                                );
                            }
                            KaigiFrame::Error(err) => {
                                println!("error: {}", err.message);
                            }
                            KaigiFrame::PaymentAck(ack) => {
                                println!(
                                    "payment_ack received_at_ms={} amount_nano_xor={} total_paid_nano_xor={} total_billed_nano_xor={}",
                                    ack.received_at_ms,
                                    ack.amount_nano_xor,
                                    ack.total_paid_nano_xor,
                                    ack.total_billed_nano_xor
                                );
                            }
                            other => {
                                println!("frame={other:?}");
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    let mut end_requested = false;
    let mut media_profile_epoch: u64 = 0;
    let mut local_e2ee_epoch: u64 = 0;
    loop {
        line.clear();
        let n = stdin.read_line(&mut line).await.context("stdin")?;
        if n == 0 {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" {
            break;
        }
        if input == "/end" {
            let current_host_state = host_state.load(Ordering::Relaxed);
            if let Some(msg) = end_command_gate_message(current_host_state) {
                println!("{msg}");
                continue;
            }
        }

        let at_ms = now_ms();
        let frame = match input {
            cmd if cmd.starts_with("/mic ") => {
                let value = cmd.strip_prefix("/mic ").unwrap_or("").trim();
                KaigiFrame::ParticipantState(ParticipantStateFrame {
                    updated_at_ms: at_ms,
                    mic_enabled: Some(parse_on_off(value)?),
                    video_enabled: None,
                    screen_share_enabled: None,
                })
            }
            cmd if cmd.starts_with("/video ") || cmd.starts_with("/cam ") => {
                let value = cmd
                    .strip_prefix("/video ")
                    .or_else(|| cmd.strip_prefix("/cam "))
                    .unwrap_or("")
                    .trim();
                KaigiFrame::ParticipantState(ParticipantStateFrame {
                    updated_at_ms: at_ms,
                    mic_enabled: None,
                    video_enabled: Some(parse_on_off(value)?),
                    screen_share_enabled: None,
                })
            }
            cmd if cmd.starts_with("/share ") => {
                let value = cmd.strip_prefix("/share ").unwrap_or("").trim();
                KaigiFrame::ParticipantState(ParticipantStateFrame {
                    updated_at_ms: at_ms,
                    mic_enabled: None,
                    video_enabled: None,
                    screen_share_enabled: Some(parse_on_off(value)?),
                })
            }
            cmd if cmd.starts_with("/rate ") => {
                let value = cmd.strip_prefix("/rate ").unwrap_or("").trim();
                let rate_per_minute_nano = value
                    .parse::<u64>()
                    .map_err(|_| anyhow!("expected /rate <u64 nano-xor-per-minute>"))?;
                KaigiFrame::RoomConfigUpdate(RoomConfigUpdateFrame {
                    updated_at_ms: at_ms,
                    rate_per_minute_nano: Some(rate_per_minute_nano),
                    max_screen_shares: None,
                })
            }
            cmd if cmd.starts_with("/maxshare ") => {
                let value = cmd.strip_prefix("/maxshare ").unwrap_or("").trim();
                let max_screen_shares = value
                    .parse::<u8>()
                    .map_err(|_| anyhow!("expected /maxshare <u8>"))?;
                KaigiFrame::RoomConfigUpdate(RoomConfigUpdateFrame {
                    updated_at_ms: at_ms,
                    rate_per_minute_nano: None,
                    max_screen_shares: Some(max_screen_shares),
                })
            }
            "/muteall" => signed_moderation_frame(
                &participant_id,
                at_ms,
                ModerationTarget::All,
                ModerationAction::DisableMic,
            ),
            cmd if cmd.starts_with("/mute ") => {
                let target = cmd.strip_prefix("/mute ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /mute <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::DisableMic,
                )
            }
            cmd if cmd.starts_with("/videooff ") => {
                let target = cmd.strip_prefix("/videooff ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /videooff <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::DisableVideo,
                )
            }
            cmd if cmd.starts_with("/shareoff ") => {
                let target = cmd.strip_prefix("/shareoff ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /shareoff <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::DisableScreenShare,
                )
            }
            cmd if cmd.starts_with("/kick ") => {
                let target = cmd.strip_prefix("/kick ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /kick <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::Kick,
                )
            }
            cmd if cmd.starts_with("/admit ") => {
                let target = cmd.strip_prefix("/admit ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /admit <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::AdmitFromWaiting,
                )
            }
            cmd if cmd.starts_with("/deny ") => {
                let target = cmd.strip_prefix("/deny ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /deny <participant_id>"));
                }
                signed_moderation_frame(
                    &participant_id,
                    at_ms,
                    ModerationTarget::Participant(target.to_string()),
                    ModerationAction::DenyFromWaiting,
                )
            }
            cmd if cmd.starts_with("/cohost ") => {
                let target = cmd.strip_prefix("/cohost ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /cohost <participant_id>"));
                }
                let signature_hex = deterministic_signature_hex(
                    "role_grant",
                    &format!("{participant_id}|{target}|cohost|{at_ms}"),
                );
                KaigiFrame::RoleGrant(RoleGrantFrame {
                    issued_at_ms: at_ms,
                    target_participant_id: target.to_string(),
                    role: RoleKind::CoHost,
                    granted_by: participant_id.clone(),
                    signature_hex,
                })
            }
            cmd if cmd.starts_with("/host ") => {
                let target = cmd.strip_prefix("/host ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /host <participant_id>"));
                }
                let signature_hex = deterministic_signature_hex(
                    "role_grant",
                    &format!("{participant_id}|{target}|host|{at_ms}"),
                );
                KaigiFrame::RoleGrant(RoleGrantFrame {
                    issued_at_ms: at_ms,
                    target_participant_id: target.to_string(),
                    role: RoleKind::Host,
                    granted_by: participant_id.clone(),
                    signature_hex,
                })
            }
            cmd if cmd.starts_with("/uncohost ") => {
                let target = cmd.strip_prefix("/uncohost ").unwrap_or("").trim();
                if target.is_empty() {
                    return Err(anyhow!("expected /uncohost <participant_id>"));
                }
                let signature_hex = deterministic_signature_hex(
                    "role_revoke",
                    &format!("{participant_id}|{target}|cohost|{at_ms}"),
                );
                KaigiFrame::RoleRevoke(RoleRevokeFrame {
                    issued_at_ms: at_ms,
                    target_participant_id: target.to_string(),
                    role: RoleKind::CoHost,
                    revoked_by: participant_id.clone(),
                    signature_hex,
                })
            }
            cmd if cmd.starts_with("/lock ") => {
                let value = cmd.strip_prefix("/lock ").unwrap_or("").trim();
                let enabled = parse_on_off(value)?;
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.room_lock = enabled;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            cmd if cmd.starts_with("/waiting ") => {
                let value = cmd.strip_prefix("/waiting ").unwrap_or("").trim();
                let enabled = parse_on_off(value)?;
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.waiting_room_enabled = enabled;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            cmd if cmd.starts_with("/guests ") => {
                let value = cmd.strip_prefix("/guests ").unwrap_or("").trim();
                let enabled = parse_on_off(value)?;
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.guest_join_allowed = enabled;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            cmd if cmd.starts_with("/recordlocal ") => {
                let value = cmd.strip_prefix("/recordlocal ").unwrap_or("").trim();
                let enabled = parse_on_off(value)?;
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.local_recording_allowed = enabled;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            cmd if cmd.starts_with("/e2eerequired ") => {
                let value = cmd.strip_prefix("/e2eerequired ").unwrap_or("").trim();
                let enabled = parse_on_off(value)?;
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.e2ee_required = enabled;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            cmd if cmd.starts_with("/maxparticipants ") => {
                let value = cmd.strip_prefix("/maxparticipants ").unwrap_or("").trim();
                let max_participants = value
                    .parse::<u32>()
                    .map_err(|_| anyhow!("expected /maxparticipants <u32>"))?;
                if max_participants == 0 {
                    return Err(anyhow!("expected /maxparticipants >= 1"));
                }
                let policy = {
                    let mut state = policy_state.lock().await;
                    state.max_participants = max_participants;
                    state.policy_epoch = state.policy_epoch.saturating_add(1);
                    SessionPolicyFrame {
                        updated_at_ms: at_ms,
                        room_lock: state.room_lock,
                        waiting_room_enabled: state.waiting_room_enabled,
                        guest_join_allowed: state.guest_join_allowed,
                        local_recording_allowed: state.local_recording_allowed,
                        e2ee_required: state.e2ee_required,
                        max_participants: state.max_participants,
                        policy_epoch: state.policy_epoch,
                        updated_by: participant_id.clone(),
                        signature_hex: session_policy_signature_hex(
                            &participant_id,
                            &state,
                            at_ms,
                        ),
                    }
                };
                KaigiFrame::SessionPolicy(policy)
            }
            "/devicecap" => KaigiFrame::DeviceCapability(DeviceCapabilityFrame {
                reported_at_ms: at_ms,
                participant_id: participant_id.clone(),
                codecs: vec!["av1".to_string(), "h265".to_string()],
                hdr_capture,
                hdr_render: hdr_display,
                max_video_streams: 4,
            }),
            cmd if cmd.starts_with("/profile ") => {
                let value = cmd.strip_prefix("/profile ").unwrap_or("").trim();
                let requested_profile = match value {
                    "sdr" => MediaProfileKind::Sdr,
                    "hdr" => MediaProfileKind::Hdr,
                    _ => return Err(anyhow!("expected /profile sdr|hdr")),
                };
                media_profile_epoch = media_profile_epoch.saturating_add(1);
                KaigiFrame::MediaProfileNegotiation(MediaProfileNegotiationFrame {
                    at_ms,
                    participant_id: participant_id.clone(),
                    requested_profile: requested_profile.clone(),
                    negotiated_profile: requested_profile,
                    codec: "av1".to_string(),
                    epoch: media_profile_epoch,
                })
            }
            "/recordstart" => KaigiFrame::RecordingNotice(RecordingNoticeFrame {
                at_ms,
                participant_id: participant_id.clone(),
                state: RecordingState::Started,
                local_recording: true,
                policy_basis: Some("local-user-action".to_string()),
                issued_by: participant_id.clone(),
            }),
            "/recordstop" => KaigiFrame::RecordingNotice(RecordingNoticeFrame {
                at_ms,
                participant_id: participant_id.clone(),
                state: RecordingState::Stopped,
                local_recording: true,
                policy_basis: Some("local-user-action".to_string()),
                issued_by: participant_id.clone(),
            }),
            cmd if cmd.starts_with("/e2eekey ") => {
                let value = cmd.strip_prefix("/e2eekey ").unwrap_or("").trim();
                let epoch = value
                    .parse::<u64>()
                    .map_err(|_| anyhow!("expected /e2eekey <epoch>"))?;
                if epoch == 0 {
                    return Err(anyhow!("expected /e2eekey <epoch>=1.."));
                }
                local_e2ee_epoch = epoch.max(local_e2ee_epoch);
                let public_key_hex = deterministic_signature_hex(
                    "e2ee_public_key",
                    &format!("{participant_id}|{epoch}"),
                );
                let signature_hex = deterministic_signature_hex(
                    "e2ee_key_epoch",
                    &format!("{participant_id}|{epoch}|{at_ms}"),
                );
                KaigiFrame::E2EEKeyEpoch(E2EEKeyEpochFrame {
                    sent_at_ms: at_ms,
                    participant_id: participant_id.clone(),
                    epoch,
                    public_key_hex,
                    signature_hex,
                })
            }
            cmd if cmd.starts_with("/e2eeack ") => {
                let value = cmd.strip_prefix("/e2eeack ").unwrap_or("").trim();
                let ack_epoch = value
                    .parse::<u64>()
                    .map_err(|_| anyhow!("expected /e2eeack <epoch>"))?;
                if ack_epoch == 0 {
                    return Err(anyhow!("expected /e2eeack <epoch>=1.."));
                }
                if local_e2ee_epoch > 0 && ack_epoch > local_e2ee_epoch {
                    return Err(anyhow!(
                        "cannot ack future epoch: local={} requested={}",
                        local_e2ee_epoch,
                        ack_epoch
                    ));
                }
                KaigiFrame::KeyRotationAck(KeyRotationAckFrame {
                    received_at_ms: at_ms,
                    participant_id: participant_id.clone(),
                    ack_epoch,
                })
            }
            "/end" => signed_moderation_frame(
                &participant_id,
                at_ms,
                ModerationTarget::All,
                ModerationAction::Kick,
            ),
            cmd if cmd.starts_with("/pay ") => {
                let value = cmd.strip_prefix("/pay ").unwrap_or("").trim();
                let amount_nano_xor = value
                    .parse::<u64>()
                    .map_err(|_| anyhow!("expected /pay <u64 nano-xor>"))?;
                let tx_hash_hex = if let Some(ref ledger_cfg) = pay_ledger {
                    let iroha_bin = ledger_cfg.iroha_bin.clone();
                    let iroha_config = ledger_cfg.iroha_config.clone();
                    let from = ledger_cfg.from.clone();
                    let to = ledger_cfg.to.clone();
                    let asset_def = ledger_cfg.asset_def.clone();
                    let blocking = ledger_cfg.blocking;
                    match tokio::task::spawn_blocking(move || {
                        ledger::transfer_xor_nano_via_cli(
                            &iroha_bin,
                            &iroha_config,
                            from.as_deref(),
                            &to,
                            amount_nano_xor,
                            &asset_def,
                            blocking,
                        )
                    })
                    .await
                    {
                        Ok(Ok(hash)) => Some(hash),
                        Ok(Err(err)) => {
                            eprintln!("payment transfer failed: {err}");
                            None
                        }
                        Err(err) => {
                            eprintln!("payment transfer task failed: {err}");
                            None
                        }
                    }
                } else {
                    None
                };
                KaigiFrame::Payment(PaymentFrame {
                    sent_at_ms: at_ms,
                    amount_nano_xor,
                    tx_hash_hex,
                })
            }
            _ => KaigiFrame::Chat(ChatFrame {
                sent_at_ms: at_ms,
                from_participant_id: participant_id.clone(),
                from_display_name: args.display_name.clone(),
                text: input.to_string(),
            }),
        };

        let request_end = input == "/end";

        if out_tx.send(frame).await.is_err() {
            break;
        }

        if request_end {
            end_requested = true;
            break;
        }
    }

    drop(out_tx);
    writer.await??;
    reader.await??;

    finalize_auto_kaigi_lifecycle(auto_kaigi_lifecycle, end_requested, session_started_at_ms).await;

    Ok(())
}

struct AnonLocalState {
    handle: String,
    secret: X25519StaticSecret,
    public: X25519PublicKey,
    epoch: u64,
}

fn random_anon_handle() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill_bytes(&mut bytes);
    format!("anon-{}", hex::encode(bytes))
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn build_initial_anon_join_frames(
    participant_handle: String,
    x25519_pubkey_hex: String,
) -> Vec<KaigiFrame> {
    vec![KaigiFrame::AnonHello(AnonHelloFrame {
        protocol_version: PROTOCOL_VERSION,
        participant_handle,
        x25519_pubkey_hex,
    })]
}

fn quote_anon_escrow_total_nano_checked(args: &RoomChatArgs) -> Result<u64> {
    let zk_minutes = args.anon_expected_duration_secs.div_ceil(60);
    let zk_extra =
        u128::from(args.anon_zk_extra_fee_per_minute_nano).saturating_mul(u128::from(zk_minutes));
    let total_amount_u128 = u128::from(args.anon_escrow_prepay_nano).saturating_add(zk_extra);
    u64::try_from(total_amount_u128)
        .map_err(|_| anyhow!("anonymous escrow quote overflow: total amount exceeds u64 nano-XOR"))
}

fn resolve_lifecycle_create_pricing(
    privacy_mode: String,
    gas_rate_per_minute: u64,
    zk_extra_fee_per_minute_nano: u64,
) -> Result<(String, u64)> {
    if privacy_mode.trim().is_empty() {
        return Err(anyhow!(
            "unsupported privacy mode `{privacy_mode}`; expected transparent|zk|zk_roster_v1|zk-roster-v1"
        ));
    }
    let privacy_mode =
        normalize_privacy_mode(Some(privacy_mode)).unwrap_or_else(|| "transparent".to_string());
    if privacy_mode != "transparent" && privacy_mode != "zk" {
        return Err(anyhow!(
            "unsupported privacy mode `{privacy_mode}`; expected transparent|zk|zk_roster_v1|zk-roster-v1"
        ));
    }
    if privacy_mode != "zk" && zk_extra_fee_per_minute_nano > 0 {
        return Err(anyhow!(
            "--zk-extra-fee-per-minute-nano requires --privacy-mode zk"
        ));
    }
    let effective_gas_rate = if privacy_mode == "zk" {
        gas_rate_per_minute
            .checked_add(zk_extra_fee_per_minute_nano)
            .ok_or_else(|| {
                anyhow!(
                    "lifecycle create pricing overflow: --gas-rate-per-minute + --zk-extra-fee-per-minute-nano exceeds u64"
                )
            })?
    } else {
        gas_rate_per_minute
    };
    Ok((privacy_mode, effective_gas_rate))
}

fn new_x25519_keypair() -> (X25519StaticSecret, X25519PublicKey) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = X25519StaticSecret::from(bytes);
    let public = X25519PublicKey::from(&secret);
    (secret, public)
}

fn parse_x25519_pubkey_hex(hex_value: &str) -> Result<X25519PublicKey> {
    let bytes = decode_hex_32(hex_value)?;
    Ok(X25519PublicKey::from(bytes))
}

fn encrypted_kind_tag(kind: &EncryptedControlKind) -> &'static [u8] {
    match kind {
        EncryptedControlKind::Chat => b"chat",
        EncryptedControlKind::ParticipantState => b"participant_state",
        EncryptedControlKind::Moderation => b"moderation",
        EncryptedControlKind::Command => b"command",
        EncryptedControlKind::EscrowHeartbeat => b"escrow_heartbeat",
    }
}

fn derive_pairwise_key(
    local_secret: &X25519StaticSecret,
    peer_public: &X25519PublicKey,
    sender_handle: &str,
    recipient_handle: &str,
    epoch: u64,
    kind: &EncryptedControlKind,
) -> [u8; 32] {
    let shared = local_secret.diffie_hellman(peer_public);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"kaigi:anon:pairwise:v1");
    hasher.update(shared.as_bytes());
    hasher.update(sender_handle.as_bytes());
    hasher.update(recipient_handle.as_bytes());
    hasher.update(&epoch.to_le_bytes());
    hasher.update(encrypted_kind_tag(kind));
    *hasher.finalize().as_bytes()
}

fn encrypt_for_recipient(
    local_secret: &X25519StaticSecret,
    peer_public: &X25519PublicKey,
    sender_handle: &str,
    recipient_handle: &str,
    epoch: u64,
    kind: &EncryptedControlKind,
    plaintext: &[u8],
) -> Result<EncryptedRecipientPayload> {
    let key = derive_pairwise_key(
        local_secret,
        peer_public,
        sender_handle,
        recipient_handle,
        epoch,
        kind,
    );
    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut nonce = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let nonce_ga = XNonce::from(nonce);
    let ciphertext = cipher
        .encrypt(
            &nonce_ga,
            Payload {
                msg: plaintext,
                aad: b"kaigi-anon-control",
            },
        )
        .map_err(|_| anyhow!("failed to encrypt anonymous payload"))?;
    Ok(EncryptedRecipientPayload {
        recipient_handle: recipient_handle.to_string(),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
    })
}

fn decrypt_from_sender(
    local_secret: &X25519StaticSecret,
    sender_public: &X25519PublicKey,
    sender_handle: &str,
    recipient_handle: &str,
    epoch: u64,
    kind: &EncryptedControlKind,
    payload: &EncryptedRecipientPayload,
) -> Result<Vec<u8>> {
    let key = derive_pairwise_key(
        local_secret,
        sender_public,
        sender_handle,
        recipient_handle,
        epoch,
        kind,
    );
    let nonce = decode_hex_vec(&payload.nonce_hex)?;
    if nonce.len() != 24 {
        return Err(anyhow!("nonce must be 24 bytes"));
    }
    let mut nonce_arr = [0u8; 24];
    nonce_arr.copy_from_slice(&nonce);
    let nonce_ga = XNonce::from(nonce_arr);
    let ciphertext = decode_hex_vec(&payload.ciphertext_hex)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let plaintext = cipher
        .decrypt(
            &nonce_ga,
            Payload {
                msg: &ciphertext,
                aad: b"kaigi-anon-control",
            },
        )
        .map_err(|_| anyhow!("failed to decrypt anonymous payload"))?;
    Ok(plaintext)
}

async fn run_room_chat_anonymous(
    args: &RoomChatArgs,
    participant_handle: String,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) -> Result<bool> {
    let (secret, public) = new_x25519_keypair();
    let local = Arc::new(Mutex::new(AnonLocalState {
        handle: participant_handle.clone(),
        secret,
        public,
        epoch: 1,
    }));
    let peers = Arc::new(Mutex::new(HashMap::<String, X25519PublicKey>::new()));
    {
        let local_guard = local.lock().await;
        peers
            .lock()
            .await
            .insert(local_guard.handle.clone(), local_guard.public);
    }

    let (out_tx, mut out_rx) = mpsc::channel::<KaigiFrame>(256);
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let bytes = encode_framed(&frame)?;
            send.write_all(&bytes).await.context("send frame")?;
        }
        send.finish().context("finish send stream")?;
        Ok::<(), anyhow::Error>(())
    });

    let (local_handle, local_pub_hex) = {
        let guard = local.lock().await;
        (guard.handle.clone(), hex::encode(guard.public.as_bytes()))
    };

    for frame in build_initial_anon_join_frames(local_handle.clone(), local_pub_hex) {
        out_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("send channel closed"))?;
    }

    if args.anon_escrow_prepay_nano > 0 {
        let args_cloned = args.clone();
        let prepay_result =
            tokio::task::spawn_blocking(move || run_anon_escrow_prepay(&args_cloned))
                .await
                .context("escrow prepay task")??;
        println!("anon_escrow_prepay={prepay_result}");
    }

    let escrow_id = args
        .anon_escrow_id
        .clone()
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| format!("escrow-{}", random_hex(8)));
    let proof_hex = args
        .anon_escrow_proof_hex
        .clone()
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| random_hex(32));
    out_tx
        .send(KaigiFrame::EscrowProof(EscrowProofFrame {
            sent_at_ms: now_ms(),
            payer_handle: local_handle.clone(),
            escrow_id: escrow_id.clone(),
            proof_hex: proof_hex.clone(),
        }))
        .await
        .map_err(|_| anyhow!("send channel closed"))?;

    if args.anon_escrow_proof_interval_secs > 0 {
        let interval_secs = args.anon_escrow_proof_interval_secs;
        let escrow_id_for_task = escrow_id.clone();
        let payer_handle_for_task = local_handle.clone();
        let proof_hex_for_task = proof_hex.clone();
        let tx = out_tx.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs(interval_secs.max(1));
            loop {
                tokio::time::sleep(interval).await;
                let frame = KaigiFrame::EscrowProof(EscrowProofFrame {
                    sent_at_ms: now_ms(),
                    payer_handle: payer_handle_for_task.clone(),
                    escrow_id: escrow_id_for_task.clone(),
                    proof_hex: proof_hex_for_task.clone(),
                });
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        });
    }

    println!("anonymous participant_handle={local_handle}");
    println!("commands: /rekey, /escrow-proof <hex>, /end, /quit");

    let peers_for_reader = peers.clone();
    let local_for_reader = local.clone();
    let local_handle_for_reader = local_handle.clone();
    let reader = tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        let mut decoder = FrameDecoder::new();
        loop {
            match recv.read(&mut buf).await.context("recv")? {
                Some(n) if n > 0 => {
                    decoder.push(&buf[..n]);
                    while let Some(frame) = decoder.try_next()? {
                        match frame {
                            KaigiFrame::AnonRoster(AnonRosterFrame { participants, .. }) => {
                                let mut map = peers_for_reader.lock().await;
                                map.clear();
                                for p in participants {
                                    if let Ok(pk) = parse_x25519_pubkey_hex(&p.x25519_pubkey_hex) {
                                        map.insert(p.participant_handle, pk);
                                    }
                                }
                                let local = local_for_reader.lock().await;
                                map.insert(local.handle.clone(), local.public);
                                println!("anon_roster peers={}", map.len());
                            }
                            KaigiFrame::GroupKeyUpdate(update) => {
                                let mut map = peers_for_reader.lock().await;
                                if update.x25519_pubkey_hex.is_empty() {
                                    map.remove(&update.participant_handle);
                                    println!("peer_left handle={}", update.participant_handle);
                                } else if let Ok(pk) =
                                    parse_x25519_pubkey_hex(&update.x25519_pubkey_hex)
                                {
                                    map.insert(update.participant_handle.clone(), pk);
                                    println!(
                                        "group_key_update handle={} epoch={}",
                                        update.participant_handle, update.epoch
                                    );
                                }
                            }
                            KaigiFrame::EncryptedControl(enc) => {
                                let Some(payload) = enc
                                    .payloads
                                    .iter()
                                    .find(|p| p.recipient_handle == local_handle_for_reader)
                                else {
                                    continue;
                                };
                                let sender_pub = {
                                    let map = peers_for_reader.lock().await;
                                    map.get(&enc.sender_handle).copied()
                                };
                                let Some(sender_pub) = sender_pub else {
                                    continue;
                                };
                                let local = local_for_reader.lock().await;
                                let plaintext = decrypt_from_sender(
                                    &local.secret,
                                    &sender_pub,
                                    &enc.sender_handle,
                                    &local.handle,
                                    enc.epoch,
                                    &enc.kind,
                                    payload,
                                );
                                drop(local);
                                match plaintext {
                                    Ok(bytes) => {
                                        let text = String::from_utf8_lossy(&bytes);
                                        if let Some(msg) = text.strip_prefix("chat:") {
                                            println!("[anon {}] {}", enc.sender_handle, msg);
                                        } else if let Some(cmd) = text.strip_prefix("cmd:") {
                                            println!(
                                                "anon_command from={} cmd={cmd}",
                                                enc.sender_handle
                                            );
                                        } else {
                                            println!("[anon {}] {}", enc.sender_handle, text);
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "failed to decrypt payload from {}: {err}",
                                            enc.sender_handle
                                        );
                                    }
                                }
                            }
                            KaigiFrame::EscrowAck(ack) => {
                                println!(
                                    "escrow_ack id={} accepted={} reason={}",
                                    ack.escrow_id,
                                    ack.accepted,
                                    ack.reason.as_deref().unwrap_or("<none>")
                                );
                            }
                            KaigiFrame::Error(err) => {
                                println!("error: {}", err.message);
                            }
                            other => {
                                println!("frame={other:?}");
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    let mut end_requested = false;
    loop {
        line.clear();
        let n = stdin.read_line(&mut line).await.context("stdin")?;
        if n == 0 {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" {
            break;
        }
        if input == "/rekey" {
            let (new_secret, new_public) = new_x25519_keypair();
            let (handle, epoch, pub_hex) = {
                let mut local_guard = local.lock().await;
                local_guard.secret = new_secret;
                local_guard.public = new_public;
                local_guard.epoch = local_guard.epoch.saturating_add(1);
                (
                    local_guard.handle.clone(),
                    local_guard.epoch,
                    hex::encode(local_guard.public.as_bytes()),
                )
            };
            peers
                .lock()
                .await
                .insert(handle.clone(), parse_x25519_pubkey_hex(&pub_hex)?);
            out_tx
                .send(KaigiFrame::GroupKeyUpdate(GroupKeyUpdateFrame {
                    sent_at_ms: now_ms(),
                    participant_handle: handle,
                    x25519_pubkey_hex: pub_hex,
                    epoch,
                }))
                .await
                .map_err(|_| anyhow!("send channel closed"))?;
            continue;
        }
        if let Some(value) = input.strip_prefix("/escrow-proof ") {
            let proof_hex = value.trim();
            if let Err(err) = validate_anon_proof_hex(proof_hex, "/escrow-proof") {
                eprintln!("{err}");
                continue;
            }
            out_tx
                .send(KaigiFrame::EscrowProof(EscrowProofFrame {
                    sent_at_ms: now_ms(),
                    payer_handle: local_handle.clone(),
                    escrow_id: escrow_id.clone(),
                    proof_hex: proof_hex.to_string(),
                }))
                .await
                .map_err(|_| anyhow!("send channel closed"))?;
            continue;
        }

        let (sender, epoch) = {
            let local_guard = local.lock().await;
            (local_guard.handle.clone(), local_guard.epoch)
        };
        let kind = if input == "/end" {
            EncryptedControlKind::Command
        } else {
            EncryptedControlKind::Chat
        };
        let body = if input == "/end" {
            "cmd:end".to_string()
        } else {
            format!("chat:{input}")
        };

        let recipients = peers.lock().await.clone();
        if recipients.is_empty() {
            continue;
        }
        let mut payloads = Vec::with_capacity(recipients.len());
        for (recipient, pubkey) in recipients {
            let local_guard = local.lock().await;
            let payload = encrypt_for_recipient(
                &local_guard.secret,
                &pubkey,
                &sender,
                &recipient,
                epoch,
                &kind,
                body.as_bytes(),
            )?;
            drop(local_guard);
            payloads.push(payload);
        }
        out_tx
            .send(KaigiFrame::EncryptedControl(EncryptedControlFrame {
                sent_at_ms: now_ms(),
                sender_handle: sender,
                epoch,
                kind,
                payloads,
            }))
            .await
            .map_err(|_| anyhow!("send channel closed"))?;

        if input == "/end" {
            end_requested = true;
            break;
        }
    }

    drop(out_tx);
    writer.await??;
    let _ = reader.await?;

    if args.anon_unshield_on_exit {
        let args_cloned = args.clone();
        match tokio::task::spawn_blocking(move || run_anon_escrow_unshield(&args_cloned)).await {
            Ok(Ok(payload)) => println!("anon_escrow_unshield={payload}"),
            Ok(Err(err)) => eprintln!("anon escrow unshield failed: {err}"),
            Err(err) => eprintln!("anon escrow unshield task failed: {err}"),
        }
    }

    Ok(end_requested)
}

fn run_anon_escrow_prepay(args: &RoomChatArgs) -> Result<String> {
    let iroha_config = args
        .pay_iroha_config
        .as_ref()
        .ok_or_else(|| anyhow!("--pay-iroha-config is required for --anon-escrow-prepay-nano"))?;
    let from = args
        .pay_from
        .as_ref()
        .ok_or_else(|| anyhow!("--pay-from is required for --anon-escrow-prepay-nano"))?;
    let iroha_bin = args
        .pay_iroha_bin
        .clone()
        .unwrap_or_else(|| ledger::default_iroha_bin().to_string());
    let total_amount = quote_anon_escrow_total_nano_checked(args)?;
    let zk_extra = total_amount.saturating_sub(args.anon_escrow_prepay_nano);
    println!(
        "anon_escrow_quote base_nano={} zk_extra_nano={} total_nano={} expected_duration_secs={}",
        args.anon_escrow_prepay_nano, zk_extra, total_amount, args.anon_expected_duration_secs
    );
    let note_commitment = random_hex(32);
    let ephemeral_pubkey = random_hex(32);
    let nonce_hex = random_hex(24);
    let mut ciphertext = vec![0u8; 64];
    rand::rng().fill_bytes(&mut ciphertext);
    let ciphertext_b64 = base64::engine::general_purpose::STANDARD.encode(ciphertext);

    let cmd = vec![
        "app".to_string(),
        "zk".to_string(),
        "shield".to_string(),
        "--asset".to_string(),
        args.pay_asset_def.clone(),
        "--from".to_string(),
        from.clone(),
        "--amount".to_string(),
        total_amount.to_string(),
        "--note-commitment".to_string(),
        note_commitment,
        "--ephemeral-pubkey".to_string(),
        ephemeral_pubkey,
        "--nonce-hex".to_string(),
        nonce_hex,
        "--ciphertext-b64".to_string(),
        ciphertext_b64,
    ];
    run_iroha_json_cli(&iroha_bin, iroha_config, &cmd)
}

fn run_anon_escrow_unshield(args: &RoomChatArgs) -> Result<String> {
    let iroha_config = args
        .pay_iroha_config
        .as_ref()
        .ok_or_else(|| anyhow!("--pay-iroha-config is required for anonymous unshield"))?;
    let to = args
        .anon_unshield_to
        .as_ref()
        .ok_or_else(|| anyhow!("--anon-unshield-to is required"))?;
    let inputs = args
        .anon_unshield_inputs
        .as_ref()
        .ok_or_else(|| anyhow!("--anon-unshield-inputs is required"))?;
    let proof_json = args
        .anon_unshield_proof_json
        .as_ref()
        .ok_or_else(|| anyhow!("--anon-unshield-proof-json is required"))?;
    let iroha_bin = args
        .pay_iroha_bin
        .clone()
        .unwrap_or_else(|| ledger::default_iroha_bin().to_string());
    let total_amount = quote_anon_escrow_total_nano_checked(args)?;
    let normalized_inputs = normalize_hex32_csv(inputs)?;

    let mut cmd = vec![
        "app".to_string(),
        "zk".to_string(),
        "unshield".to_string(),
        "--asset".to_string(),
        args.pay_asset_def.clone(),
        "--to".to_string(),
        to.clone(),
        "--amount".to_string(),
        total_amount.to_string(),
        "--inputs".to_string(),
        normalized_inputs,
        "--proof-json".to_string(),
        proof_json.to_string_lossy().into_owned(),
    ];
    if let Some(root_hint) = args.anon_unshield_root_hint_hex.as_deref() {
        validate_hex_32_arg(root_hint, "--anon-unshield-root-hint-hex")?;
        cmd.extend(["--root-hint".to_string(), normalize_hex_arg(root_hint)]);
    }
    run_iroha_json_cli(&iroha_bin, iroha_config, &cmd)
}

async fn finalize_auto_kaigi_lifecycle(
    auto_kaigi_lifecycle: Option<AutoKaigiLifecycle>,
    end_requested: bool,
    session_started_at_ms: u64,
) {
    let Some(lc) = auto_kaigi_lifecycle else {
        return;
    };
    let duration_ms = now_ms().saturating_sub(session_started_at_ms);
    let mut ended_call = false;
    if end_requested {
        let lc_end = lc.clone();
        let ended_at_ms = now_ms();
        match tokio::task::spawn_blocking(move || run_auto_kaigi_end(&lc_end, ended_at_ms)).await {
            Ok(Ok(payload)) => {
                ended_call = true;
                println!("kaigi_end={payload}");
            }
            Ok(Err(err)) => {
                eprintln!("kaigi lifecycle end failed: {err}");
            }
            Err(err) => {
                eprintln!("kaigi lifecycle end task failed: {err}");
            }
        }
    }

    if !should_skip_leave_after_end(end_requested, ended_call) {
        let lc_leave = lc.clone();
        match tokio::task::spawn_blocking(move || run_auto_kaigi_leave(&lc_leave)).await {
            Ok(Ok(payload)) => {
                println!("kaigi_leave={payload}");
            }
            Ok(Err(err)) => {
                eprintln!("kaigi lifecycle leave failed: {err}");
            }
            Err(err) => {
                eprintln!("kaigi lifecycle leave task failed: {err}");
            }
        }
    }

    if lc.record_usage {
        let lc_usage = lc.clone();
        match tokio::task::spawn_blocking(move || {
            run_auto_kaigi_record_usage(&lc_usage, duration_ms)
        })
        .await
        {
            Ok(Ok(payload)) => {
                println!("kaigi_record_usage={payload}");
            }
            Ok(Err(err)) => {
                eprintln!("kaigi lifecycle record-usage failed: {err}");
            }
            Err(err) => {
                eprintln!("kaigi lifecycle record-usage task failed: {err}");
            }
        }
    }
}

fn parse_on_off(value: &str) -> Result<bool> {
    match value {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        _ => Err(anyhow!("expected on|off")),
    }
}

fn deterministic_signature_hex(tag: &str, payload: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag.as_bytes());
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize().as_bytes())
}

fn session_policy_signature_hex(
    updated_by: &str,
    state: &LocalSessionPolicyState,
    updated_at_ms: u64,
) -> String {
    deterministic_signature_hex(
        "session_policy",
        &format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            updated_by,
            state.room_lock,
            state.waiting_room_enabled,
            state.guest_join_allowed,
            state.local_recording_allowed,
            state.e2ee_required,
            state.max_participants,
            state.policy_epoch,
            updated_at_ms
        ),
    )
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

fn signed_moderation_frame(
    issued_by: &str,
    sent_at_ms: u64,
    target: ModerationTarget,
    action: ModerationAction,
) -> KaigiFrame {
    let signature_hex = deterministic_signature_hex(
        "moderation",
        &format!(
            "{}|{}|{}|{}",
            issued_by,
            moderation_target_token(&target),
            moderation_action_token(&action),
            sent_at_ms
        ),
    );
    KaigiFrame::ModerationSigned(ModerationSignedFrame {
        sent_at_ms,
        target,
        action,
        issued_by: issued_by.to_string(),
        signature_hex,
    })
}

fn is_pay_auto_enabled(args: &RoomChatArgs) -> bool {
    args.pay_auto || !args.no_pay_auto
}

fn should_skip_leave_after_end(end_requested: bool, end_succeeded: bool) -> bool {
    end_requested && end_succeeded
}

fn validate_nexus_routing_requirement(
    allow_local_handshake: bool,
    torii: Option<&str>,
) -> Result<()> {
    if allow_local_handshake || torii.is_some() {
        return Ok(());
    }
    Err(anyhow!(
        "--torii (or a --join-link containing torii=...) is required for Nexus-routed calls; pass --allow-local-handshake only for local dev harness"
    ))
}

fn validate_join_link_call_metadata(
    kaigi_domain: Option<&str>,
    kaigi_call_name: Option<&str>,
) -> Result<()> {
    if kaigi_domain.is_some() != kaigi_call_name.is_some() {
        return Err(anyhow!(
            "--kaigi-domain and --kaigi-call-name must be set together when embedding lifecycle metadata"
        ));
    }
    Ok(())
}

fn validate_privacy_mode_arg(mode: Option<&str>) -> Result<()> {
    if let Some(mode) = mode {
        let normalized = mode.trim().to_ascii_lowercase().replace('-', "_");
        if normalized != "transparent" && normalized != "zk" && normalized != "zk_roster_v1" {
            return Err(anyhow!(
                "unsupported privacy mode `{mode}`; expected transparent|zk|zk_roster_v1|zk-roster-v1"
            ));
        }
    }
    Ok(())
}

fn validate_anon_escrow_id(value: &str) -> Result<()> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(anyhow!("--anon-escrow-id must be non-empty"));
    }
    if !value.is_ascii() {
        return Err(anyhow!("--anon-escrow-id must be ASCII"));
    }
    if value.contains('@') {
        return Err(anyhow!("--anon-escrow-id must not contain '@'"));
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(anyhow!(
            "--anon-escrow-id must not contain whitespace/control chars"
        ));
    }
    if normalized.len() > MAX_ESCROW_ID_LEN {
        return Err(anyhow!(
            "--anon-escrow-id too long: max {MAX_ESCROW_ID_LEN} chars"
        ));
    }
    Ok(())
}

fn validate_anonymous_participant_handle(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!(
            "--participant-id must be non-empty in anonymous mode"
        ));
    }
    if !value.is_ascii() {
        return Err(anyhow!("--participant-id must be ASCII in anonymous mode"));
    }
    if value.contains('@') {
        return Err(anyhow!(
            "--participant-id must not contain '@' in anonymous mode"
        ));
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(anyhow!(
            "--participant-id must not contain whitespace/control chars in anonymous mode"
        ));
    }
    if value.len() > MAX_ANON_PARTICIPANT_HANDLE_LEN {
        return Err(anyhow!(
            "--participant-id too long in anonymous mode: max {MAX_ANON_PARTICIPANT_HANDLE_LEN} chars"
        ));
    }
    Ok(())
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

fn normalize_hex_arg(value: &str) -> String {
    strip_hex_prefix(value.trim()).to_ascii_lowercase()
}

fn normalize_hex32_csv(value: &str) -> Result<String> {
    validate_unshield_inputs_arg(value)?;
    Ok(value
        .split(',')
        .map(|token| normalize_hex_arg(token.trim()))
        .collect::<Vec<_>>()
        .join(","))
}

fn validate_anon_proof_hex(value: &str, arg_name: &str) -> Result<()> {
    let normalized = value.trim();
    let normalized = strip_hex_prefix(normalized);
    if normalized.len() > MAX_ESCROW_PROOF_HEX_LEN {
        return Err(anyhow!(
            "{arg_name} too long: max {MAX_ESCROW_PROOF_HEX_LEN} hex chars"
        ));
    }
    if normalized.is_empty()
        || !normalized.len().is_multiple_of(2)
        || hex::decode(normalized).is_err()
    {
        return Err(anyhow!("{arg_name} must be valid non-empty hex"));
    }
    Ok(())
}

fn validate_hex_32_arg(value: &str, arg_name: &str) -> Result<()> {
    let normalized = strip_hex_prefix(value.trim());
    if normalized.len() != 64 || hex::decode(normalized).is_err() {
        return Err(anyhow!("{arg_name} must be 32-byte hex"));
    }
    Ok(())
}

fn validate_unshield_inputs_arg(value: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for (idx, token) in value.split(',').enumerate() {
        if idx >= MAX_ANON_UNSHIELD_INPUTS {
            return Err(anyhow!(
                "--anon-unshield-inputs has too many entries: max {MAX_ANON_UNSHIELD_INPUTS}"
            ));
        }
        let normalized = normalize_hex_arg(token.trim());
        if validate_hex_32_arg(&normalized, "--anon-unshield-inputs").is_err() {
            return Err(anyhow!(
                "--anon-unshield-inputs item {} must be 32-byte hex",
                idx + 1
            ));
        }
        if !seen.insert(normalized) {
            return Err(anyhow!(
                "--anon-unshield-inputs item {} duplicates a previous nullifier",
                idx + 1
            ));
        }
    }
    Ok(())
}

fn has_anonymous_escrow_overrides(args: &RoomChatArgs) -> bool {
    args.anon_escrow_prepay_nano > 0
        || args.anon_zk_extra_fee_per_minute_nano > 0
        || args.anon_expected_duration_secs != 60
        || args.anon_escrow_proof_interval_secs != 20
        || args.anon_escrow_id.is_some()
        || args.anon_escrow_proof_hex.is_some()
        || args.anon_unshield_on_exit
        || args.anon_unshield_to.is_some()
        || args.anon_unshield_inputs.is_some()
        || args.anon_unshield_proof_json.is_some()
        || args.anon_unshield_root_hint_hex.is_some()
}

fn validate_anonymous_escrow_settings(args: &RoomChatArgs, anonymous_mode: bool) -> Result<()> {
    if !anonymous_mode && has_anonymous_escrow_overrides(args) {
        return Err(anyhow!(
            "anonymous escrow flags require --kaigi-privacy-mode zk"
        ));
    }
    if !anonymous_mode {
        return Ok(());
    }

    if let Some(participant_handle) = args.participant_id.as_deref() {
        validate_anonymous_participant_handle(participant_handle)?;
    }
    if let Some(escrow_id) = args.anon_escrow_id.as_deref() {
        validate_anon_escrow_id(escrow_id)?;
    }
    if let Some(proof_hex) = args.anon_escrow_proof_hex.as_deref() {
        validate_anon_proof_hex(proof_hex, "--anon-escrow-proof-hex")?;
    }

    if args.anon_zk_extra_fee_per_minute_nano > 0 && args.anon_escrow_prepay_nano == 0 {
        return Err(anyhow!(
            "--anon-zk-extra-fee-per-minute-nano requires --anon-escrow-prepay-nano > 0"
        ));
    }
    if args.pay_rate_per_minute_nano > 0 {
        return Err(anyhow!(
            "--pay-rate-per-minute-nano is unsupported in anonymous mode; use escrow proof + prepay options instead"
        ));
    }
    if args.pay_auto || args.no_pay_auto {
        return Err(anyhow!(
            "--pay-auto/--no-pay-auto are unsupported in anonymous mode"
        ));
    }
    if args.allow_unsettled_payments {
        return Err(anyhow!(
            "--allow-unsettled-payments is unsupported in anonymous mode"
        ));
    }
    if args.anon_zk_extra_fee_per_minute_nano > 0 && args.anon_expected_duration_secs == 0 {
        return Err(anyhow!(
            "--anon-zk-extra-fee-per-minute-nano requires --anon-expected-duration-secs > 0"
        ));
    }
    if args.anon_escrow_prepay_nano > 0
        && (args.pay_iroha_config.is_none() || args.pay_from.is_none())
    {
        return Err(anyhow!(
            "--anon-escrow-prepay-nano requires --pay-iroha-config and --pay-from"
        ));
    }
    let has_unshield_arg_without_flag = (args.anon_unshield_to.is_some()
        || args.anon_unshield_inputs.is_some()
        || args.anon_unshield_proof_json.is_some()
        || args.anon_unshield_root_hint_hex.is_some())
        && !args.anon_unshield_on_exit;
    if has_unshield_arg_without_flag {
        return Err(anyhow!(
            "--anon-unshield-to/--anon-unshield-inputs/--anon-unshield-proof-json/--anon-unshield-root-hint-hex require --anon-unshield-on-exit"
        ));
    }
    if args.anon_unshield_on_exit {
        if args.anon_escrow_prepay_nano == 0 {
            return Err(anyhow!(
                "--anon-unshield-on-exit requires --anon-escrow-prepay-nano > 0"
            ));
        }
        if args.pay_iroha_config.is_none() {
            return Err(anyhow!(
                "--anon-unshield-on-exit requires --pay-iroha-config"
            ));
        }
        if args.anon_unshield_to.is_none()
            || args.anon_unshield_inputs.is_none()
            || args.anon_unshield_proof_json.is_none()
        {
            return Err(anyhow!(
                "--anon-unshield-on-exit requires --anon-unshield-to --anon-unshield-inputs --anon-unshield-proof-json"
            ));
        }
        if let Some(inputs) = args.anon_unshield_inputs.as_deref() {
            validate_unshield_inputs_arg(inputs)?;
        }
        if let Some(root_hint) = args.anon_unshield_root_hint_hex.as_deref() {
            validate_hex_32_arg(root_hint, "--anon-unshield-root-hint-hex")?;
        }
    }
    if args.anon_escrow_prepay_nano > 0 || args.anon_unshield_on_exit {
        let _ = quote_anon_escrow_total_nano_checked(args)?;
    }
    Ok(())
}

fn normalize_privacy_mode(mode: Option<String>) -> Option<String> {
    mode.and_then(|raw| {
        let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
        if normalized.is_empty() {
            None
        } else if normalized == "zk_roster_v1" {
            Some("zk".to_string())
        } else {
            Some(normalized)
        }
    })
}

fn is_anonymous_mode_enabled(mode: Option<&str>) -> bool {
    matches!(
        normalize_privacy_mode(mode.map(|value| value.to_string())).as_deref(),
        Some("zk")
    )
}

fn apply_room_chat_join_link_defaults(
    args: &mut RoomChatArgs,
    join_link: Option<&JoinLinkPayload>,
) {
    let Some(payload) = join_link else {
        return;
    };

    if args.relay.is_none() {
        args.relay = Some(payload.relay);
    }
    if args.channel.is_none() {
        args.channel = Some(hex::encode(payload.channel));
    }
    if args.server_name.is_none() {
        args.server_name = payload.server_name.clone();
    }
    if payload.authenticated {
        args.authenticated = true;
    }
    if payload.insecure {
        args.insecure = true;
    }
    if args.torii.is_none() {
        args.torii = payload.torii.clone();
    }
    if args.pay_to.is_none() {
        args.pay_to = payload.pay_to.clone();
    }
    if args.kaigi_domain.is_none() {
        args.kaigi_domain = payload.kaigi_domain.clone();
    }
    if args.kaigi_call_name.is_none() {
        args.kaigi_call_name = payload.kaigi_call_name.clone();
    }
    if args.kaigi_privacy_mode.is_none() {
        args.kaigi_privacy_mode = payload.kaigi_privacy_mode.clone();
    }
}

fn apply_room_chat_env_defaults(args: &mut RoomChatArgs) {
    if args.torii.is_none() {
        args.torii = env_trimmed("KAIGI_TORII");
    }

    if args.pay_iroha_config.is_none() {
        args.pay_iroha_config =
            env_pathbuf("KAIGI_PAY_IROHA_CONFIG").or_else(|| env_pathbuf("IROHA_CONFIG"));
    }
    if args.pay_to.is_none() {
        args.pay_to = env_trimmed("KAIGI_PAY_TO");
    }

    if args.kaigi_iroha_config.is_none() {
        args.kaigi_iroha_config = env_pathbuf("KAIGI_LIFECYCLE_IROHA_CONFIG");
    }
    if args.kaigi_domain.is_none() {
        args.kaigi_domain = env_trimmed("KAIGI_DOMAIN");
    }
    if args.kaigi_call_name.is_none() {
        args.kaigi_call_name = env_trimmed("KAIGI_CALL_NAME");
    }
    if args.kaigi_privacy_mode.is_none() {
        args.kaigi_privacy_mode = env_trimmed("KAIGI_PRIVACY_MODE");
    }
    if args.kaigi_participant.is_none() {
        args.kaigi_participant = env_trimmed("KAIGI_PARTICIPANT");
    }
}

fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_pathbuf(key: &str) -> Option<PathBuf> {
    env_trimmed(key).map(PathBuf::from)
}

fn now_ms() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.as_millis()).unwrap_or(u64::MAX)
}

fn write_route_update(args: WriteRouteUpdateArgs) -> Result<()> {
    let relay_id = decode_hex_32(&args.relay_id)?;
    let channel_id = match args.channel.as_deref() {
        Some(hex) => decode_hex_32(hex)?,
        None => {
            let mut id = [0u8; 32];
            rand::rng().fill_bytes(&mut id);
            id
        }
    };

    let access_kind = match args.access_kind.as_str() {
        "public" => SoranetAccessKind::ReadOnly,
        "authenticated" => SoranetAccessKind::Authenticated,
        other => return Err(anyhow!("unknown access_kind: {other}")),
    };

    let mut route_id = [0u8; 32];
    let mut stream_id = [0u8; 32];
    let mut exit_token = vec![0u8; 32];
    rand::rng().fill_bytes(&mut route_id);
    rand::rng().fill_bytes(&mut stream_id);
    rand::rng().fill_bytes(&mut exit_token);
    let room_id = derive_kaigi_room_id(&channel_id, &route_id, &stream_id);

    let route = SoranetRoute {
        channel_id: SoranetChannelId::from(channel_id),
        exit_multiaddr: args.exit_multiaddr.clone(),
        padding_budget_ms: args.padding_budget_ms,
        access_kind,
        stream_tag: SoranetStreamTag::Kaigi,
    };

    let update = PrivacyRouteUpdate {
        route_id,
        stream_id,
        content_key_id: 0,
        valid_from_segment: args.valid_from_segment,
        valid_until_segment: args.valid_until_segment,
        exit_token,
        soranet: Some(route),
    };

    let bytes = to_bytes(&update).context("encode PrivacyRouteUpdate (norito)")?;

    let relay_hex = hex::encode(relay_id);
    let mut dir = args.spool_dir.join(format!("exit-{relay_hex}"));
    dir.push("kaigi-stream");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create catalog dir {}", dir.display()))?;

    let filename = format!(
        "kaigi-route-{}-{}.norito",
        args.valid_from_segment,
        hex::encode(&stream_id[..8])
    );
    let path = dir.join(filename);
    std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;

    println!("wrote={}", path.display());
    println!("relay_id_hex={}", hex::encode(relay_id));
    println!("channel_id_hex={}", hex::encode(channel_id));
    println!("route_id_hex={}", hex::encode(route_id));
    println!("stream_id_hex={}", hex::encode(stream_id));
    println!("room_id_hex={}", hex::encode(room_id));
    println!("access_kind={}", args.access_kind);
    println!("exit_multiaddr={}", args.exit_multiaddr);
    if let Some(relay) = args.relay {
        validate_nexus_routing_requirement(args.allow_local_handshake, args.torii.as_deref())?;
        validate_join_link_call_metadata(
            args.kaigi_domain.as_deref(),
            args.kaigi_call_name.as_deref(),
        )?;
        validate_privacy_mode_arg(args.kaigi_privacy_mode.as_deref())?;
        let mut join = JoinLinkPayload {
            version: if args.join_link_legacy_v1 {
                JOIN_LINK_VERSION_LEGACY
            } else {
                JOIN_LINK_VERSION_SIGNED
            },
            relay,
            channel: channel_id,
            authenticated: matches!(access_kind, SoranetAccessKind::Authenticated),
            insecure: args.insecure,
            server_name: Some(args.server_name.unwrap_or_else(|| "localhost".to_string())),
            torii: args.torii,
            pay_to: args.pay_to,
            kaigi_domain: args.kaigi_domain,
            kaigi_call_name: args.kaigi_call_name,
            kaigi_privacy_mode: normalize_privacy_mode(args.kaigi_privacy_mode),
            expires_at_ms: None,
            nonce_hex: None,
            signature_hex: None,
        };
        if join.version == JOIN_LINK_VERSION_SIGNED {
            populate_join_link_security_fields(&mut join, args.join_link_expires_in_secs)?;
        }
        println!("join_link={}", render_join_link(&join));
    }
    Ok(())
}

fn list_routes(args: ListRoutesArgs) -> Result<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(relay_id_hex) = args.relay_id.as_deref() {
        let relay_id = decode_hex_32(relay_id_hex)?;
        let relay_hex = hex::encode(relay_id);
        dirs.push(
            args.spool_dir
                .join(format!("exit-{relay_hex}"))
                .join("kaigi-stream"),
        );
    } else {
        if !args.spool_dir.is_dir() {
            return Err(anyhow!(
                "spool_dir is not a directory: {}",
                args.spool_dir.display()
            ));
        }
        for entry in std::fs::read_dir(&args.spool_dir)
            .with_context(|| format!("read spool_dir {}", args.spool_dir.display()))?
        {
            let entry = entry.context("read_dir entry")?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.starts_with("exit-") {
                continue;
            }
            dirs.push(path.join("kaigi-stream"));
        }
    }

    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry.context("read_dir entry")?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "norito") {
                continue;
            }
            let meta = entry.metadata().context("metadata")?;
            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            files.push((mtime, path));
        }
    }

    files.sort_by(|(a_time, a), (b_time, b)| b_time.cmp(a_time).then_with(|| a.cmp(b)));

    let limit = args.limit.clamp(1, 10_000);
    let mut printed = 0usize;
    for (_mtime, path) in files {
        if printed >= limit {
            break;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let update: PrivacyRouteUpdate =
            norito::decode_from_bytes(&bytes).context("decode PrivacyRouteUpdate (norito)")?;

        let (channel_hex, access_kind, exit_multiaddr, stream_tag, authenticated) = update
            .soranet
            .as_ref()
            .map(|s| {
                (
                    hex::encode(*s.channel_id.as_ref()),
                    format!("{:?}", s.access_kind),
                    s.exit_multiaddr.clone(),
                    format!("{:?}", s.stream_tag),
                    matches!(s.access_kind, SoranetAccessKind::Authenticated),
                )
            })
            .unwrap_or_else(|| {
                (
                    "<none>".to_string(),
                    "<none>".to_string(),
                    "<none>".to_string(),
                    "<none>".to_string(),
                    false,
                )
            });

        let room_id = if let Some(route) = update.soranet.as_ref() {
            derive_kaigi_room_id(
                route.channel_id.as_ref(),
                &update.route_id,
                &update.stream_id,
            )
        } else {
            [0u8; 32]
        };

        println!("file={}", path.display());
        println!(
            "  channel_id_hex={channel_hex} room_id_hex={} access_kind={access_kind} stream_tag={stream_tag}",
            hex::encode(room_id)
        );
        println!(
            "  route_id_hex={} stream_id_hex={} valid_from_segment={} valid_until_segment={} exit_multiaddr={exit_multiaddr} exit_token_len={}",
            hex::encode(update.route_id),
            hex::encode(update.stream_id),
            update.valid_from_segment,
            update.valid_until_segment,
            update.exit_token.len(),
        );
        if channel_hex != "<none>" {
            println!(
                "  join_link_template=kaigi://join?v=1&relay=<relay-host:port>&channel={channel_hex}&authenticated={}&insecure=<0|1>&sni=<server-name>&torii=<torii-base-url>&pay_to=<billing-account-id>&kaigi_domain=<domain>&kaigi_call_name=<call-name>&kaigi_privacy_mode=<transparent|zk|zk_roster_v1|zk-roster-v1>",
                if authenticated { 1 } else { 0 }
            );
        }

        printed += 1;
    }

    if printed == 0 {
        println!(
            "no kaigi route updates found under {}",
            args.spool_dir.display()
        );
    }
    Ok(())
}

fn print_handshake(params: &HandshakeParams) {
    println!(
        "descriptor_commit_hex={}",
        hex::encode(params.descriptor_commit)
    );
    println!(
        "client_capabilities_hex={}",
        hex::encode(&params.client_capabilities)
    );
    println!(
        "relay_capabilities_hex={}",
        hex::encode(&params.relay_capabilities)
    );
    println!("kem_id={}", params.kem_id);
    println!("sig_id={}", params.sig_id);
    if let Some(resume) = params.resume_hash.as_deref() {
        println!("resume_hash_hex={}", hex::encode(resume));
    } else {
        println!("resume_hash_hex=<none>");
    }
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

mod ledger {
    use super::*;

    pub fn default_iroha_bin() -> &'static str {
        "../iroha/target/release/iroha"
    }

    pub fn transfer_xor_nano_via_cli(
        iroha_bin: &str,
        iroha_config_path: &PathBuf,
        from_account_id_override: Option<&str>,
        to: &str,
        amount_nano_xor: u64,
        asset_def: &str,
        _blocking: bool,
    ) -> Result<String> {
        let from_account = match from_account_id_override {
            Some(id) => id.to_string(),
            None => infer_account_id_from_config(iroha_config_path)?,
        };

        let asset_id = format!("{asset_def}#{from_account}");
        let quantity = nano_xor_to_decimal(amount_nano_xor);

        let output = std::process::Command::new(iroha_bin)
            .arg("-c")
            .arg(iroha_config_path)
            .arg("--output-format")
            .arg("json")
            .arg("ledger")
            .arg("asset")
            .arg("transfer")
            .arg("--id")
            .arg(asset_id)
            .arg("--to")
            .arg(to)
            .arg("--quantity")
            .arg(quantity)
            .output()
            .with_context(|| format!("spawn iroha cli ({iroha_bin})"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "iroha cli transfer failed (exit={:?}): {}",
                output.status.code(),
                stderr.trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let value: JsonValue =
            serde_json::from_str(stdout.trim()).context("parse iroha cli json output")?;
        let hash = extract_hash(&value).context("extract tx hash")?;
        Ok(hash)
    }

    fn extract_hash(v: &JsonValue) -> Result<String> {
        let Some(hash_val) = v.get("hash") else {
            return Err(anyhow!("iroha cli output missing `hash` field"));
        };

        if let Some(s) = hash_val.as_str() {
            return Ok(s.to_string());
        }

        if let Some(obj) = hash_val.as_object() {
            for key in ["value", "hash", "hex", "bytes"] {
                if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                    return Ok(s.to_string());
                }
            }
        }

        Ok(hash_val.to_string())
    }

    fn nano_xor_to_decimal(amount_nano_xor: u64) -> String {
        let whole = amount_nano_xor / 1_000_000_000;
        let frac = amount_nano_xor % 1_000_000_000;
        format!("{whole}.{frac:09}")
    }

    fn infer_account_id_from_config(path: &PathBuf) -> Result<String> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read iroha config {}", path.display()))?;

        let mut in_account = false;
        let mut domain: Option<String> = None;
        let mut public_key: Option<String> = None;
        for mut line in raw.lines() {
            line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                in_account = line == "[account]";
                continue;
            }

            if !in_account {
                continue;
            }

            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let mut value = v.trim();
            // Trim surrounding quotes for simple string values.
            value = value.trim_matches('"').trim_matches('\'');

            match key {
                "domain" => domain = Some(value.to_string()),
                "public_key" => public_key = Some(value.to_string()),
                _ => {}
            }

            if domain.is_some() && public_key.is_some() {
                break;
            }
        }

        let domain = domain.ok_or_else(|| anyhow!("missing [account].domain in config"))?;
        let public_key =
            public_key.ok_or_else(|| anyhow!("missing [account].public_key in config"))?;
        Ok(format!("{public_key}@{domain}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svec(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn clear_join_link_nonce_cache() {
        if let Some(cache) = SEEN_JOIN_LINK_NONCES.get()
            && let Ok(mut guard) = cache.lock()
        {
            guard.clear();
        }
    }

    fn join_link_test_mutex() -> &'static StdMutex<()> {
        static JOIN_LINK_TEST_MUTEX: OnceLock<StdMutex<()>> = OnceLock::new();
        JOIN_LINK_TEST_MUTEX.get_or_init(|| StdMutex::new(()))
    }

    fn with_join_link_test_lock<T>(f: impl FnOnce() -> T) -> T {
        let _guard = join_link_test_mutex()
            .lock()
            .expect("join link test mutex lock");
        clear_join_link_nonce_cache();
        let out = f();
        clear_join_link_nonce_cache();
        out
    }

    #[test]
    fn join_link_v1_roundtrip_preserves_fields() {
        with_join_link_test_lock(|| {
            let payload = JoinLinkPayload {
                version: JOIN_LINK_VERSION_LEGACY,
                relay: "127.0.0.1:5000".parse().expect("socket"),
                channel: [0xAB; 32],
                authenticated: true,
                insecure: false,
                server_name: Some("localhost".to_string()),
                torii: Some("http://127.0.0.1:8080/".to_string()),
                pay_to: Some("billing@sora".to_string()),
                kaigi_domain: Some("sora".to_string()),
                kaigi_call_name: Some("standup".to_string()),
                kaigi_privacy_mode: Some("zk".to_string()),
                expires_at_ms: None,
                nonce_hex: None,
                signature_hex: None,
            };

            let link = render_join_link(&payload);
            let decoded = parse_join_link(&link).expect("parse join link");

            assert_eq!(decoded.version, payload.version);
            assert_eq!(decoded.relay, payload.relay);
            assert_eq!(decoded.channel, payload.channel);
            assert_eq!(decoded.authenticated, payload.authenticated);
            assert_eq!(decoded.insecure, payload.insecure);
            assert_eq!(decoded.server_name, payload.server_name);
            assert_eq!(decoded.torii, payload.torii);
            assert_eq!(decoded.pay_to, payload.pay_to);
            assert_eq!(decoded.kaigi_domain, payload.kaigi_domain);
            assert_eq!(decoded.kaigi_call_name, payload.kaigi_call_name);
            assert_eq!(decoded.kaigi_privacy_mode, payload.kaigi_privacy_mode);
            assert_eq!(decoded.expires_at_ms, None);
            assert_eq!(decoded.nonce_hex, None);
            assert_eq!(decoded.signature_hex, None);
        });
    }

    fn signed_join_link_payload(nonce_hex: &str, expires_at_ms: u64) -> JoinLinkPayload {
        let mut payload = JoinLinkPayload {
            version: JOIN_LINK_VERSION_SIGNED,
            relay: "127.0.0.1:5000".parse().expect("socket"),
            channel: [0xCD; 32],
            authenticated: true,
            insecure: false,
            server_name: Some("localhost".to_string()),
            torii: Some("http://127.0.0.1:8080/".to_string()),
            pay_to: Some("billing@sora".to_string()),
            kaigi_domain: Some("sora".to_string()),
            kaigi_call_name: Some("standup".to_string()),
            kaigi_privacy_mode: Some("zk".to_string()),
            expires_at_ms: Some(expires_at_ms),
            nonce_hex: Some(nonce_hex.to_string()),
            signature_hex: None,
        };
        payload.signature_hex = Some(join_link_signature_hex(&payload).expect("signature"));
        payload
    }

    #[test]
    fn join_link_v2_roundtrip_preserves_fields() {
        with_join_link_test_lock(|| {
            let payload = signed_join_link_payload(
                "11".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms() + 60_000,
            );
            let link = render_join_link(&payload);
            let decoded = parse_join_link(&link).expect("parse join link");

            assert_eq!(decoded.version, payload.version);
            assert_eq!(decoded.relay, payload.relay);
            assert_eq!(decoded.channel, payload.channel);
            assert_eq!(decoded.authenticated, payload.authenticated);
            assert_eq!(decoded.insecure, payload.insecure);
            assert_eq!(decoded.server_name, payload.server_name);
            assert_eq!(decoded.torii, payload.torii);
            assert_eq!(decoded.pay_to, payload.pay_to);
            assert_eq!(decoded.kaigi_domain, payload.kaigi_domain);
            assert_eq!(decoded.kaigi_call_name, payload.kaigi_call_name);
            assert_eq!(decoded.kaigi_privacy_mode, payload.kaigi_privacy_mode);
            assert_eq!(decoded.expires_at_ms, payload.expires_at_ms);
            assert_eq!(decoded.nonce_hex, payload.nonce_hex);
            assert_eq!(decoded.signature_hex, payload.signature_hex);
        });
    }

    #[test]
    fn join_link_v2_rejects_replay_nonce() {
        with_join_link_test_lock(|| {
            let payload = signed_join_link_payload(
                "22".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms() + 60_000,
            );
            let link = render_join_link(&payload);
            parse_join_link(&link).expect("first parse should pass");
            let err = parse_join_link(&link).expect_err("second parse should fail replay");
            assert!(err.to_string().contains("replay detected"));
        });
    }

    #[test]
    fn join_link_inspection_parse_does_not_consume_nonce() {
        with_join_link_test_lock(|| {
            let payload = signed_join_link_payload(
                "2a".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms() + 60_000,
            );
            let link = render_join_link(&payload);

            parse_join_link_for_inspection(&link).expect("inspection parse should pass");
            parse_join_link_for_inspection(&link).expect("repeat inspection parse should pass");
            parse_join_link(&link).expect("join parse should still pass after inspection");

            let err = parse_join_link(&link).expect_err("second join parse should fail replay");
            assert!(err.to_string().contains("replay detected"));
        });
    }

    #[test]
    fn join_link_v2_rejects_expired_link() {
        with_join_link_test_lock(|| {
            let payload = signed_join_link_payload(
                "33".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms().saturating_sub(1),
            );
            let link = render_join_link(&payload);
            let err = parse_join_link(&link).expect_err("expired link should fail");
            assert!(err.to_string().contains("expired"));
        });
    }

    #[test]
    fn join_link_v1_rejects_exp_nonce_sig_fields() {
        with_join_link_test_lock(|| {
            let payload = JoinLinkPayload {
                version: JOIN_LINK_VERSION_LEGACY,
                relay: "127.0.0.1:5000".parse().expect("socket"),
                channel: [0xEF; 32],
                authenticated: true,
                insecure: false,
                server_name: Some("localhost".to_string()),
                torii: Some("http://127.0.0.1:8080/".to_string()),
                pay_to: Some("billing@sora".to_string()),
                kaigi_domain: Some("sora".to_string()),
                kaigi_call_name: Some("standup".to_string()),
                kaigi_privacy_mode: Some("zk".to_string()),
                expires_at_ms: Some(now_ms() + 60_000),
                nonce_hex: Some("44".repeat(JOIN_LINK_NONCE_BYTES)),
                signature_hex: Some("55".repeat(32)),
            };
            let link = render_join_link(&payload);
            let err = parse_join_link(&link).expect_err("v1 security fields must be rejected");
            assert!(err
                .to_string()
                .contains("join link v1 must not include exp/nonce/sig fields"));
        });
    }

    #[test]
    fn join_link_v2_rejects_bad_signature() {
        with_join_link_test_lock(|| {
            let mut payload = signed_join_link_payload(
                "66".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms() + 60_000,
            );
            payload.signature_hex = Some("77".repeat(32));
            let link = render_join_link(&payload);
            let err = parse_join_link(&link).expect_err("bad signature should fail");
            assert!(err
                .to_string()
                .contains("join link signature verification failed"));
        });
    }

    #[test]
    fn join_link_v2_rejects_malformed_nonce() {
        with_join_link_test_lock(|| {
            let mut payload = signed_join_link_payload(
                "88".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms() + 60_000,
            );
            payload.nonce_hex = Some("abcd".to_string());
            payload.signature_hex = Some(join_link_signature_hex(&payload).expect("signature"));
            let link = render_join_link(&payload);
            let err = parse_join_link(&link).expect_err("bad nonce should fail");
            assert!(err.to_string().contains("join link nonce must be"));
        });
    }

    #[test]
    fn join_link_v2_rejects_exp_too_far_in_future() {
        with_join_link_test_lock(|| {
            let payload = signed_join_link_payload(
                "99".repeat(JOIN_LINK_NONCE_BYTES).as_str(),
                now_ms().saturating_add(
                    MAX_JOIN_LINK_EXPIRES_IN_SECS
                        .saturating_mul(1000)
                        .saturating_add(60_000),
                ),
            );
            let link = render_join_link(&payload);
            let err = parse_join_link(&link).expect_err("far-future exp should fail");
            assert!(err
                .to_string()
                .contains("join link exp exceeds max future window"));
        });
    }

    #[test]
    fn join_link_nonce_cache_rejects_when_full() {
        with_join_link_test_lock(|| {
            let expires_at_ms = now_ms().saturating_add(60_000);
            for i in 0..MAX_JOIN_LINK_NONCE_CACHE_ENTRIES {
                let nonce_hex = format!("{i:0width$x}", width = JOIN_LINK_NONCE_BYTES * 2);
                register_join_link_nonce_once(&nonce_hex, expires_at_ms)
                    .expect("nonce registration should succeed before cap");
            }
            let cache_len = seen_join_link_nonce_cache()
                .lock()
                .expect("nonce cache lock")
                .len();
            assert_eq!(cache_len, MAX_JOIN_LINK_NONCE_CACHE_ENTRIES);

            let overflow_nonce = format!(
                "{:0width$x}",
                MAX_JOIN_LINK_NONCE_CACHE_ENTRIES,
                width = JOIN_LINK_NONCE_BYTES * 2
            );
            let err = register_join_link_nonce_once(&overflow_nonce, expires_at_ms)
                .expect_err("capacity overflow should be rejected");
            assert!(err.to_string().contains("nonce cache is full"));
        });
    }

    #[test]
    fn parse_target_platform_accepts_aliases() {
        assert_eq!(
            parse_target_platform("web-chromium").expect("parse web-chromium"),
            TargetPlatform::WebChromium
        );
        assert_eq!(
            parse_target_platform("safari").expect("parse safari"),
            TargetPlatform::WebSafari
        );
        assert_eq!(
            parse_target_platform("firefox").expect("parse firefox"),
            TargetPlatform::WebFirefox
        );
        assert_eq!(
            parse_target_platform(" mac_os ").expect("parse mac_os"),
            TargetPlatform::MacOS
        );
        assert_eq!(
            parse_target_platform("ipad").expect("parse ipad"),
            TargetPlatform::IPadOS
        );
        assert_eq!(
            parse_target_platform("win").expect("parse win"),
            TargetPlatform::Windows
        );
    }

    #[test]
    fn parse_target_platform_rejects_unknown_value() {
        let err = parse_target_platform("ps5").expect_err("unknown platform must fail");
        assert!(err.to_string().contains("unsupported --platform value"));
    }

    #[test]
    fn platform_contract_command_parses_platform_and_pretty_flags() {
        let cli = Cli::try_parse_from([
            "kaigi-cli",
            "platform-contract",
            "--platform",
            "web-safari",
            "--pretty",
        ])
        .expect("parse platform-contract command");
        match cli.cmd {
            Command::PlatformContract(args) => {
                assert_eq!(args.platform, Some("web-safari".to_string()));
                assert!(args.pretty);
            }
            _ => panic!("expected platform-contract command"),
        }
    }

    #[test]
    fn percent_codec_roundtrip_handles_symbols() {
        let original = "http://127.0.0.1:8080/v1/config?x=1+2&y=z";
        let encoded = pct_encode(original);
        let decoded = pct_decode(&encoded).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn privacy_mode_normalization_maps_zk_alias() {
        assert_eq!(
            normalize_privacy_mode(Some("zk_roster_v1".to_string())),
            Some("zk".to_string())
        );
        assert_eq!(
            normalize_privacy_mode(Some("zk-roster-v1".to_string())),
            Some("zk".to_string())
        );
        assert!(is_anonymous_mode_enabled(Some("zk")));
        assert!(is_anonymous_mode_enabled(Some("zk-roster-v1")));
        assert!(!is_anonymous_mode_enabled(Some("transparent")));
    }

    #[test]
    fn privacy_mode_validation_accepts_hyphen_alias() {
        validate_privacy_mode_arg(Some("zk-roster-v1")).expect("hyphen alias should pass");
    }

    #[test]
    fn lifecycle_create_pricing_applies_zk_surcharge() {
        let (privacy_mode, effective_rate) =
            resolve_lifecycle_create_pricing("zk_roster_v1".to_string(), 100, 25)
                .expect("zk alias should be accepted");
        assert_eq!(privacy_mode, "zk");
        assert_eq!(effective_rate, 125);
    }

    #[test]
    fn lifecycle_create_pricing_accepts_uppercase_alias() {
        let (privacy_mode, effective_rate) =
            resolve_lifecycle_create_pricing(" ZK_ROSTER_V1 ".to_string(), 100, 25)
                .expect("zk alias should be accepted");
        assert_eq!(privacy_mode, "zk");
        assert_eq!(effective_rate, 125);
    }

    #[test]
    fn lifecycle_create_pricing_accepts_hyphen_alias() {
        let (privacy_mode, effective_rate) =
            resolve_lifecycle_create_pricing("zk-roster-v1".to_string(), 100, 25)
                .expect("hyphen alias should be accepted");
        assert_eq!(privacy_mode, "zk");
        assert_eq!(effective_rate, 125);
    }

    #[test]
    fn lifecycle_create_pricing_keeps_transparent_rate() {
        let (privacy_mode, effective_rate) =
            resolve_lifecycle_create_pricing("transparent".to_string(), 100, 0)
                .expect("transparent without surcharge should pass");
        assert_eq!(privacy_mode, "transparent");
        assert_eq!(effective_rate, 100);
    }

    #[test]
    fn lifecycle_create_pricing_rejects_overflow() {
        let err = resolve_lifecycle_create_pricing("zk".to_string(), u64::MAX - 1, 25)
            .expect_err("overflow must be rejected");
        assert!(err.to_string().contains("pricing overflow"));
    }

    #[test]
    fn lifecycle_create_pricing_rejects_transparent_surcharge() {
        let err = resolve_lifecycle_create_pricing("transparent".to_string(), 100, 1)
            .expect_err("transparent surcharge must be rejected");
        assert!(err.to_string().contains("--privacy-mode zk"));
    }

    #[test]
    fn lifecycle_create_pricing_rejects_unknown_privacy_mode() {
        let err = resolve_lifecycle_create_pricing("private".to_string(), 100, 0)
            .expect_err("unknown mode must be rejected");
        assert!(err.to_string().contains("unsupported privacy mode"));
    }

    #[test]
    fn lifecycle_create_pricing_rejects_empty_privacy_mode() {
        let err = resolve_lifecycle_create_pricing("   ".to_string(), 100, 0)
            .expect_err("empty mode must be rejected");
        assert!(err.to_string().contains("unsupported privacy mode"));
    }

    #[test]
    fn lifecycle_create_cli_parse_accepts_uppercase_alias() {
        let args = KaigiLifecycleArgs::try_parse_from([
            "kaigi-lifecycle",
            "--iroha-config",
            "/tmp/iroha.toml",
            "create",
            "--domain",
            "sora",
            "--call-name",
            "standup",
            "--host",
            "alice@sora",
            "--privacy-mode",
            "ZK_ROSTER_V1",
        ])
        .expect("parse kaigi-lifecycle");
        match args.cmd {
            KaigiLifecycleCommand::Create { privacy_mode, .. } => {
                assert_eq!(privacy_mode, "ZK_ROSTER_V1");
            }
            _ => panic!("expected create command"),
        }
    }

    #[test]
    fn lifecycle_create_cli_parse_accepts_unknown_mode_for_runtime_validation() {
        let args = KaigiLifecycleArgs::try_parse_from([
            "kaigi-lifecycle",
            "--iroha-config",
            "/tmp/iroha.toml",
            "create",
            "--domain",
            "sora",
            "--call-name",
            "standup",
            "--host",
            "alice@sora",
            "--privacy-mode",
            "private",
        ])
        .expect("parse kaigi-lifecycle");
        match args.cmd {
            KaigiLifecycleCommand::Create { privacy_mode, .. } => {
                assert_eq!(privacy_mode, "private");
            }
            _ => panic!("expected create command"),
        }
    }

    #[test]
    fn lifecycle_create_cli_parse_accepts_auth_room_policy_alias() {
        let args = KaigiLifecycleArgs::try_parse_from([
            "kaigi-lifecycle",
            "--iroha-config",
            "/tmp/iroha.toml",
            "create",
            "--domain",
            "sora",
            "--call-name",
            "standup",
            "--host",
            "alice@sora",
            "--room-policy",
            "auth",
        ])
        .expect("parse kaigi-lifecycle");
        match args.cmd {
            KaigiLifecycleCommand::Create { room_policy, .. } => {
                assert_eq!(room_policy, "auth");
            }
            _ => panic!("expected create command"),
        }
    }

    #[test]
    fn lifecycle_create_builder_normalizes_room_policy_and_privacy_aliases() {
        let cmd = build_kaigi_lifecycle_cli_args(KaigiLifecycleCommand::Create {
            domain: "sora".to_string(),
            call_name: "standup".to_string(),
            host: "alice@sora".to_string(),
            room_policy: "AUTH".to_string(),
            privacy_mode: "zk-roster-v1".to_string(),
            gas_rate_per_minute: 100,
            zk_extra_fee_per_minute_nano: 25,
        })
        .expect("builder should succeed");
        assert_eq!(
            cmd,
            svec(&[
                "app",
                "kaigi",
                "create",
                "--domain",
                "sora",
                "--call-name",
                "standup",
                "--host",
                "alice@sora",
                "--room-policy",
                "authenticated",
                "--privacy-mode",
                "zk",
                "--gas-rate-per-minute",
                "125",
            ])
        );
    }

    #[test]
    fn lifecycle_join_builder_normalizes_hex_fields() {
        let cmd = build_kaigi_lifecycle_cli_args(KaigiLifecycleCommand::Join {
            domain: "sora".to_string(),
            call_name: "standup".to_string(),
            participant: "alice@sora".to_string(),
            commitment_hex: Some("0XAB".to_string()),
            commitment_alias: Some("alias-1".to_string()),
            nullifier_hex: Some("0XCD".to_string()),
            nullifier_issued_at_ms: Some(7),
            roster_root_hex: Some("0XEF".to_string()),
            proof_hex: Some("0X12".to_string()),
        })
        .expect("builder should succeed");
        assert_eq!(
            cmd,
            svec(&[
                "app",
                "kaigi",
                "join",
                "--domain",
                "sora",
                "--call-name",
                "standup",
                "--participant",
                "alice@sora",
                "--commitment-hex",
                "ab",
                "--commitment-alias",
                "alias-1",
                "--nullifier-hex",
                "cd",
                "--nullifier-issued-at-ms",
                "7",
                "--roster-root-hex",
                "ef",
                "--proof-hex",
                "12",
            ])
        );
    }

    #[test]
    fn lifecycle_record_usage_builder_normalizes_hex_fields() {
        let cmd = build_kaigi_lifecycle_cli_args(KaigiLifecycleCommand::RecordUsage {
            domain: "sora".to_string(),
            call_name: "standup".to_string(),
            duration_ms: 1_000,
            billed_gas: 5,
            usage_commitment_hex: Some("0XAB".to_string()),
            proof_hex: Some("0XCD".to_string()),
        })
        .expect("builder should succeed");
        assert_eq!(
            cmd,
            svec(&[
                "app",
                "kaigi",
                "record-usage",
                "--domain",
                "sora",
                "--call-name",
                "standup",
                "--duration-ms",
                "1000",
                "--billed-gas",
                "5",
                "--usage-commitment-hex",
                "ab",
                "--proof-hex",
                "cd",
            ])
        );
    }

    #[test]
    fn lifecycle_join_builder_rejects_alias_without_commitment() {
        let err = build_kaigi_lifecycle_cli_args(KaigiLifecycleCommand::Join {
            domain: "sora".to_string(),
            call_name: "standup".to_string(),
            participant: "alice@sora".to_string(),
            commitment_hex: None,
            commitment_alias: Some("alias-1".to_string()),
            nullifier_hex: None,
            nullifier_issued_at_ms: None,
            roster_root_hex: None,
            proof_hex: None,
        })
        .expect_err("alias-only join must fail");
        assert!(
            err.to_string()
                .contains("--commitment-alias requires --commitment-hex")
        );
    }

    #[test]
    fn anon_join_bootstrap_emits_only_anon_hello() {
        let frames = build_initial_anon_join_frames("anon-a".to_string(), "11".repeat(32));
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            KaigiFrame::AnonHello(hello) => {
                assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
                assert_eq!(hello.participant_handle, "anon-a");
                assert_eq!(hello.x25519_pubkey_hex, "11".repeat(32));
            }
            other => panic!("unexpected bootstrap frame: {other:?}"),
        }
    }

    #[test]
    fn anon_escrow_quote_applies_ceil_minute_surcharge() {
        let channel_hex = "44".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--anon-escrow-prepay-nano",
            "1000",
            "--anon-zk-extra-fee-per-minute-nano",
            "200",
            "--anon-expected-duration-secs",
            "61",
        ])
        .expect("parse room-chat");
        assert_eq!(
            quote_anon_escrow_total_nano_checked(&args).expect("quote"),
            1400
        );
    }

    #[test]
    fn random_anon_handle_matches_validation_rules() {
        let handle = random_anon_handle();
        assert!(handle.starts_with("anon-"));
        validate_anonymous_participant_handle(&handle).expect("generated handle should be valid");
    }

    #[test]
    fn anon_escrow_quote_without_surcharge_equals_base() {
        let channel_hex = "55".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--anon-escrow-prepay-nano",
            "777",
        ])
        .expect("parse room-chat");
        assert_eq!(
            quote_anon_escrow_total_nano_checked(&args).expect("quote"),
            777
        );
    }

    #[test]
    fn anon_escrow_quote_checked_rejects_overflow() {
        let channel_hex = "56".repeat(32);
        let max = u64::MAX.to_string();
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            max.as_str(),
            "--anon-zk-extra-fee-per-minute-nano",
            max.as_str(),
            "--anon-expected-duration-secs",
            max.as_str(),
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected overflow");
        assert!(err.to_string().contains("overflow"));
    }

    #[test]
    fn anon_escrow_flags_require_zk_privacy_mode() {
        let channel_hex = "66".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--anon-escrow-prepay-nano",
            "100",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, false).expect_err("expected error");
        assert!(err.to_string().contains("--kaigi-privacy-mode zk"));
    }

    #[test]
    fn anon_surcharge_requires_prepay() {
        let channel_hex = "77".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-zk-extra-fee-per-minute-nano",
            "10",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--anon-escrow-prepay-nano"));
    }

    #[test]
    fn anon_surcharge_requires_positive_expected_duration() {
        let channel_hex = "78".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--anon-zk-extra-fee-per-minute-nano",
            "10",
            "--anon-expected-duration-secs",
            "0",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-expected-duration-secs > 0")
        );
    }

    #[test]
    fn anon_mode_rejects_payment_rate_flags() {
        let channel_hex = "79".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--pay-rate-per-minute-nano",
            "1",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("unsupported in anonymous mode"));
    }

    #[test]
    fn anon_mode_rejects_pay_auto_flags() {
        let channel_hex = "7d".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--pay-auto",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--pay-auto/--no-pay-auto"));
    }

    #[test]
    fn anon_mode_rejects_no_pay_auto_flags() {
        let channel_hex = "7c".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--no-pay-auto",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--pay-auto/--no-pay-auto"));
    }

    #[test]
    fn anon_mode_rejects_unsettled_payment_flags() {
        let channel_hex = "7b".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--allow-unsettled-payments",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("unsupported in anonymous mode"));
    }

    #[test]
    fn anon_mode_rejects_empty_participant_handle() {
        let channel_hex = "7e".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--participant-id",
            " ",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--participant-id must be non-empty in anonymous mode")
        );
    }

    #[test]
    fn anon_mode_rejects_whitespace_participant_handle() {
        let channel_hex = "70".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--participant-id",
            "anon a",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--participant-id must not contain whitespace/control chars")
        );
    }

    #[test]
    fn anon_mode_rejects_non_ascii_participant_handle() {
        let channel_hex = "71".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--participant-id",
            "匿名",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--participant-id must be ASCII in anonymous mode")
        );
    }

    #[test]
    fn anon_mode_rejects_account_like_participant_handle() {
        let channel_hex = "73".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--participant-id",
            "alice@sora",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--participant-id must not contain '@' in anonymous mode")
        );
    }

    #[test]
    fn anon_mode_rejects_oversized_participant_handle() {
        let channel_hex = "7f".repeat(32);
        let oversized = "a".repeat(MAX_ANON_PARTICIPANT_HANDLE_LEN + 1);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--participant-id",
            oversized.as_str(),
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("too long"));
    }

    #[test]
    fn anon_escrow_id_rejects_empty_value() {
        let channel_hex = "7a".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-id",
            " ",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-escrow-id must be non-empty")
        );
    }

    #[test]
    fn anon_escrow_id_rejects_whitespace_chars() {
        let channel_hex = "70".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-id",
            "escrow id",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-escrow-id must not contain")
        );
    }

    #[test]
    fn anon_escrow_id_rejects_non_ascii_chars() {
        let channel_hex = "72".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-id",
            "匿名",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--anon-escrow-id must be ASCII"));
    }

    #[test]
    fn anon_escrow_id_rejects_account_like_chars() {
        let channel_hex = "74".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-id",
            "escrow@sora",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-escrow-id must not contain '@'")
        );
    }

    #[test]
    fn anon_escrow_id_rejects_oversized_value() {
        let channel_hex = "7b".repeat(32);
        let oversized = "x".repeat(MAX_ESCROW_ID_LEN + 1);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-id",
            oversized.as_str(),
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("max"));
    }

    #[test]
    fn anon_escrow_proof_hex_rejects_invalid_value() {
        let channel_hex = "7c".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-proof-hex",
            "zz",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-escrow-proof-hex must be valid non-empty hex")
        );
    }

    #[test]
    fn anon_escrow_proof_hex_rejects_oversized_value() {
        let oversized = "ab".repeat((MAX_ESCROW_PROOF_HEX_LEN / 2).saturating_add(1));
        let err = validate_anon_proof_hex(oversized.as_str(), "--anon-escrow-proof-hex")
            .expect_err("expected oversize error");
        assert!(err.to_string().contains("too long"));
    }

    #[test]
    fn anon_escrow_proof_hex_accepts_prefixed_hex() {
        validate_anon_proof_hex("0xabcdef", "--anon-escrow-proof-hex")
            .expect("prefixed hex should be accepted");
    }

    #[test]
    fn anon_escrow_proof_hex_accepts_uppercase_prefixed_hex() {
        validate_anon_proof_hex("0Xabcdef", "--anon-escrow-proof-hex")
            .expect("prefixed hex should be accepted");
    }

    #[test]
    fn anon_prepay_requires_iroha_config_and_from() {
        let channel_hex = "88".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--pay-iroha-config"));
    }

    #[test]
    fn anon_unshield_requires_required_inputs() {
        let channel_hex = "99".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--anon-unshield-on-exit",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--pay-iroha-config"));
    }

    #[test]
    fn anon_unshield_requires_non_zero_prepay() {
        let channel_hex = "9a".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-unshield-on-exit",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--anon-unshield-to",
            "alice@sora",
            "--anon-unshield-inputs",
            "ab",
            "--anon-unshield-proof-json",
            "/tmp/proof.json",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--anon-escrow-prepay-nano > 0"));
    }

    #[test]
    fn anon_unshield_args_require_unshield_flag() {
        let channel_hex = "9b".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-unshield-to",
            "alice@sora",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(err.to_string().contains("--anon-unshield-on-exit"));
    }

    #[test]
    fn anon_unshield_root_hint_requires_hex32() {
        let channel_hex = "9c".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
            "--anon-unshield-on-exit",
            "--anon-unshield-to",
            "alice@sora",
            "--anon-unshield-inputs",
            "abababababababababababababababababababababababababababababababab",
            "--anon-unshield-proof-json",
            "/tmp/proof.json",
            "--anon-unshield-root-hint-hex",
            "xyz",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-unshield-root-hint-hex must be 32-byte hex")
        );
    }

    #[test]
    fn anon_unshield_inputs_require_hex32_list() {
        let channel_hex = "9d".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
            "--anon-unshield-on-exit",
            "--anon-unshield-to",
            "alice@sora",
            "--anon-unshield-inputs",
            "ab,cd",
            "--anon-unshield-proof-json",
            "/tmp/proof.json",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-unshield-inputs item 1 must be 32-byte hex")
        );
    }

    #[test]
    fn anon_unshield_inputs_reject_duplicate_nullifiers() {
        let channel_hex = "9f".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
            "--anon-unshield-on-exit",
            "--anon-unshield-to",
            "alice@sora",
            "--anon-unshield-inputs",
            "0xabababababababababababababababababababababababababababababababab,ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB",
            "--anon-unshield-proof-json",
            "/tmp/proof.json",
        ])
        .expect("parse room-chat");
        let err = validate_anonymous_escrow_settings(&args, true).expect_err("expected error");
        assert!(
            err.to_string()
                .contains("--anon-unshield-inputs item 2 duplicates a previous nullifier")
        );
    }

    #[test]
    fn anon_unshield_inputs_reject_too_many_entries() {
        let oversized = (0..=MAX_ANON_UNSHIELD_INPUTS)
            .map(|i| format!("{i:064x}"))
            .collect::<Vec<_>>()
            .join(",");
        let err = validate_unshield_inputs_arg(&oversized).expect_err("must reject oversized");
        assert!(
            err.to_string()
                .contains("--anon-unshield-inputs has too many entries")
        );
    }

    #[test]
    fn anon_unshield_validation_accepts_hex32_inputs_and_root_hint() {
        let channel_hex = "9e".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
            "--anon-unshield-on-exit",
            "--anon-unshield-to",
            "alice@sora",
            "--anon-unshield-inputs",
            "0xabababababababababababababababababababababababababababababababab,CDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCD",
            "--anon-unshield-proof-json",
            "/tmp/proof.json",
            "--anon-unshield-root-hint-hex",
            "0Xefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef",
        ])
        .expect("parse room-chat");
        validate_anonymous_escrow_settings(&args, true).expect("validation should pass");
    }

    #[test]
    fn anon_unshield_inputs_normalization_canonicalizes_prefix_and_case() {
        let normalized = normalize_hex32_csv(
            "0xABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB,CDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCDCD",
        )
        .expect("inputs should normalize");
        assert_eq!(
            normalized,
            "abababababababababababababababababababababababababababababababab,cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
        );
    }

    #[test]
    fn anon_unshield_inputs_normalization_rejects_invalid_values() {
        let err = normalize_hex32_csv("ab,cd").expect_err("invalid inputs should fail");
        assert!(
            err.to_string()
                .contains("--anon-unshield-inputs item 1 must be 32-byte hex")
        );
    }

    #[test]
    fn anon_prepay_validation_accepts_complete_args() {
        let channel_hex = "aa".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-privacy-mode",
            "zk",
            "--anon-escrow-prepay-nano",
            "100",
            "--pay-iroha-config",
            "/tmp/iroha.toml",
            "--pay-from",
            "alice@sora",
        ])
        .expect("parse room-chat");
        validate_anonymous_escrow_settings(&args, true).expect("validation should pass");
    }

    #[test]
    fn room_chat_enables_pay_auto_by_default() {
        let channel_hex = "11".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
        ])
        .expect("parse room-chat");

        assert!(is_pay_auto_enabled(&args));
    }

    #[test]
    fn room_chat_can_disable_pay_auto() {
        let channel_hex = "22".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--no-pay-auto",
        ])
        .expect("parse room-chat");

        assert!(!is_pay_auto_enabled(&args));
    }

    #[test]
    fn room_chat_rejects_conflicting_pay_auto_flags() {
        let channel_hex = "33".repeat(32);
        let res = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--pay-auto",
            "--no-pay-auto",
        ]);
        assert!(res.is_err());
    }

    #[test]
    fn deterministic_signature_hex_is_stable_and_tag_sensitive() {
        let a = deterministic_signature_hex("session_policy", "alice|1|payload");
        let b = deterministic_signature_hex("session_policy", "alice|1|payload");
        let c = deterministic_signature_hex("role_grant", "alice|1|payload");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn session_policy_signature_hex_binds_e2ee_required_flag() {
        let mut policy = LocalSessionPolicyState::default();
        policy.policy_epoch = 3;
        policy.max_participants = 250;
        let with_e2ee = session_policy_signature_hex("host@sora", &policy, 100);
        policy.e2ee_required = false;
        let without_e2ee = session_policy_signature_hex("host@sora", &policy, 100);
        assert_ne!(with_e2ee, without_e2ee);
    }

    #[test]
    fn signed_moderation_frame_emits_signature_bound_payload() {
        let frame = signed_moderation_frame(
            "host@sora",
            7,
            ModerationTarget::Participant("alice@sora".to_string()),
            ModerationAction::DisableVideo,
        );
        let KaigiFrame::ModerationSigned(moderation) = frame else {
            panic!("expected ModerationSigned");
        };
        assert_eq!(moderation.issued_by, "host@sora");
        assert_eq!(moderation.sent_at_ms, 7);
        assert_eq!(
            moderation.signature_hex,
            deterministic_signature_hex(
                "moderation",
                "host@sora|participant:alice@sora|disable_video|7"
            )
        );
    }

    #[test]
    fn local_session_policy_defaults_match_expected_values() {
        let policy = LocalSessionPolicyState::default();
        assert!(!policy.room_lock);
        assert!(!policy.waiting_room_enabled);
        assert!(policy.guest_join_allowed);
        assert!(policy.local_recording_allowed);
        assert!(policy.e2ee_required);
        assert_eq!(policy.max_participants, DEFAULT_POLICY_MAX_PARTICIPANTS);
        assert_eq!(policy.policy_epoch, 0);
    }

    #[test]
    fn auto_lifecycle_overrides_require_iroha_config() {
        let channel_hex = "44".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-record-usage",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected missing config error");
        assert!(err.to_string().contains(
            "--kaigi-iroha-config is required when using Kaigi lifecycle mirroring flags"
        ));
    }

    #[test]
    fn auto_lifecycle_absent_when_no_config_and_no_overrides() {
        let channel_hex = "55".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
        ])
        .expect("parse room-chat");

        let lifecycle = build_auto_kaigi_lifecycle(&args).expect("should not fail");
        assert!(lifecycle.is_none());
    }

    #[test]
    fn auto_lifecycle_metadata_without_config_is_ignored() {
        let channel_hex = "66".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
        ])
        .expect("parse room-chat");

        let lifecycle = build_auto_kaigi_lifecycle(&args).expect("should not fail");
        assert!(lifecycle.is_none());
    }

    #[test]
    fn auto_lifecycle_participant_without_config_errors() {
        let channel_hex = "99".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-participant",
            "alice@sora",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected missing config error");
        assert!(err.to_string().contains(
            "--kaigi-iroha-config is required when using Kaigi lifecycle mirroring flags"
        ));
    }

    #[test]
    fn auto_lifecycle_usage_fields_require_record_usage() {
        let channel_hex = "77".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-billed-gas",
            "100",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected usage guard");
        assert!(err.to_string().contains("--kaigi-record-usage"));
    }

    #[test]
    fn auto_lifecycle_record_usage_allows_usage_fields() {
        let channel_hex = "88".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-record-usage",
            "--kaigi-billed-gas",
            "100",
            "--kaigi-usage-commitment-hex",
            "ab",
            "--kaigi-usage-proof-hex",
            "cd",
        ])
        .expect("parse room-chat");

        let lifecycle = build_auto_kaigi_lifecycle(&args).expect("should pass");
        assert!(lifecycle.is_some());
    }

    #[test]
    fn auto_lifecycle_join_alias_requires_join_commitment() {
        let channel_hex = "aa".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-join-commitment-alias",
            "alias-1",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected dependency error");
        assert!(err.to_string().contains("--kaigi-join-commitment-alias"));
    }

    #[test]
    fn auto_lifecycle_nullifier_timestamp_requires_nullifier() {
        let channel_hex = "ab".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-nullifier-issued-at-ms",
            "1",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected dependency error");
        assert!(
            err.to_string()
                .contains("--kaigi-nullifier-issued-at-ms requires --kaigi-join-nullifier-hex or --kaigi-leave-nullifier-hex")
        );
    }

    #[test]
    fn auto_lifecycle_nullifier_timestamp_with_leave_nullifier_passes() {
        let channel_hex = "ac".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-leave-nullifier-hex",
            "ab",
            "--kaigi-nullifier-issued-at-ms",
            "1",
        ])
        .expect("parse room-chat");

        let lifecycle = build_auto_kaigi_lifecycle(&args).expect("validation should pass");
        assert!(lifecycle.is_some());
    }

    #[test]
    fn auto_lifecycle_rejects_invalid_hex_payload_field() {
        let channel_hex = "ad".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-join-proof-hex",
            "zz",
        ])
        .expect("parse room-chat");

        let err = build_auto_kaigi_lifecycle(&args).expect_err("expected invalid hex");
        assert!(
            err.to_string()
                .contains("--kaigi-join-proof-hex must be valid non-empty hex")
        );
    }

    #[test]
    fn auto_lifecycle_normalizes_hex_payload_fields() {
        let channel_hex = "ae".repeat(32);
        let args = RoomChatArgs::try_parse_from([
            "room-chat",
            "--relay",
            "127.0.0.1:5000",
            "--insecure",
            "--channel",
            channel_hex.as_str(),
            "--kaigi-iroha-config",
            "/tmp/iroha.toml",
            "--kaigi-domain",
            "sora",
            "--kaigi-call-name",
            "townhall",
            "--kaigi-participant",
            "alice@sora",
            "--kaigi-join-commitment-hex",
            "0XAB",
            "--kaigi-join-nullifier-hex",
            "0XCD",
            "--kaigi-leave-commitment-hex",
            "0XEF",
            "--kaigi-leave-nullifier-hex",
            "0X12",
            "--kaigi-roster-root-hex",
            "0X34",
            "--kaigi-join-proof-hex",
            "0X56",
            "--kaigi-leave-proof-hex",
            "0X78",
            "--kaigi-record-usage",
            "--kaigi-usage-commitment-hex",
            "0X9A",
            "--kaigi-usage-proof-hex",
            "0XBC",
        ])
        .expect("parse room-chat");

        let lifecycle = build_auto_kaigi_lifecycle(&args)
            .expect("validation should pass")
            .expect("lifecycle should be present");
        assert_eq!(lifecycle.join_commitment_hex.as_deref(), Some("ab"));
        assert_eq!(lifecycle.join_nullifier_hex.as_deref(), Some("cd"));
        assert_eq!(lifecycle.leave_commitment_hex.as_deref(), Some("ef"));
        assert_eq!(lifecycle.leave_nullifier_hex.as_deref(), Some("12"));
        assert_eq!(lifecycle.roster_root_hex.as_deref(), Some("34"));
        assert_eq!(lifecycle.join_proof_hex.as_deref(), Some("56"));
        assert_eq!(lifecycle.leave_proof_hex.as_deref(), Some("78"));
        assert_eq!(lifecycle.usage_commitment_hex.as_deref(), Some("9a"));
        assert_eq!(lifecycle.usage_proof_hex.as_deref(), Some("bc"));
    }

    #[test]
    fn commitment_alias_dependency_requires_commitment() {
        let err = validate_commitment_alias_dependency(
            None,
            Some("alias-1"),
            "--commitment-hex",
            "--commitment-alias",
        )
        .expect_err("alias without commitment must fail");
        assert!(
            err.to_string()
                .contains("--commitment-alias requires --commitment-hex")
        );
    }

    #[test]
    fn nullifier_timestamp_dependency_requires_nullifier() {
        let err = validate_nullifier_timestamp_dependency(
            None,
            Some(1),
            "--nullifier-hex",
            "--nullifier-issued-at-ms",
        )
        .expect_err("timestamp without nullifier must fail");
        assert!(
            err.to_string()
                .contains("--nullifier-issued-at-ms requires --nullifier-hex")
        );
    }

    #[test]
    fn optional_hex_validation_rejects_invalid_and_accepts_prefixed() {
        let err = validate_optional_hex_arg(Some("zz"), "--proof-hex")
            .expect_err("invalid hex must fail");
        assert!(
            err.to_string()
                .contains("--proof-hex must be valid non-empty hex")
        );
        validate_optional_hex_arg(Some("0Xab"), "--proof-hex").expect("prefixed hex should pass");
        validate_optional_hex_arg(None, "--proof-hex").expect("none should pass");
    }

    #[test]
    fn optional_hex_normalization_canonicalizes_prefix_and_case() {
        assert_eq!(
            normalize_optional_hex_owned(Some("0XAB".to_string())),
            Some("ab".to_string())
        );
        assert_eq!(normalize_optional_hex_owned(None), None);
    }

    #[test]
    fn room_policy_normalization_accepts_alias_and_rejects_unknown() {
        assert_eq!(
            normalize_room_policy_arg("auth".to_string()).expect("alias should normalize"),
            "authenticated"
        );
        assert_eq!(
            normalize_room_policy_arg("PUBLIC".to_string()).expect("public should normalize"),
            "public"
        );
        let err =
            normalize_room_policy_arg("private".to_string()).expect_err("unknown policy must fail");
        assert!(err.to_string().contains("unsupported room policy"));
    }

    #[test]
    fn host_state_classification_matches_expected_roles() {
        assert_eq!(classify_host_state(None, "alice"), HOST_STATE_UNKNOWN);
        assert_eq!(classify_host_state(Some("alice"), "alice"), HOST_STATE_SELF);
        assert_eq!(classify_host_state(Some("bob"), "alice"), HOST_STATE_OTHER);
    }

    #[test]
    fn end_command_gate_respects_host_state() {
        assert_eq!(end_command_gate_message(HOST_STATE_SELF), None);
        assert_eq!(
            end_command_gate_message(HOST_STATE_OTHER),
            Some("error: /end is host-only")
        );
        assert_eq!(
            end_command_gate_message(HOST_STATE_UNKNOWN),
            Some("error: host role unknown; wait for room_config before /end")
        );
    }

    #[test]
    fn leave_skips_only_after_successful_end() {
        assert!(should_skip_leave_after_end(true, true));
        assert!(!should_skip_leave_after_end(true, false));
        assert!(!should_skip_leave_after_end(false, true));
        assert!(!should_skip_leave_after_end(false, false));
    }

    #[test]
    fn nexus_routing_requires_torii_by_default() {
        let err = validate_nexus_routing_requirement(false, None).expect_err("torii required");
        let msg = err.to_string();
        assert!(msg.contains("--torii"));
        assert!(msg.contains("--allow-local-handshake"));
    }

    #[test]
    fn nexus_routing_allows_torii_source() {
        validate_nexus_routing_requirement(false, Some("http://127.0.0.1:8080"))
            .expect("torii accepted");
    }

    #[test]
    fn nexus_routing_allows_local_override() {
        validate_nexus_routing_requirement(true, None).expect("local override accepted");
    }
}
