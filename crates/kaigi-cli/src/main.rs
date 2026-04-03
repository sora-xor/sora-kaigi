use std::{
    collections::{HashMap, HashSet},
    io::{Read as _, Write as _},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
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
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use kaigi_platform_contract::{TargetPlatform, all_platform_contracts, platform_contract};
use kaigi_soranet_client::{
    HandshakeParams, RelayConnectOptions, connect_and_handshake, decode_hex_32, decode_hex_vec,
    derive_kaigi_room_id, fetch_handshake_params_from_torii, open_kaigi_stream,
};
use kaigi_wire::{
    AnonEncryptedPayloadFrame, AnonHelloFrame, AnonRosterFrame, AnonymousPayloadKind,
    AudioCodecKind, AudioPacketFrame, ChatFrame, DeviceCapabilityFrame, E2EEKeyEpochFrame,
    EncryptedControlFrame, EncryptedControlKind, EncryptedRecipientPayload, EscrowProofFrame,
    FrameDecoder, GroupKeyUpdateFrame, HelloFrame, KaigiFrame, KeyRotationAckFrame,
    MAX_ANON_PARTICIPANT_HANDLE_LEN, MAX_ESCROW_ID_LEN, MAX_ESCROW_PROOF_HEX_LEN,
    MediaCapabilityFrame, MediaProfileKind, MediaProfileNegotiationFrame, MediaTrackKind,
    MediaTrackStateFrame, ModerationAction, ModerationSignedFrame, ModerationTarget,
    PROTOCOL_VERSION, ParticipantStateFrame, PaymentFrame, PermissionsSnapshotFrame, PingFrame,
    RecordingNoticeFrame, RecordingState, RoleGrantFrame, RoleKind, RoleRevokeFrame,
    RoomConfigUpdateFrame, RoomEventFrame, SessionPolicyFrame, VideoCodecKind, VideoSegmentFrame,
    encode_framed,
};
use norito::{
    decode_from_bytes,
    streaming::{
        AudioFrame, AudioLayout, PrivacyRouteUpdate, SoranetAccessKind, SoranetChannelId,
        SoranetRoute, SoranetStreamTag,
        chunk::BaselineDecoder,
        codec::{
            AudioDecoder, AudioEncoder, AudioEncoderConfig, BaselineEncoder, BaselineEncoderConfig,
            FrameDimensions, RawFrame, SegmentBundle,
        },
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

    /// Connect to a relay, open a Kaigi stream, and run conference mode (`--tui` for fullscreen ASCII).
    RoomChat(RoomChatArgs),

    /// Join a meeting and run the cyberpunk ASCII live console renderer.
    AsciiLive(RoomChatArgs),

    /// Render a local video file as cyberpunk ASCII in the terminal (ffmpeg required).
    AsciiPlay(AsciiPlayArgs),

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

    /// Launch fullscreen ASCII TUI conferencing mode.
    #[arg(long)]
    tui: bool,

    /// Force legacy slash-command mode (debug fallback).
    #[arg(long, hide = true)]
    legacy_ui: bool,
}

#[derive(Parser, Debug, Clone)]
struct AsciiPlayArgs {
    /// Input video path.
    #[arg(long)]
    input: PathBuf,

    /// ffmpeg executable path.
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg_bin: String,

    /// Output width in pixels before ASCII conversion.
    #[arg(long, default_value_t = 96)]
    width: u16,

    /// Output height in pixels before ASCII conversion.
    #[arg(long, default_value_t = 54)]
    height: u16,

    /// Output ASCII frame rate.
    #[arg(long, default_value_t = 18)]
    fps: u16,

    /// Density preset (0..4).
    #[arg(long, default_value_t = 2)]
    density: usize,

    /// Initial theme index (0=MATRIX, 1=NEON-ICE, 2=SYNTHWAVE, 3=BLADE).
    #[arg(long, default_value_t = 0)]
    theme: usize,

    /// Enable glitch pass while rendering.
    #[arg(long)]
    glitch: bool,

    /// Loop playback forever.
    #[arg(long = "loop")]
    loop_playback: bool,
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
        Command::AsciiLive(mut args) => {
            args.tui = true;
            args.legacy_ui = false;
            room_chat_conference(args).await?;
        }
        Command::AsciiPlay(args) => {
            ascii_play(args)?;
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
        "visionos" | "vision-os" | "vision" => TargetPlatform::VisionOS,
        "windows" | "win" => TargetPlatform::Windows,
        "android" => TargetPlatform::Android,
        "linux" => TargetPlatform::Linux,
        _ => {
            return Err(anyhow!(
                "unsupported --platform value `{raw}`; expected one of web-chromium, web-safari, web-firefox, macos, ios, ipados, visionos, windows, android, linux"
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

fn populate_join_link_security_fields(
    payload: &mut JoinLinkPayload,
    expires_in_secs: u64,
) -> Result<()> {
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

fn validate_join_link_security(
    payload: &JoinLinkPayload,
    consume_nonce_replay: bool,
) -> Result<()> {
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

fn ascii_play(args: AsciiPlayArgs) -> Result<()> {
    if args.width < 8 || args.height < 8 {
        return Err(anyhow!("--width/--height must be at least 8"));
    }
    if args.fps == 0 {
        return Err(anyhow!("--fps must be >= 1"));
    }
    let source = args
        .input
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("video");
    let frame_bytes = usize::from(args.width).saturating_mul(usize::from(args.height));
    let frame_sleep = Duration::from_millis((1_000 / u64::from(args.fps.max(1))).max(1));
    let _terminal_guard = TuiTerminalGuard::activate()?;

    let mut command = std::process::Command::new(&args.ffmpeg_bin);
    if args.loop_playback {
        command.arg("-stream_loop").arg("-1");
    }
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(&args.input)
        .arg("-an")
        .arg("-sn")
        .arg("-dn")
        .arg("-vf")
        .arg(format!(
            "fps={},scale={}:{}:flags=fast_bilinear,format=gray",
            args.fps, args.width, args.height
        ))
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("gray")
        .arg("pipe:1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

    let mut child = command.spawn().with_context(|| {
        format!(
            "spawn ffmpeg (`{}`) for {:?}; install ffmpeg or pass --ffmpeg-bin",
            args.ffmpeg_bin, args.input
        )
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg stdout unavailable"))?;

    let mut frame_no = 0u64;
    let mut buf = vec![0u8; frame_bytes];
    let mut density = args.density.min(4);
    let mut theme_idx = args.theme;
    let mut auto_theme = false;
    let mut luma_boost_level = 1u8;
    let mut glitch = args.glitch;
    let mut datamosh = false;
    let mut noise_overlay = false;
    let mut edge_enhance = true;
    let mut rain_overlay = true;
    let mut paused = false;
    let mut quit_requested = false;
    let mut last_frame: Option<AsciiVideoFrame> = None;
    loop {
        while event::poll(Duration::from_millis(0)).context("poll ascii-play key event")? {
            let evt = event::read().context("read ascii-play key event")?;
            if let Event::Key(key) = evt
                && let Some(input) = map_tui_key_to_control_input(key)
            {
                match input {
                    "/quit" => {
                        quit_requested = true;
                    }
                    "c" => {
                        theme_idx = cycle_dashboard_theme(theme_idx);
                    }
                    "t" => {
                        auto_theme = !auto_theme;
                    }
                    "d" => {
                        density = cycle_density_idx(density);
                    }
                    "b" => {
                        luma_boost_level = cycle_luma_boost_level(luma_boost_level);
                    }
                    "j" => {
                        datamosh = !datamosh;
                    }
                    "n" => {
                        noise_overlay = !noise_overlay;
                    }
                    "+" => {
                        density = (density + 1).min(4);
                    }
                    "-" => {
                        density = density.saturating_sub(1);
                    }
                    "g" => {
                        glitch = !glitch;
                    }
                    "e" => {
                        edge_enhance = !edge_enhance;
                    }
                    "r" => {
                        rain_overlay = !rain_overlay;
                    }
                    "k" => {
                        if let Some(frame) = last_frame.as_ref() {
                            let tick = frame_no / 2;
                            let lines = ascii_lines_from_luma(
                                frame,
                                ascii_density_shades(density),
                                glitch,
                                datamosh,
                                noise_overlay,
                                edge_enhance,
                                rain_overlay,
                                luma_boost_permille(luma_boost_level),
                                tick,
                            );
                            let header = format!(
                                "mode=ascii-play source={} frame={} theme={} auto_theme={} density={}({}) boost={} glitch={} mosh={} noise={} edge={} rain={}",
                                source,
                                frame_no,
                                dashboard_theme_label(resolve_theme_index(
                                    theme_idx, auto_theme, tick
                                )),
                                auto_theme,
                                density,
                                density_label(density),
                                luma_boost_label(luma_boost_level),
                                glitch,
                                datamosh,
                                noise_overlay,
                                edge_enhance,
                                rain_overlay
                            );
                            match write_ascii_snapshot(&format!("play-{source}"), &header, &lines) {
                                Ok(path) => println!("snapshot saved -> {}", path.display()),
                                Err(err) => eprintln!("snapshot failed: {err}"),
                            }
                        }
                    }
                    "p" => {
                        paused = !paused;
                    }
                    _ => {}
                }
            }
        }
        if quit_requested {
            break;
        }

        if paused {
            if let Some(frame) = last_frame.as_ref() {
                let effective_theme = resolve_theme_index(theme_idx, auto_theme, frame_no / 2);
                render_ascii_play_frame(
                    frame,
                    effective_theme,
                    ascii_density_shades(density),
                    glitch,
                    datamosh,
                    noise_overlay,
                    edge_enhance,
                    rain_overlay,
                    auto_theme,
                    luma_boost_level,
                    source,
                    frame_no,
                    args.fps,
                    density,
                    args.loop_playback,
                    true,
                );
            }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        let started = std::time::Instant::now();
        match stdout.read_exact(&mut buf) {
            Ok(()) => {
                frame_no = frame_no.saturating_add(1);
                let frame = AsciiVideoFrame {
                    width: args.width,
                    height: args.height,
                    luma: buf.clone(),
                    updated_at_ms: now_ms(),
                };
                last_frame = Some(frame.clone());
                let effective_theme = resolve_theme_index(theme_idx, auto_theme, frame_no / 2);
                render_ascii_play_frame(
                    &frame,
                    effective_theme,
                    ascii_density_shades(density),
                    glitch,
                    datamosh,
                    noise_overlay,
                    edge_enhance,
                    rain_overlay,
                    auto_theme,
                    luma_boost_level,
                    source,
                    frame_no,
                    args.fps,
                    density,
                    args.loop_playback,
                    false,
                );
                let elapsed = started.elapsed();
                if elapsed < frame_sleep {
                    std::thread::sleep(frame_sleep - elapsed);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(anyhow!("read ffmpeg output failed: {err}")),
        }
    }

    if quit_requested {
        let _ = child.kill();
    }
    let status = child.wait().context("wait ffmpeg process")?;
    if !status.success() && !quit_requested {
        return Err(anyhow!("ffmpeg exited with status {status}"));
    }
    print!("\x1b[0m\n");
    let _ = std::io::stdout().flush();
    Ok(())
}

#[derive(Clone, Debug)]
struct AsciiVideoFrame {
    width: u16,
    height: u16,
    luma: Vec<u8>,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct ConferenceControls {
    mic_enabled: bool,
    camera_enabled: bool,
    share_enabled: bool,
    paused: bool,
    glitch: bool,
    datamosh: bool,
    noise_overlay: bool,
    edge_enhance: bool,
    rain_overlay: bool,
    luma_boost_level: u8,
    advanced: bool,
    help_overlay: bool,
    density_idx: usize,
    theme_idx: usize,
    auto_theme: bool,
    focus_idx: usize,
    adaptive_override: Option<u8>,
    sort_worst_first: bool,
}

#[derive(Clone, Debug, Default)]
struct ConferenceAudioStat {
    rms: u16,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct AnonGroupMediaState {
    epoch: u64,
    key: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Default)]
struct FlowTelemetry {
    window_start_ms: u64,
    window_count: u32,
    rate_per_sec: u32,
    total: u64,
    last_at_ms: u64,
    last_seq: Option<u64>,
    missing_packets: u64,
    sequence_packets: u64,
    jitter_ewma_ms: u64,
    last_arrival_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct ConferenceTelemetry {
    rx_video: FlowTelemetry,
    rx_audio: FlowTelemetry,
    tx_video: FlowTelemetry,
    tx_audio: FlowTelemetry,
    ui_render: FlowTelemetry,
    rtt_last_ms: u64,
    rtt_ewma_ms: u64,
    rtt_samples: u64,
    adaptive_video_interval_ms: u64,
    adaptive_video_quantizer: u8,
    adaptive_audio_interval_ms: u64,
    adaptive_audio_gain_permille: u16,
    adaptive_level: u8,
    adaptive_manual: bool,
    quality_history: Vec<u8>,
    quality_history_last_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct ParticipantMediaTelemetry {
    video: FlowTelemetry,
    audio: FlowTelemetry,
}

#[derive(Clone, Copy, Debug)]
struct DashboardTheme {
    label: &'static str,
    head: (u8, u8, u8),
    hud: (u8, u8, u8),
    hud_dim: (u8, u8, u8),
    feed_header: (u8, u8, u8),
    feed_row_even: (u8, u8, u8),
    feed_row_odd: (u8, u8, u8),
    idle_row_even: (u8, u8, u8),
    idle_row_odd: (u8, u8, u8),
}

const DASHBOARD_THEMES: [DashboardTheme; 4] = [
    DashboardTheme {
        label: "MATRIX",
        head: (0, 170, 110),
        hud: (0, 130, 90),
        hud_dim: (0, 118, 82),
        feed_header: (0, 210, 100),
        feed_row_even: (0, 225, 120),
        feed_row_odd: (0, 255, 120),
        idle_row_even: (0, 140, 70),
        idle_row_odd: (0, 175, 70),
    },
    DashboardTheme {
        label: "NEON-ICE",
        head: (70, 230, 255),
        hud: (45, 170, 230),
        hud_dim: (35, 145, 205),
        feed_header: (125, 235, 255),
        feed_row_even: (175, 250, 255),
        feed_row_odd: (110, 220, 255),
        idle_row_even: (85, 180, 235),
        idle_row_odd: (70, 145, 205),
    },
    DashboardTheme {
        label: "SYNTHWAVE",
        head: (255, 80, 180),
        hud: (230, 120, 255),
        hud_dim: (200, 105, 220),
        feed_header: (255, 140, 220),
        feed_row_even: (255, 190, 240),
        feed_row_odd: (235, 150, 225),
        idle_row_even: (210, 95, 200),
        idle_row_odd: (170, 70, 165),
    },
    DashboardTheme {
        label: "BLADE",
        head: (255, 170, 70),
        hud: (240, 130, 45),
        hud_dim: (205, 112, 40),
        feed_header: (255, 205, 125),
        feed_row_even: (255, 220, 165),
        feed_row_odd: (240, 185, 120),
        idle_row_even: (220, 140, 75),
        idle_row_odd: (190, 115, 62),
    },
];

struct TuiTerminalGuard {
    active: bool,
}

impl TuiTerminalGuard {
    fn activate() -> Result<Self> {
        enable_raw_mode().context("enable raw terminal mode")?;
        print!("\x1b[?25l");
        let _ = std::io::stdout().flush();
        Ok(Self { active: true })
    }
}

impl Drop for TuiTerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            print!("\x1b[?25h\x1b[0m");
            let _ = std::io::stdout().flush();
        }
    }
}

fn map_tui_key_to_control_input(key: KeyEvent) -> Option<&'static str> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return Some("/quit");
    }
    match key.code {
        KeyCode::Char(' ') => Some("p"),
        KeyCode::Char('+') => Some("+"),
        KeyCode::Char('=') => Some("+"),
        KeyCode::Char('-') => Some("-"),
        KeyCode::Char('?') => Some("h"),
        KeyCode::Char(ch) => match ch.to_ascii_lowercase() {
            '0' => Some("0"),
            '1' => Some("1"),
            '2' => Some("2"),
            '3' => Some("3"),
            '4' => Some("4"),
            'c' => Some("c"),
            'm' => Some("m"),
            'v' => Some("v"),
            's' => Some("s"),
            'p' => Some("p"),
            'g' => Some("g"),
            'j' => Some("j"),
            'n' => Some("n"),
            'e' => Some("e"),
            'r' => Some("r"),
            'b' => Some("b"),
            'k' => Some("k"),
            't' => Some("t"),
            'd' => Some("d"),
            'a' => Some("a"),
            'w' => Some("w"),
            'x' => Some("x"),
            'h' => Some("h"),
            'q' => Some("/quit"),
            _ => None,
        },
        KeyCode::Left => Some("left"),
        KeyCode::Right => Some("right"),
        KeyCode::Up => Some("up"),
        KeyCode::Down => Some("down"),
        KeyCode::Tab | KeyCode::BackTab => Some("tab"),
        KeyCode::Esc => Some("/quit"),
        KeyCode::End => Some("/end"),
        KeyCode::F(1) => Some("h"),
        _ => None,
    }
}

fn spawn_tui_input_task(
    tx: mpsc::Sender<String>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::task::spawn_blocking(move || {
        while !stop.load(Ordering::Relaxed) {
            let ready = event::poll(Duration::from_millis(80)).context("poll key event")?;
            if !ready {
                continue;
            }
            let event = event::read().context("read key event")?;
            if let Event::Key(key) = event
                && let Some(input) = map_tui_key_to_control_input(key)
                && tx.blocking_send(input.to_string()).is_err()
            {
                break;
            }
        }
        Ok(())
    })
}

async fn room_chat_conference(args: RoomChatArgs) -> Result<()> {
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

    let local_id = if anonymous_mode {
        args.participant_id
            .clone()
            .unwrap_or_else(random_anon_handle)
    } else {
        args.participant_id.clone().unwrap_or_else(|| {
            let mut bytes = [0u8; 8];
            rand::rng().fill_bytes(&mut bytes);
            format!("p-{}", hex::encode(bytes))
        })
    };
    if anonymous_mode {
        validate_anonymous_participant_handle(&local_id)?;
    }

    let controls = Arc::new(Mutex::new(ConferenceControls {
        density_idx: 2,
        glitch: true,
        datamosh: false,
        noise_overlay: false,
        edge_enhance: true,
        rain_overlay: true,
        luma_boost_level: 1,
        ..ConferenceControls::default()
    }));
    let video_frames = Arc::new(Mutex::new(HashMap::<String, AsciiVideoFrame>::new()));
    let audio_levels = Arc::new(Mutex::new(HashMap::<String, ConferenceAudioStat>::new()));
    let participant_telemetry = Arc::new(Mutex::new(
        HashMap::<String, ParticipantMediaTelemetry>::new(),
    ));
    let anon_group = Arc::new(Mutex::new(AnonGroupMediaState::default()));
    let telemetry = Arc::new(Mutex::new(ConferenceTelemetry::default()));
    let (out_tx, mut out_rx) = mpsc::channel::<KaigiFrame>(512);

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let bytes = encode_framed(&frame)?;
            send.write_all(&bytes).await.context("send frame")?;
        }
        send.finish().context("finish send stream")?;
        Ok::<(), anyhow::Error>(())
    });

    if anonymous_mode {
        let (secret, public) = new_x25519_keypair();
        out_tx
            .send(KaigiFrame::AnonHello(AnonHelloFrame {
                protocol_version: PROTOCOL_VERSION,
                participant_handle: local_id.clone(),
                x25519_pubkey_hex: hex::encode(public.as_bytes()),
            }))
            .await
            .map_err(|_| anyhow!("send channel closed"))?;
        // Keep legacy anonymous control path compatible while media uses group envelopes.
        out_tx
            .send(KaigiFrame::GroupKeyUpdate(GroupKeyUpdateFrame {
                sent_at_ms: now_ms(),
                participant_handle: local_id.clone(),
                x25519_pubkey_hex: hex::encode(public.as_bytes()),
                epoch: 1,
            }))
            .await
            .map_err(|_| anyhow!("send channel closed"))?;
        drop(secret);
    } else {
        let auto_hdr_display = if args.no_hdr_auto {
            false
        } else {
            detect_hdr_display()
        };
        let hdr_display = args.hdr_display || auto_hdr_display;
        let hdr_capture = args.hdr_capture;
        out_tx
            .send(KaigiFrame::Hello(HelloFrame {
                protocol_version: PROTOCOL_VERSION,
                participant_id: local_id.clone(),
                display_name: args.display_name.clone(),
                mic_enabled: false,
                video_enabled: false,
                screen_share_enabled: false,
                hdr_display,
                hdr_capture,
            }))
            .await
            .map_err(|_| anyhow!("send channel closed"))?;
        out_tx
            .send(KaigiFrame::MediaCapability(MediaCapabilityFrame {
                reported_at_ms: now_ms(),
                participant_id: local_id.clone(),
                max_video_width: 1280,
                max_video_height: 720,
                max_video_fps: 30,
                video_codecs: vec![VideoCodecKind::NoritoBaseline],
                audio_codecs: vec![AudioCodecKind::NoritoNative],
                audio_sample_rate: 48_000,
                audio_channels: 2,
            }))
            .await
            .map_err(|_| anyhow!("send channel closed"))?;
    }

    if let Some(ref lc) = auto_kaigi_lifecycle {
        let lc = lc.clone();
        match tokio::task::spawn_blocking(move || run_auto_kaigi_join(&lc)).await {
            Ok(Ok(payload)) => println!("kaigi_join={payload}"),
            Ok(Err(err)) => eprintln!("kaigi lifecycle join failed: {err}"),
            Err(err) => eprintln!("kaigi lifecycle join task failed: {err}"),
        }
    }

    println!(
        "connected participant={local_id} mode={}",
        if anonymous_mode {
            "anonymous"
        } else {
            "transparent"
        }
    );
    println!(
        "controls: m mic, v video, s share, p pause, a cycle adaptive, 0..4 direct adaptive, c theme, t auto-theme, d density, b boost, g glitch, j mosh, n noise, e edge, r rain, w sort, x reset, k snapshot, +/- density, arrows focus, tab advanced, h help, q quit, End end-call"
    );

    let local_id_for_reader = local_id.clone();
    let frames_for_reader = video_frames.clone();
    let audio_for_reader = audio_levels.clone();
    let participant_telemetry_for_reader = participant_telemetry.clone();
    let anon_group_for_reader = anon_group.clone();
    let telemetry_for_reader = telemetry.clone();
    let reader = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        let mut decoder = FrameDecoder::new();
        loop {
            match recv.read(&mut buf).await.context("recv")? {
                Some(n) if n > 0 => {
                    decoder.push(&buf[..n]);
                    while let Some(frame) = decoder.try_next()? {
                        match frame {
                            KaigiFrame::Roster(roster) => {
                                println!("roster count={}", roster.participants.len());
                            }
                            KaigiFrame::Event(event) => {
                                if let RoomEventFrame::Left(left) = event {
                                    frames_for_reader.lock().await.remove(&left.participant_id);
                                    audio_for_reader.lock().await.remove(&left.participant_id);
                                    participant_telemetry_for_reader
                                        .lock()
                                        .await
                                        .remove(&left.participant_id);
                                }
                            }
                            KaigiFrame::MediaTrackState(track) => {
                                println!(
                                    "track participant={} mic={} camera={} share={} active={:?}",
                                    track.participant_id,
                                    track.mic_enabled,
                                    track.camera_enabled,
                                    track.screen_share_enabled,
                                    track.active_video_track
                                );
                            }
                            KaigiFrame::VideoSegment(segment) => {
                                if let Some(decoded) = decode_ascii_video_segment(&segment)? {
                                    let at_ms = now_ms();
                                    frames_for_reader
                                        .lock()
                                        .await
                                        .insert(segment.participant_id.clone(), decoded);
                                    let mut tel = telemetry_for_reader.lock().await;
                                    record_rx_sequence_telemetry(
                                        &mut tel.rx_video,
                                        segment.segment_number,
                                        at_ms,
                                        100,
                                    );
                                    let mut pstats = participant_telemetry_for_reader.lock().await;
                                    let entry =
                                        pstats.entry(segment.participant_id.clone()).or_default();
                                    record_rx_sequence_telemetry(
                                        &mut entry.video,
                                        segment.segment_number,
                                        at_ms,
                                        100,
                                    );
                                }
                            }
                            KaigiFrame::AudioPacket(packet) => {
                                if let Some(level) = decode_audio_level(&packet)? {
                                    let at_ms = now_ms();
                                    audio_for_reader.lock().await.insert(
                                        packet.participant_id.clone(),
                                        ConferenceAudioStat {
                                            rms: level,
                                            updated_at_ms: at_ms,
                                        },
                                    );
                                    let mut tel = telemetry_for_reader.lock().await;
                                    record_rx_sequence_telemetry(
                                        &mut tel.rx_audio,
                                        packet.sequence,
                                        at_ms,
                                        100,
                                    );
                                    let mut pstats = participant_telemetry_for_reader.lock().await;
                                    let entry =
                                        pstats.entry(packet.participant_id.clone()).or_default();
                                    record_rx_sequence_telemetry(
                                        &mut entry.audio,
                                        packet.sequence,
                                        at_ms,
                                        100,
                                    );
                                }
                            }
                            KaigiFrame::AnonGroupKeyRotate(update) => {
                                let key = derive_anon_group_key(&update.key_wrap_hex)?;
                                let mut state = anon_group_for_reader.lock().await;
                                state.epoch = update.epoch;
                                state.key = Some(key);
                                println!(
                                    "anon group key rotated epoch={} members={}",
                                    update.epoch,
                                    update.member_handles.len()
                                );
                            }
                            KaigiFrame::AnonEncryptedPayload(enc) => {
                                let state = anon_group_for_reader.lock().await.clone();
                                let Some(key) = state.key else {
                                    continue;
                                };
                                if state.epoch == 0 || enc.epoch == 0 || enc.epoch < state.epoch {
                                    continue;
                                }
                                let plaintext =
                                    decrypt_anon_group_payload(&key, &enc.sender_handle, &enc)?;
                                match enc.kind {
                                    AnonymousPayloadKind::VideoSegment => {
                                        if let Ok(segment) =
                                            decode_from_bytes::<VideoSegmentFrame>(&plaintext)
                                            && let Some(decoded) =
                                                decode_ascii_video_segment(&segment)?
                                        {
                                            let at_ms = now_ms();
                                            frames_for_reader
                                                .lock()
                                                .await
                                                .insert(segment.participant_id.clone(), decoded);
                                            let mut tel = telemetry_for_reader.lock().await;
                                            record_rx_sequence_telemetry(
                                                &mut tel.rx_video,
                                                segment.segment_number,
                                                at_ms,
                                                100,
                                            );
                                            let mut pstats =
                                                participant_telemetry_for_reader.lock().await;
                                            let entry = pstats
                                                .entry(segment.participant_id.clone())
                                                .or_default();
                                            record_rx_sequence_telemetry(
                                                &mut entry.video,
                                                segment.segment_number,
                                                at_ms,
                                                100,
                                            );
                                        }
                                    }
                                    AnonymousPayloadKind::AudioPacket => {
                                        if let Ok(packet) =
                                            decode_from_bytes::<AudioPacketFrame>(&plaintext)
                                            && let Some(level) = decode_audio_level(&packet)?
                                        {
                                            let at_ms = now_ms();
                                            audio_for_reader.lock().await.insert(
                                                packet.participant_id.clone(),
                                                ConferenceAudioStat {
                                                    rms: level,
                                                    updated_at_ms: at_ms,
                                                },
                                            );
                                            let mut tel = telemetry_for_reader.lock().await;
                                            record_rx_sequence_telemetry(
                                                &mut tel.rx_audio,
                                                packet.sequence,
                                                at_ms,
                                                100,
                                            );
                                            let mut pstats =
                                                participant_telemetry_for_reader.lock().await;
                                            let entry = pstats
                                                .entry(packet.participant_id.clone())
                                                .or_default();
                                            record_rx_sequence_telemetry(
                                                &mut entry.audio,
                                                packet.sequence,
                                                at_ms,
                                                100,
                                            );
                                        }
                                    }
                                    AnonymousPayloadKind::Control => {}
                                }
                            }
                            KaigiFrame::Pong(pong) => {
                                let now = now_ms();
                                if pong.nonce <= now {
                                    let sample = now.saturating_sub(pong.nonce);
                                    if sample <= 60_000 {
                                        let mut telemetry = telemetry_for_reader.lock().await;
                                        record_rtt_sample(&mut telemetry, sample);
                                    }
                                }
                            }
                            KaigiFrame::Error(err) => {
                                println!("error: {}", err.message);
                            }
                            KaigiFrame::Chat(chat) => {
                                println!("[{}] {}", chat.from_participant_id, chat.text);
                            }
                            _ => {}
                        }
                    }
                }
                _ => break,
            }
        }
        println!("reader closed for participant={local_id_for_reader}");
        Ok::<(), anyhow::Error>(())
    });

    let controls_for_media = controls.clone();
    let out_tx_for_media = out_tx.clone();
    let local_id_for_media = local_id.clone();
    let anon_group_for_media = anon_group.clone();
    let telemetry_for_media = telemetry.clone();
    let media_sender = tokio::spawn(async move {
        let frame_dimensions = FrameDimensions::new(96, 54);
        let mut frame_duration_ns: u32 = 100_000_000;
        let mut current_video_interval_ms: u64 = 100;
        let mut current_video_quantizer: u8 = 20;
        let mut video_encoder = BaselineEncoder::new(BaselineEncoderConfig {
            frame_dimensions,
            frame_duration_ns,
            frames_per_segment: 1,
            quantizer: current_video_quantizer,
            ..BaselineEncoderConfig::default()
        });
        let mut audio_encoder = AudioEncoder::new(AudioEncoderConfig {
            sample_rate: 48_000,
            frame_samples: 240,
            layout: AudioLayout::Stereo,
            ..AudioEncoderConfig::default()
        })?;
        let mut video_seq: u64 = 0;
        let mut audio_seq: u64 = 0;
        let mut phase: f32 = 0.0;
        let mut last_video_sent_ms: u64 = 0;
        let mut last_audio_sent_ms: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let current = controls_for_media.lock().await.clone();
            let now = now_ms();
            let (
                video_interval_ms,
                video_quantizer,
                audio_interval_ms,
                audio_gain_permille,
                adaptive_level,
                adaptive_manual,
            ) = {
                let mut telemetry = telemetry_for_media.lock().await;
                let auto_profile = choose_adaptive_video_profile(&telemetry);
                let profile = if let Some(level) = current.adaptive_override {
                    adaptive_profile_for_level(level)
                } else {
                    auto_profile
                };
                let audio_profile = adaptive_audio_profile_for_level(profile.2);
                telemetry.adaptive_video_interval_ms = profile.0;
                telemetry.adaptive_video_quantizer = profile.1;
                telemetry.adaptive_audio_interval_ms = audio_profile.0;
                telemetry.adaptive_audio_gain_permille = audio_profile.1;
                telemetry.adaptive_level = profile.2;
                telemetry.adaptive_manual = current.adaptive_override.is_some();
                (
                    profile.0,
                    profile.1,
                    audio_profile.0,
                    audio_profile.1,
                    profile.2,
                    telemetry.adaptive_manual,
                )
            };
            if video_interval_ms != current_video_interval_ms
                || video_quantizer != current_video_quantizer
            {
                current_video_interval_ms = video_interval_ms;
                current_video_quantizer = video_quantizer;
                frame_duration_ns = current_video_interval_ms
                    .saturating_mul(1_000_000)
                    .min(u64::from(u32::MAX)) as u32;
                video_encoder = BaselineEncoder::new(BaselineEncoderConfig {
                    frame_dimensions,
                    frame_duration_ns,
                    frames_per_segment: 1,
                    quantizer: current_video_quantizer,
                    ..BaselineEncoderConfig::default()
                });
                if adaptive_manual {
                    println!(
                        "adaptive mode {}(manual): interval={}ms q{}",
                        adaptive_mode_label(adaptive_level),
                        current_video_interval_ms,
                        current_video_quantizer
                    );
                } else {
                    println!(
                        "adaptive mode {}(auto): interval={}ms q{}",
                        adaptive_mode_label(adaptive_level),
                        current_video_interval_ms,
                        current_video_quantizer
                    );
                }
            }
            if current.paused {
                continue;
            }

            let should_send_video = (current.camera_enabled || current.share_enabled)
                && (last_video_sent_ms == 0
                    || now.saturating_sub(last_video_sent_ms) >= current_video_interval_ms);
            if should_send_video {
                video_seq = video_seq.saturating_add(1);
                let luma = generate_synthetic_luma(
                    frame_dimensions.width,
                    frame_dimensions.height,
                    video_seq,
                    current.share_enabled,
                );
                let frame = RawFrame::new(frame_dimensions, luma)?;
                let encoded = video_encoder.encode_segment(
                    video_seq,
                    now.saturating_mul(1_000_000),
                    1,
                    &[frame],
                    None,
                )?;
                let bundle = encoded.to_bundle(frame_dimensions, frame_duration_ns);
                let payload = to_bytes(&bundle)?;
                let packet = VideoSegmentFrame {
                    sent_at_ms: now,
                    participant_id: local_id_for_media.clone(),
                    segment_number: video_seq,
                    frame_width: frame_dimensions.width,
                    frame_height: frame_dimensions.height,
                    frame_duration_ns,
                    payload,
                };
                let sent_at_ms = packet.sent_at_ms;
                if anonymous_mode {
                    let state = anon_group_for_media.lock().await.clone();
                    if let (Some(key), true) = (state.key, state.epoch > 0) {
                        let plaintext = to_bytes(&packet)?;
                        let wrapped = encrypt_anon_group_payload(
                            &key,
                            &local_id_for_media,
                            state.epoch,
                            AnonymousPayloadKind::VideoSegment,
                            &plaintext,
                        )?;
                        if out_tx_for_media
                            .send(KaigiFrame::AnonEncryptedPayload(wrapped))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        let mut tel = telemetry_for_media.lock().await;
                        record_flow_telemetry(&mut tel.tx_video, sent_at_ms);
                    }
                } else if out_tx_for_media
                    .send(KaigiFrame::VideoSegment(packet))
                    .await
                    .is_err()
                {
                    break;
                } else {
                    let mut tel = telemetry_for_media.lock().await;
                    record_flow_telemetry(&mut tel.tx_video, sent_at_ms);
                }
                last_video_sent_ms = now;
            }

            let should_send_audio = current.mic_enabled
                && (last_audio_sent_ms == 0
                    || now.saturating_sub(last_audio_sent_ms) >= audio_interval_ms);
            if should_send_audio {
                let mut pcm = Vec::with_capacity(usize::from(240u16) * 2);
                let amplitude = 1_400.0_f32 * (f32::from(audio_gain_permille) / 1_000.0);
                for _ in 0..240 {
                    let sample = (phase.sin() * amplitude) as i16;
                    pcm.push(sample);
                    pcm.push(sample);
                    phase += 0.10;
                }
                let frame =
                    audio_encoder.encode_frame(audio_seq, now.saturating_mul(1_000_000), &pcm)?;
                let packet = AudioPacketFrame {
                    sent_at_ms: now,
                    participant_id: local_id_for_media.clone(),
                    sequence: audio_seq,
                    sample_rate: 48_000,
                    channels: 2,
                    frame_samples: 240,
                    payload: to_bytes(&frame)?,
                };
                let sent_at_ms = packet.sent_at_ms;
                audio_seq = audio_seq.saturating_add(1);
                if anonymous_mode {
                    let state = anon_group_for_media.lock().await.clone();
                    if let (Some(key), true) = (state.key, state.epoch > 0) {
                        let plaintext = to_bytes(&packet)?;
                        let wrapped = encrypt_anon_group_payload(
                            &key,
                            &local_id_for_media,
                            state.epoch,
                            AnonymousPayloadKind::AudioPacket,
                            &plaintext,
                        )?;
                        if out_tx_for_media
                            .send(KaigiFrame::AnonEncryptedPayload(wrapped))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        let mut tel = telemetry_for_media.lock().await;
                        record_flow_telemetry(&mut tel.tx_audio, sent_at_ms);
                    }
                } else if out_tx_for_media
                    .send(KaigiFrame::AudioPacket(packet))
                    .await
                    .is_err()
                {
                    break;
                } else {
                    let mut tel = telemetry_for_media.lock().await;
                    record_flow_telemetry(&mut tel.tx_audio, sent_at_ms);
                }
                last_audio_sent_ms = now;
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let out_tx_for_ping = out_tx.clone();
    let ping_sender = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if out_tx_for_ping
                .send(KaigiFrame::Ping(PingFrame { nonce: now_ms() }))
                .await
                .is_err()
            {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let controls_for_render = controls.clone();
    let frames_for_render = video_frames.clone();
    let audio_for_render = audio_levels.clone();
    let participant_telemetry_for_render = participant_telemetry.clone();
    let telemetry_for_render = telemetry.clone();
    let tui_enabled = args.tui;
    let _tui_terminal_guard = if tui_enabled {
        Some(TuiTerminalGuard::activate()?)
    } else {
        None
    };
    let (mut tui_input_rx, tui_input_stop, tui_input_task): (
        Option<mpsc::Receiver<String>>,
        Option<Arc<AtomicBool>>,
        Option<tokio::task::JoinHandle<Result<()>>>,
    ) = if tui_enabled {
        let (input_tx, input_rx) = mpsc::channel::<String>(128);
        let stop = Arc::new(AtomicBool::new(false));
        let task = spawn_tui_input_task(input_tx, stop.clone());
        (Some(input_rx), Some(stop), Some(task))
    } else {
        (None, None, None)
    };
    let render_task = tokio::spawn(async move {
        if !tui_enabled {
            return Ok::<(), anyhow::Error>(());
        }
        let theme_idx = controls_for_render.lock().await.theme_idx;
        render_tui_boot_sequence(theme_idx).await;
        loop {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let controls = controls_for_render.lock().await.clone();
            let frames = frames_for_render.lock().await.clone();
            let audio = audio_for_render.lock().await.clone();
            let participant_telemetry = participant_telemetry_for_render.lock().await.clone();
            let now = now_ms();
            let telemetry = {
                let mut telemetry = telemetry_for_render.lock().await;
                record_flow_telemetry(&mut telemetry.ui_render, now);
                let quality_label =
                    classify_conference_quality(now, &telemetry, !frames.is_empty());
                let quality_level = quality_level_from_label(quality_label);
                record_quality_history_sample(&mut telemetry, now, quality_level);
                telemetry.clone()
            };
            render_ascii_dashboard(
                &controls,
                &frames,
                &audio,
                &telemetry,
                &participant_telemetry,
            );
        }
    });

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    let mut end_requested = false;
    loop {
        let input = if let Some(rx) = tui_input_rx.as_mut() {
            let Some(value) = rx.recv().await else {
                break;
            };
            value
        } else {
            line.clear();
            let n = stdin.read_line(&mut line).await.context("stdin")?;
            if n == 0 {
                break;
            }
            line.trim().to_ascii_lowercase()
        };
        if input.is_empty() {
            continue;
        }
        if input == "q" || input == "/quit" {
            break;
        }
        let mut controls_mut = controls.lock().await;
        let mut state_changed = false;
        let mut reset_requested = false;
        let mut snapshot_requested = false;
        match input.as_str() {
            "m" | "/mic" => {
                controls_mut.mic_enabled = !controls_mut.mic_enabled;
                state_changed = true;
            }
            "v" | "/video" => {
                controls_mut.camera_enabled = !controls_mut.camera_enabled;
                if controls_mut.camera_enabled {
                    controls_mut.share_enabled = false;
                }
                state_changed = true;
            }
            "s" | "/share" => {
                controls_mut.share_enabled = !controls_mut.share_enabled;
                if controls_mut.share_enabled {
                    controls_mut.camera_enabled = false;
                }
                state_changed = true;
            }
            "space" | "p" => {
                controls_mut.paused = !controls_mut.paused;
            }
            "+" => {
                controls_mut.density_idx = (controls_mut.density_idx + 1).min(4);
            }
            "-" => {
                controls_mut.density_idx = controls_mut.density_idx.saturating_sub(1);
            }
            "g" => {
                controls_mut.glitch = !controls_mut.glitch;
            }
            "j" => {
                controls_mut.datamosh = !controls_mut.datamosh;
                println!(
                    "datamosh -> {}",
                    if controls_mut.datamosh { "ON" } else { "OFF" }
                );
            }
            "n" => {
                controls_mut.noise_overlay = !controls_mut.noise_overlay;
                println!(
                    "noise overlay -> {}",
                    if controls_mut.noise_overlay {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "e" => {
                controls_mut.edge_enhance = !controls_mut.edge_enhance;
                println!(
                    "edge enhance -> {}",
                    if controls_mut.edge_enhance {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r" => {
                controls_mut.rain_overlay = !controls_mut.rain_overlay;
                println!(
                    "rain overlay -> {}",
                    if controls_mut.rain_overlay {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "c" => {
                controls_mut.theme_idx = cycle_dashboard_theme(controls_mut.theme_idx);
                println!("theme -> {}", dashboard_theme_label(controls_mut.theme_idx));
            }
            "t" => {
                controls_mut.auto_theme = !controls_mut.auto_theme;
                println!(
                    "auto theme -> {}",
                    if controls_mut.auto_theme { "ON" } else { "OFF" }
                );
            }
            "d" => {
                controls_mut.density_idx = cycle_density_idx(controls_mut.density_idx);
                println!(
                    "density -> {} ({})",
                    controls_mut.density_idx,
                    density_label(controls_mut.density_idx)
                );
            }
            "b" => {
                controls_mut.luma_boost_level =
                    cycle_luma_boost_level(controls_mut.luma_boost_level);
                println!(
                    "luma boost -> {}",
                    luma_boost_label(controls_mut.luma_boost_level)
                );
            }
            "a" => {
                controls_mut.adaptive_override =
                    cycle_adaptive_override(controls_mut.adaptive_override);
                let label = adaptive_override_label(controls_mut.adaptive_override);
                println!("adaptive override -> {label}");
            }
            "0" | "1" | "2" | "3" | "4" => {
                if let Some(next) = adaptive_override_from_shortcut(input.as_str()) {
                    controls_mut.adaptive_override = next;
                    let label = adaptive_override_label(controls_mut.adaptive_override);
                    println!("adaptive override -> {label}");
                }
            }
            "w" => {
                controls_mut.sort_worst_first = !controls_mut.sort_worst_first;
                println!(
                    "feed sort -> {}",
                    sort_mode_label(controls_mut.sort_worst_first)
                );
            }
            "x" => {
                reset_requested = true;
                println!("telemetry counters reset");
            }
            "k" => {
                snapshot_requested = true;
            }
            "left" | "up" => {
                controls_mut.focus_idx = controls_mut.focus_idx.saturating_sub(1);
            }
            "right" | "down" => {
                controls_mut.focus_idx = controls_mut.focus_idx.saturating_add(1);
            }
            "tab" => {
                controls_mut.advanced = !controls_mut.advanced;
            }
            "h" | "help" | "/help" => {
                controls_mut.help_overlay = !controls_mut.help_overlay;
                println!(
                    "help overlay -> {}",
                    if controls_mut.help_overlay {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
                println!("theme -> {}", dashboard_theme_label(controls_mut.theme_idx));
            }
            "/end" => {
                end_requested = true;
                break;
            }
            _ => {
                println!("unknown control `{input}`");
            }
        }
        let controls_snapshot = controls_mut.clone();
        drop(controls_mut);

        if reset_requested {
            *telemetry.lock().await = ConferenceTelemetry::default();
            participant_telemetry.lock().await.clear();
        }
        if snapshot_requested {
            match capture_conference_snapshot(
                &controls_snapshot,
                &video_frames,
                &participant_telemetry,
            )
            .await
            {
                Ok(Some(path)) => println!("snapshot saved -> {}", path.display()),
                Ok(None) => println!("snapshot skipped (no active video feeds)"),
                Err(err) => eprintln!("snapshot failed: {err}"),
            }
        }

        if state_changed && !anonymous_mode {
            let participant_state = KaigiFrame::ParticipantState(ParticipantStateFrame {
                updated_at_ms: now_ms(),
                mic_enabled: Some(controls_snapshot.mic_enabled),
                video_enabled: Some(controls_snapshot.camera_enabled),
                screen_share_enabled: Some(controls_snapshot.share_enabled),
            });
            let _ = out_tx.send(participant_state).await;
            let track_state = KaigiFrame::MediaTrackState(MediaTrackStateFrame {
                updated_at_ms: now_ms(),
                participant_id: local_id.clone(),
                mic_enabled: controls_snapshot.mic_enabled,
                camera_enabled: controls_snapshot.camera_enabled,
                screen_share_enabled: controls_snapshot.share_enabled,
                active_video_track: if controls_snapshot.share_enabled {
                    MediaTrackKind::ScreenShare
                } else {
                    MediaTrackKind::Camera
                },
            });
            let _ = out_tx.send(track_state).await;
        }
    }

    if let Some(stop) = tui_input_stop.as_ref() {
        stop.store(true, Ordering::Relaxed);
    }
    drop(tui_input_rx);
    if let Some(task) = tui_input_task {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => eprintln!("tui input task error: {err}"),
            Err(err) => eprintln!("tui input task join error: {err}"),
        }
    }

    drop(out_tx);
    media_sender.abort();
    ping_sender.abort();
    render_task.abort();
    let _ = writer.await?;
    let _ = reader.await?;
    finalize_auto_kaigi_lifecycle(auto_kaigi_lifecycle, end_requested, session_started_at_ms).await;
    Ok(())
}

fn generate_synthetic_luma(width: u16, height: u16, tick: u64, share: bool) -> Vec<u8> {
    let w = usize::from(width);
    let h = usize::from(height);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = ((x as u64 * 7 + y as u64 * 13 + tick * 9) % 255) as u8;
            if share {
                v = v.saturating_add((((y as u64 + tick) % 16) * 8) as u8);
            }
            if x % 12 == 0 || y % 8 == 0 {
                v = v.saturating_add(24);
            }
            out[y * w + x] = v;
        }
    }
    out
}

fn decode_ascii_video_segment(segment: &VideoSegmentFrame) -> Result<Option<AsciiVideoFrame>> {
    let bundle: SegmentBundle = decode_from_bytes(&segment.payload)
        .map_err(|err| anyhow!("decode segment bundle: {err}"))?;
    let (encoded, dims, frame_duration_ns) = bundle
        .into_segment()
        .map_err(|err| anyhow!("segment bundle validation failed: {err}"))?;
    let decoder = BaselineDecoder::new(dims, frame_duration_ns);
    let frames = decoder
        .decode_segment(&encoded)
        .map_err(|err| anyhow!("decode baseline segment: {err}"))?;
    let Some(frame) = frames.first() else {
        return Ok(None);
    };
    Ok(Some(AsciiVideoFrame {
        width: dims.width,
        height: dims.height,
        luma: frame.luma.clone(),
        updated_at_ms: now_ms(),
    }))
}

fn decode_audio_level(packet: &AudioPacketFrame) -> Result<Option<u16>> {
    let frame: AudioFrame =
        decode_from_bytes(&packet.payload).map_err(|err| anyhow!("decode audio frame: {err}"))?;
    let layout = match packet.channels {
        1 => AudioLayout::Mono,
        2 => AudioLayout::Stereo,
        4 => AudioLayout::FirstOrderAmbisonics,
        _ => return Ok(None),
    };
    let config = AudioEncoderConfig {
        sample_rate: packet.sample_rate,
        frame_samples: packet.frame_samples,
        layout,
        ..AudioEncoderConfig::default()
    };
    let mut decoder = AudioDecoder::new(config).map_err(|err| anyhow!("audio decoder: {err}"))?;
    let pcm = decoder
        .decode_frame(&frame)
        .map_err(|err| anyhow!("audio decode: {err}"))?;
    if pcm.is_empty() {
        return Ok(None);
    }
    let sum: u64 = pcm
        .iter()
        .map(|value: &i16| u64::from(value.unsigned_abs()))
        .sum();
    Ok(Some(
        (sum / pcm.len() as u64).min(u64::from(u16::MAX)) as u16
    ))
}

fn derive_anon_group_key(key_wrap_hex: &str) -> Result<[u8; 32]> {
    let bytes = decode_hex_vec(key_wrap_hex)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"kaigi-anon-group-media-v1");
    hasher.update(&bytes);
    Ok(*hasher.finalize().as_bytes())
}

fn anon_payload_kind_tag(kind: AnonymousPayloadKind) -> &'static [u8] {
    match kind {
        AnonymousPayloadKind::Control => b"control",
        AnonymousPayloadKind::VideoSegment => b"video",
        AnonymousPayloadKind::AudioPacket => b"audio",
    }
}

fn encrypt_anon_group_payload(
    key: &[u8; 32],
    sender_handle: &str,
    epoch: u64,
    kind: AnonymousPayloadKind,
    plaintext: &[u8],
) -> Result<AnonEncryptedPayloadFrame> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let aad = [
        b"kaigi-anon-group-envelope-v1".as_slice(),
        sender_handle.as_bytes(),
        &epoch.to_le_bytes(),
        anon_payload_kind_tag(kind),
    ]
    .concat();
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("encrypt anonymous group payload"))?;
    Ok(AnonEncryptedPayloadFrame {
        sent_at_ms: now_ms(),
        sender_handle: sender_handle.to_string(),
        epoch,
        kind,
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
    })
}

fn decrypt_anon_group_payload(
    key: &[u8; 32],
    sender_handle: &str,
    frame: &AnonEncryptedPayloadFrame,
) -> Result<Vec<u8>> {
    let nonce = decode_hex_vec(&frame.nonce_hex)?;
    if nonce.len() != 24 {
        return Err(anyhow!("anonymous payload nonce must be 24 bytes"));
    }
    let mut nonce_arr = [0u8; 24];
    nonce_arr.copy_from_slice(&nonce);
    let ciphertext = decode_hex_vec(&frame.ciphertext_hex)?;
    let aad = [
        b"kaigi-anon-group-envelope-v1".as_slice(),
        sender_handle.as_bytes(),
        &frame.epoch.to_le_bytes(),
        anon_payload_kind_tag(frame.kind),
    ]
    .concat();
    let cipher = XChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(
            &XNonce::from(nonce_arr),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("decrypt anonymous group payload"))?;
    Ok(plaintext)
}

fn record_flow_telemetry(flow: &mut FlowTelemetry, at_ms: u64) {
    flow.total = flow.total.saturating_add(1);
    flow.last_at_ms = at_ms;
    if flow.window_start_ms == 0 {
        flow.window_start_ms = at_ms;
    }
    flow.window_count = flow.window_count.saturating_add(1);
    let elapsed_ms = at_ms.saturating_sub(flow.window_start_ms);
    if elapsed_ms >= 1_000 {
        flow.rate_per_sec = ((u64::from(flow.window_count) * 1_000) / elapsed_ms.max(1))
            .min(u64::from(u32::MAX)) as u32;
        flow.window_count = 0;
        flow.window_start_ms = at_ms;
    }
}

fn record_rx_sequence_telemetry(
    flow: &mut FlowTelemetry,
    seq: u64,
    at_ms: u64,
    expected_interval_ms: u64,
) {
    record_flow_telemetry(flow, at_ms);
    flow.sequence_packets = flow.sequence_packets.saturating_add(1);
    if let Some(last_seq) = flow.last_seq
        && seq > last_seq.saturating_add(1)
    {
        flow.missing_packets = flow
            .missing_packets
            .saturating_add(seq.saturating_sub(last_seq).saturating_sub(1));
    }
    if flow.last_arrival_ms != 0 && expected_interval_ms > 0 {
        let gap = at_ms.saturating_sub(flow.last_arrival_ms);
        let jitter = gap.abs_diff(expected_interval_ms);
        flow.jitter_ewma_ms = if flow.jitter_ewma_ms == 0 {
            jitter
        } else {
            (flow.jitter_ewma_ms.saturating_mul(7) + jitter) / 8
        };
    }
    if flow.last_seq.map(|last| seq > last).unwrap_or(true) {
        flow.last_seq = Some(seq);
    }
    flow.last_arrival_ms = at_ms;
}

fn render_loss_percent(flow: &FlowTelemetry) -> String {
    let total = flow.sequence_packets.saturating_add(flow.missing_packets);
    if total == 0 {
        return "--".to_string();
    }
    let tenths = flow
        .missing_packets
        .saturating_mul(1_000)
        .checked_div(total)
        .unwrap_or(0);
    format!("{}.{}%", tenths / 10, tenths % 10)
}

fn render_jitter_ms(flow: &FlowTelemetry) -> String {
    if flow.sequence_packets == 0 {
        "--".to_string()
    } else {
        format!("{}ms", flow.jitter_ewma_ms)
    }
}

fn choose_adaptive_video_profile(telemetry: &ConferenceTelemetry) -> (u64, u8, u8) {
    let total = telemetry
        .rx_video
        .sequence_packets
        .saturating_add(telemetry.rx_video.missing_packets);
    let loss_pct = if total == 0 {
        0
    } else {
        telemetry
            .rx_video
            .missing_packets
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
    };
    let jitter_ms = telemetry.rx_video.jitter_ewma_ms;
    let rtt_ms = telemetry.rtt_ewma_ms;

    if loss_pct >= 25 || jitter_ms >= 300 || rtt_ms >= 600 {
        adaptive_profile_for_level(3)
    } else if loss_pct >= 12 || jitter_ms >= 180 || rtt_ms >= 350 {
        adaptive_profile_for_level(2)
    } else if loss_pct >= 5 || jitter_ms >= 120 || rtt_ms >= 220 {
        adaptive_profile_for_level(1)
    } else {
        adaptive_profile_for_level(0)
    }
}

fn adaptive_mode_label(level: u8) -> &'static str {
    match level {
        0 => "BOOST",
        1 => "WARM",
        2 => "BAL",
        3 => "SAFE",
        _ => "UNK",
    }
}

fn adaptive_override_label(override_level: Option<u8>) -> &'static str {
    match override_level {
        None => "AUTO",
        Some(level) => adaptive_mode_label(level),
    }
}

fn adaptive_profile_for_level(level: u8) -> (u64, u8, u8) {
    match level {
        0 => (100, 20, 0),
        1 => (130, 24, 1),
        2 => (160, 28, 2),
        3 => (220, 34, 3),
        _ => (100, 20, 0),
    }
}

fn adaptive_audio_profile_for_level(level: u8) -> (u64, u16) {
    match level {
        0 => (100, 1_000),
        1 => (110, 920),
        2 => (130, 800),
        3 => (160, 680),
        _ => (100, 1_000),
    }
}

fn cycle_adaptive_override(current: Option<u8>) -> Option<u8> {
    match current {
        None => Some(0),
        Some(0) => Some(1),
        Some(1) => Some(2),
        Some(2) => Some(3),
        _ => None,
    }
}

fn dashboard_theme(theme_idx: usize) -> &'static DashboardTheme {
    &DASHBOARD_THEMES[theme_idx % DASHBOARD_THEMES.len()]
}

fn resolve_theme_index(base_theme_idx: usize, auto_theme: bool, tick: u64) -> usize {
    if auto_theme {
        base_theme_idx.saturating_add((tick / 24) as usize)
    } else {
        base_theme_idx
    }
}

fn luma_boost_permille(level: u8) -> u16 {
    match level {
        0 => 900,
        1 => 1_000,
        2 => 1_200,
        3 => 1_450,
        _ => 1_000,
    }
}

fn luma_boost_label(level: u8) -> &'static str {
    match level {
        0 => "DIM",
        1 => "NORM",
        2 => "HOT",
        3 => "MAX",
        _ => "NORM",
    }
}

fn cycle_luma_boost_level(current: u8) -> u8 {
    match current {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => 0,
    }
}

fn dashboard_theme_label(theme_idx: usize) -> &'static str {
    dashboard_theme(theme_idx).label
}

fn cycle_dashboard_theme(current: usize) -> usize {
    (current + 1) % DASHBOARD_THEMES.len()
}

fn pulse_rgb(base: (u8, u8, u8), pulse: i32, step: u8) -> (u8, u8, u8) {
    let bump = u16::from(step).saturating_mul(pulse.max(0) as u16);
    let r = (u16::from(base.0).saturating_add(bump)).min(255) as u8;
    let g = (u16::from(base.1).saturating_add(bump)).min(255) as u8;
    let b = (u16::from(base.2).saturating_add(bump)).min(255) as u8;
    (r, g, b)
}

fn sort_mode_label(sort_worst_first: bool) -> &'static str {
    if sort_worst_first { "WORST" } else { "ID" }
}

fn adaptive_override_from_shortcut(input: &str) -> Option<Option<u8>> {
    match input {
        "0" => Some(None),
        "1" => Some(Some(0)),
        "2" => Some(Some(1)),
        "3" => Some(Some(2)),
        "4" => Some(Some(3)),
        _ => None,
    }
}

fn record_rtt_sample(telemetry: &mut ConferenceTelemetry, sample_ms: u64) {
    telemetry.rtt_last_ms = sample_ms;
    telemetry.rtt_samples = telemetry.rtt_samples.saturating_add(1);
    if telemetry.rtt_ewma_ms == 0 {
        telemetry.rtt_ewma_ms = sample_ms;
    } else {
        telemetry.rtt_ewma_ms = (telemetry.rtt_ewma_ms.saturating_mul(7) + sample_ms) / 8;
    }
}

fn flow_age_ms(flow: &FlowTelemetry, now_ms: u64) -> u64 {
    if flow.last_at_ms == 0 {
        u64::MAX
    } else {
        now_ms.saturating_sub(flow.last_at_ms)
    }
}

fn classify_conference_quality(
    now_ms: u64,
    telemetry: &ConferenceTelemetry,
    has_video_feed: bool,
) -> &'static str {
    let has_any_media = has_video_feed
        || telemetry.rx_audio.last_at_ms > 0
        || telemetry.tx_audio.last_at_ms > 0
        || telemetry.tx_video.last_at_ms > 0;
    if !has_any_media {
        return "IDLE";
    }

    if has_video_feed {
        let age = flow_age_ms(&telemetry.rx_video, now_ms);
        let fps = telemetry.rx_video.rate_per_sec;
        let high_loss = telemetry.rx_video.sequence_packets >= 8
            && telemetry.rx_video.missing_packets.saturating_mul(100)
                > telemetry
                    .rx_video
                    .sequence_packets
                    .saturating_add(telemetry.rx_video.missing_packets)
                    .saturating_mul(15);
        let high_jitter =
            telemetry.rx_video.sequence_packets >= 8 && telemetry.rx_video.jitter_ewma_ms > 260;
        if high_loss || high_jitter {
            return "NOISY";
        }
        if age <= 500 && fps >= 6 {
            "NEON-GOOD"
        } else if age <= 1_400 && fps >= 2 {
            "WARM"
        } else {
            "DEGRADED"
        }
    } else {
        let age = flow_age_ms(&telemetry.rx_audio, now_ms);
        let pps = telemetry.rx_audio.rate_per_sec;
        let high_loss = telemetry.rx_audio.sequence_packets >= 10
            && telemetry.rx_audio.missing_packets.saturating_mul(100)
                > telemetry
                    .rx_audio
                    .sequence_packets
                    .saturating_add(telemetry.rx_audio.missing_packets)
                    .saturating_mul(20);
        let high_jitter =
            telemetry.rx_audio.sequence_packets >= 10 && telemetry.rx_audio.jitter_ewma_ms > 180;
        if high_loss || high_jitter {
            return "AUDIO-NOISY";
        }
        if age <= 600 && pps >= 6 {
            "AUDIO-GOOD"
        } else if age <= 1_600 && pps >= 2 {
            "AUDIO-WARM"
        } else {
            "AUDIO-DEGRADED"
        }
    }
}

fn render_flow_age(age_ms: u64) -> String {
    if age_ms == u64::MAX {
        "--".to_string()
    } else {
        format!("{age_ms}ms")
    }
}

fn render_rtt(telemetry: &ConferenceTelemetry) -> String {
    if telemetry.rtt_samples == 0 {
        "--".to_string()
    } else {
        format!("{}ms/{}ms", telemetry.rtt_last_ms, telemetry.rtt_ewma_ms)
    }
}

fn quality_level_from_label(label: &str) -> u8 {
    match label {
        "NEON-GOOD" | "AUDIO-GOOD" => 4,
        "WARM" | "AUDIO-WARM" => 3,
        "NOISY" | "AUDIO-NOISY" => 2,
        "DEGRADED" | "AUDIO-DEGRADED" => 1,
        _ => 0,
    }
}

fn record_quality_history_sample(telemetry: &mut ConferenceTelemetry, at_ms: u64, level: u8) {
    if telemetry.quality_history_last_ms != 0
        && at_ms.saturating_sub(telemetry.quality_history_last_ms) < 800
    {
        return;
    }
    telemetry.quality_history_last_ms = at_ms;
    telemetry.quality_history.push(level.min(4));
    if telemetry.quality_history.len() > 64 {
        let overflow = telemetry.quality_history.len() - 64;
        telemetry.quality_history.drain(0..overflow);
    }
}

fn render_quality_history(history: &[u8], width: usize) -> String {
    if history.is_empty() || width == 0 {
        return "--".to_string();
    }
    let palette = [' ', '.', ':', '*', '#'];
    let start = history.len().saturating_sub(width);
    let mut out = String::with_capacity(width);
    let tail = &history[start..];
    for _ in tail.len()..width {
        out.push(' ');
    }
    for level in tail {
        out.push(palette[usize::from((*level).min(4))]);
    }
    out
}

fn participant_quality_score(
    now_ms: u64,
    stats: &ParticipantMediaTelemetry,
    frame_age_ms: u64,
) -> u8 {
    let mut score = 5i32;
    if frame_age_ms > 1_200 {
        score -= 2;
    }
    if frame_age_ms > 2_400 {
        score -= 2;
    }

    let video = &stats.video;
    let total = video.sequence_packets.saturating_add(video.missing_packets);
    if total >= 8 {
        let loss_pct = video
            .missing_packets
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0);
        if loss_pct > 20 {
            score -= 3;
        } else if loss_pct > 10 {
            score -= 2;
        } else if loss_pct > 4 {
            score -= 1;
        }
    }
    if video.sequence_packets >= 8 {
        if video.jitter_ewma_ms > 260 {
            score -= 2;
        } else if video.jitter_ewma_ms > 140 {
            score -= 1;
        }
        if video.rate_per_sec < 2 {
            score -= 1;
        }
    }
    if flow_age_ms(video, now_ms) > 2_000 {
        score -= 2;
    }
    score.clamp(0, 5) as u8
}

fn render_quality_bar(score: u8) -> String {
    let mut out = String::from("[");
    for idx in 0..5 {
        out.push(if idx < usize::from(score.min(5)) {
            '#'
        } else {
            '-'
        });
    }
    out.push(']');
    out
}

fn quality_label(score: u8) -> &'static str {
    match score {
        5 | 4 => "NEON",
        3 => "WARM",
        2 => "NOISY",
        1 | 0 => "BAD",
        _ => "UNK",
    }
}

fn sorted_feed_ids(
    frames: &HashMap<String, AsciiVideoFrame>,
    participant_telemetry: &HashMap<String, ParticipantMediaTelemetry>,
    controls: &ConferenceControls,
    now_ms: u64,
) -> Vec<String> {
    let mut ids = frames.keys().cloned().collect::<Vec<_>>();
    ids.sort();
    if controls.sort_worst_first {
        ids.sort_by(|a, b| {
            let Some(frame_a) = frames.get(a) else {
                return a.cmp(b);
            };
            let Some(frame_b) = frames.get(b) else {
                return a.cmp(b);
            };
            let age_a = now_ms.saturating_sub(frame_a.updated_at_ms);
            let age_b = now_ms.saturating_sub(frame_b.updated_at_ms);
            let stats_a = participant_telemetry.get(a).cloned().unwrap_or_default();
            let stats_b = participant_telemetry.get(b).cloned().unwrap_or_default();
            let score_a = participant_quality_score(now_ms, &stats_a, age_a);
            let score_b = participant_quality_score(now_ms, &stats_b, age_b);
            score_a
                .cmp(&score_b)
                .then_with(|| age_b.cmp(&age_a))
                .then_with(|| a.cmp(b))
        });
    }
    ids
}

fn top_worst_peers(
    frames: &HashMap<String, AsciiVideoFrame>,
    participant_telemetry: &HashMap<String, ParticipantMediaTelemetry>,
    now_ms: u64,
    limit: usize,
) -> String {
    if limit == 0 || frames.is_empty() {
        return "--".to_string();
    }
    let mut rows = Vec::with_capacity(frames.len());
    for (participant_id, frame) in frames {
        let age = now_ms.saturating_sub(frame.updated_at_ms);
        let stats = participant_telemetry
            .get(participant_id)
            .cloned()
            .unwrap_or_default();
        let score = participant_quality_score(now_ms, &stats, age);
        rows.push((score, age, participant_id.clone()));
    }
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let mut out = Vec::new();
    for (score, _, participant_id) in rows.into_iter().take(limit) {
        out.push(format!("{participant_id}:{}", quality_label(score)));
    }
    if out.is_empty() {
        "--".to_string()
    } else {
        out.join(" ")
    }
}

fn sanitize_snapshot_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '/' || ch == ':' {
            out.push('-');
        }
    }
    if out.is_empty() {
        "snapshot".to_string()
    } else {
        out
    }
}

fn write_ascii_snapshot(label: &str, header: &str, lines: &[String]) -> Result<PathBuf> {
    let dir = PathBuf::from("ascii-snapshots");
    std::fs::create_dir_all(&dir).context("create ascii-snapshots directory")?;
    let file = format!("{}-{}.txt", now_ms(), sanitize_snapshot_label(label));
    let path = dir.join(file);
    let mut payload = String::new();
    payload.push_str(header);
    payload.push('\n');
    for line in lines {
        payload.push_str(line);
        payload.push('\n');
    }
    std::fs::write(&path, payload)
        .with_context(|| format!("write snapshot to {}", path.display()))?;
    Ok(path)
}

async fn capture_conference_snapshot(
    controls: &ConferenceControls,
    frames: &Arc<Mutex<HashMap<String, AsciiVideoFrame>>>,
    participant_telemetry: &Arc<Mutex<HashMap<String, ParticipantMediaTelemetry>>>,
) -> Result<Option<PathBuf>> {
    let frames_guard = frames.lock().await.clone();
    if frames_guard.is_empty() {
        return Ok(None);
    }
    let pstats_guard = participant_telemetry.lock().await.clone();
    let now = now_ms();
    let mut ids = sorted_feed_ids(&frames_guard, &pstats_guard, controls, now);
    if ids.is_empty() {
        return Ok(None);
    }
    let focus = controls.focus_idx % ids.len();
    ids.rotate_left(focus);
    let Some(participant_id) = ids.first().cloned() else {
        return Ok(None);
    };
    let Some(frame) = frames_guard.get(&participant_id) else {
        return Ok(None);
    };
    let tick = now / 120;
    let lines = ascii_lines_from_luma(
        frame,
        ascii_density_shades(controls.density_idx),
        controls.glitch,
        controls.datamosh,
        controls.noise_overlay,
        controls.edge_enhance,
        controls.rain_overlay,
        luma_boost_permille(controls.luma_boost_level),
        tick,
    );
    let theme_idx = resolve_theme_index(controls.theme_idx, controls.auto_theme, tick);
    let header = format!(
        "mode=ascii-live participant={} theme={} auto_theme={} density={}({}) boost={} glitch={} mosh={} noise={} edge={} rain={}",
        participant_id,
        dashboard_theme_label(theme_idx),
        controls.auto_theme,
        controls.density_idx,
        density_label(controls.density_idx),
        luma_boost_label(controls.luma_boost_level),
        controls.glitch,
        controls.datamosh,
        controls.noise_overlay,
        controls.edge_enhance,
        controls.rain_overlay
    );
    write_ascii_snapshot(&format!("live-{participant_id}"), &header, &lines).map(Some)
}

fn ascii_density_shades(density_idx: usize) -> &'static str {
    match density_idx {
        0 => " .:-=+*#%@",
        1 => " .,:;irsXA253hMHGS#9B&@",
        2 => " .'`^\",:;Il!i~+_-?][}{1)(|\\/*tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$",
        3 => " .-:=+*#%@",
        4 => " .`-~:+=*#%@",
        _ => " .:-=+*#%@",
    }
}

fn density_label(density_idx: usize) -> &'static str {
    match density_idx {
        0 => "LITE",
        1 => "GRID",
        2 => "ULTRA",
        3 => "BOLD",
        4 => "CHROME",
        _ => "LITE",
    }
}

fn cycle_density_idx(current: usize) -> usize {
    if current >= 4 { 0 } else { current + 1 }
}

fn render_boot_bar(width: usize, fill: usize) -> String {
    let mut out = String::with_capacity(width + 2);
    out.push('[');
    for idx in 0..width {
        out.push(if idx < fill.min(width) { '#' } else { '.' });
    }
    out.push(']');
    out
}

async fn render_tui_boot_sequence(theme_idx: usize) {
    let theme = dashboard_theme(theme_idx);
    let total_steps = 12u64;
    for step in 0..=total_steps {
        let pulse = ((step % 8) as i32).abs_diff(4) as i32;
        let head = pulse_rgb(theme.head, 4 - pulse, 8);
        let hud = pulse_rgb(theme.hud, 4 - pulse, 5);
        let progress = ((step * 100) / total_steps) as usize;
        let bar_fill = ((step * 40) / total_steps) as usize;
        let bar = render_boot_bar(40, bar_fill);
        print!("\x1b[2J\x1b[H");
        println!(
            "\x1b[38;2;{};{};{}mKAIGI ASCII LIVE :: NEON LINK INIT [{}]\x1b[0m",
            head.0, head.1, head.2, theme.label
        );
        println!(
            "\x1b[38;2;{};{};{}mbooting stream core ... {:>3}%\x1b[0m",
            hud.0, hud.1, hud.2, progress
        );
        println!("\x1b[38;2;{};{};{}m{}\x1b[0m", hud.0, hud.1, hud.2, bar);
        println!(
            "\x1b[38;2;{};{};{}mentropy={}kHz  jitter-scan={}ms  route=stable\x1b[0m",
            hud.0,
            hud.1,
            hud.2,
            320 + step * 9,
            16_u64.saturating_sub(step / 2)
        );
        let _ = std::io::stdout().flush();
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn render_ascii_play_frame(
    frame: &AsciiVideoFrame,
    theme_idx: usize,
    shades: &str,
    glitch: bool,
    datamosh: bool,
    noise_overlay: bool,
    edge_enhance: bool,
    rain_overlay: bool,
    auto_theme: bool,
    luma_boost_level: u8,
    source: &str,
    frame_no: u64,
    fps: u16,
    density: usize,
    looping: bool,
    paused: bool,
) {
    let theme = dashboard_theme(theme_idx);
    let tick = frame_no / 2;
    let pulse = (tick % 10) as i32;
    let pulse = if pulse > 5 { 10 - pulse } else { pulse };
    let head = pulse_rgb(theme.head, pulse, 8);
    let hud = pulse_rgb(theme.hud, pulse, 5);
    let hud_dim = pulse_rgb(theme.hud_dim, pulse, 4);
    print!("\x1b[2J\x1b[H");
    println!(
        "\x1b[38;2;{};{};{}mKAIGI ASCII PLAY // {} // frame={} fps={}\x1b[0m",
        head.0, head.1, head.2, theme.label, frame_no, fps
    );
    println!(
        "\x1b[38;2;{};{};{}m+--------------------------------------------------------------------------------+\x1b[0m",
        hud.0, hud.1, hud.2
    );
    println!(
        "\x1b[38;2;{};{};{}m| src={} dim={}x{} density={}({}) boost={} glitch={} mosh={} noise={} edge={} rain={} auto-theme={} loop={} pause={} |\x1b[0m",
        hud_dim.0,
        hud_dim.1,
        hud_dim.2,
        source,
        frame.width,
        frame.height,
        density,
        density_label(density),
        luma_boost_label(luma_boost_level),
        glitch,
        datamosh,
        noise_overlay,
        edge_enhance,
        rain_overlay,
        auto_theme,
        looping,
        paused
    );
    println!(
        "\x1b[38;2;{};{};{}m| keys: c theme  t auto-theme  d density  b boost  +/- density  g glitch  j mosh  n noise  e edge  r rain  k snap  p pause  q quit |\x1b[0m",
        hud_dim.0, hud_dim.1, hud_dim.2
    );
    println!(
        "\x1b[38;2;{};{};{}m+--------------------------------------------------------------------------------+\x1b[0m",
        hud.0, hud.1, hud.2
    );
    for (row_idx, line) in ascii_lines_from_luma(
        frame,
        shades,
        glitch,
        datamosh,
        noise_overlay,
        edge_enhance,
        rain_overlay,
        luma_boost_permille(luma_boost_level),
        tick,
    )
    .into_iter()
    .enumerate()
    {
        let color = if row_idx % 2 == 0 {
            theme.feed_row_even
        } else {
            theme.feed_row_odd
        };
        println!(
            "\x1b[38;2;{};{};{}m{}\x1b[0m",
            color.0, color.1, color.2, line
        );
    }
    println!(
        "\x1b[38;2;{};{};{}m+--------------------------------------------------------------------------------+\x1b[0m",
        hud.0, hud.1, hud.2
    );
    let _ = std::io::stdout().flush();
}

fn render_ascii_dashboard(
    controls: &ConferenceControls,
    frames: &HashMap<String, AsciiVideoFrame>,
    audio: &HashMap<String, ConferenceAudioStat>,
    telemetry: &ConferenceTelemetry,
    participant_telemetry: &HashMap<String, ParticipantMediaTelemetry>,
) {
    let now = now_ms();
    let tick = now / 120;
    let mut ids = sorted_feed_ids(frames, participant_telemetry, controls, now);
    if !ids.is_empty() {
        let focus = controls.focus_idx % ids.len();
        ids.rotate_left(focus);
    }
    let ids = ids.into_iter().take(4).collect::<Vec<_>>();
    let shades = ascii_density_shades(controls.density_idx);
    print!("\x1b[2J\x1b[H");
    let pulse = (tick % 12) as i32;
    let pulse = if pulse > 6 { 12 - pulse } else { pulse };
    let theme_idx = resolve_theme_index(controls.theme_idx, controls.auto_theme, tick);
    let theme = dashboard_theme(theme_idx);
    let head_rgb = pulse_rgb(theme.head, pulse, 9);
    let hud_rgb = pulse_rgb(theme.hud, pulse, 6);
    let hud_dim_rgb = pulse_rgb(theme.hud_dim, pulse, 4);
    println!(
        "\x1b[38;2;{};{};{}mKAIGI ASCII LIVE // CYBERPUNK {} // t={}\x1b[0m",
        head_rgb.0, head_rgb.1, head_rgb.2, theme.label, tick
    );
    println!(
        "\x1b[38;2;{};{};{}m+--------------------------------------------------------------------------------+\x1b[0m",
        hud_rgb.0, hud_rgb.1, hud_rgb.2
    );
    println!(
        "\x1b[38;2;{};{};{}m| MIC={} VIDEO={} SHARE={} PAUSE={} GLITCH={} MOSH={} NOISE={} DENSITY={} BOOST={} EDGE={} RAIN={} ADV={} THEME={} AUTO-THEME={} |\x1b[0m",
        hud_rgb.0,
        hud_rgb.1,
        hud_rgb.2,
        controls.mic_enabled,
        controls.camera_enabled,
        controls.share_enabled,
        controls.paused,
        controls.glitch,
        controls.datamosh,
        controls.noise_overlay,
        density_label(controls.density_idx),
        luma_boost_label(controls.luma_boost_level),
        controls.edge_enhance,
        controls.rain_overlay,
        controls.advanced,
        theme.label,
        controls.auto_theme
    );
    println!(
        "\x1b[38;2;{};{};{}m| feeds={} sort={} | controls: m v s p a 0..4 c t d b g j n e r w x k +/- arrows tab h q End |\x1b[0m",
        hud_dim_rgb.0,
        hud_dim_rgb.1,
        hud_dim_rgb.2,
        ids.len(),
        sort_mode_label(controls.sort_worst_first)
    );
    let quality = classify_conference_quality(now, telemetry, !ids.is_empty());
    let rx_age = render_flow_age(flow_age_ms(&telemetry.rx_video, now));
    let tx_age = render_flow_age(flow_age_ms(&telemetry.tx_video, now));
    let rtt = render_rtt(telemetry);
    let adapt_mode = adaptive_mode_label(telemetry.adaptive_level);
    let adapt_source = if telemetry.adaptive_manual {
        "MANUAL"
    } else {
        "AUTO"
    };
    let adapt_interval = if telemetry.adaptive_video_interval_ms == 0 {
        100
    } else {
        telemetry.adaptive_video_interval_ms
    };
    let adapt_quantizer = if telemetry.adaptive_video_quantizer == 0 {
        20
    } else {
        telemetry.adaptive_video_quantizer
    };
    let adapt_audio_interval = if telemetry.adaptive_audio_interval_ms == 0 {
        100
    } else {
        telemetry.adaptive_audio_interval_ms
    };
    let adapt_audio_gain = if telemetry.adaptive_audio_gain_permille == 0 {
        1_000
    } else {
        telemetry.adaptive_audio_gain_permille
    };
    let rx_video_loss = render_loss_percent(&telemetry.rx_video);
    let rx_audio_loss = render_loss_percent(&telemetry.rx_audio);
    let rx_video_jitter = render_jitter_ms(&telemetry.rx_video);
    let rx_audio_jitter = render_jitter_ms(&telemetry.rx_audio);
    println!(
        "\x1b[38;2;{};{};{}m| quality={} | rtt={} | adapt={}({}) v{}ms q{} a{}ms g{}% | rx[v:{}fps a:{}pps age:{}] tx[v:{}fps a:{}pps age:{}] |\x1b[0m",
        hud_dim_rgb.0,
        hud_dim_rgb.1,
        hud_dim_rgb.2,
        quality,
        rtt,
        adapt_mode,
        adapt_source,
        adapt_interval,
        adapt_quantizer,
        adapt_audio_interval,
        adapt_audio_gain / 10,
        telemetry.rx_video.rate_per_sec,
        telemetry.rx_audio.rate_per_sec,
        rx_age,
        telemetry.tx_video.rate_per_sec,
        telemetry.tx_audio.rate_per_sec,
        tx_age
    );
    println!(
        "\x1b[38;2;{};{};{}m| net loss[v:{} a:{}] jitter[v:{} a:{}] ui:{}fps                                  |\x1b[0m",
        hud_dim_rgb.0,
        hud_dim_rgb.1,
        hud_dim_rgb.2,
        rx_video_loss,
        rx_audio_loss,
        rx_video_jitter,
        rx_audio_jitter,
        telemetry.ui_render.rate_per_sec
    );
    if controls.help_overlay {
        println!(
            "\x1b[38;2;{};{};{}m| help m/v/s/p media  a cycle-adapt  0..4 set adapt  c theme  t auto-theme  d density |\x1b[0m",
            hud_dim_rgb.0, hud_dim_rgb.1, hud_dim_rgb.2
        );
        println!(
            "\x1b[38;2;{};{};{}m| help b boost  g glitch  j mosh  n noise  e edge  r rain  w sort  x reset  k snapshot |\x1b[0m",
            hud_dim_rgb.0, hud_dim_rgb.1, hud_dim_rgb.2
        );
        println!(
            "\x1b[38;2;{};{};{}m| help arrows focus  +/- density  tab advanced  h help  q quit  End end         |\x1b[0m",
            hud_dim_rgb.0, hud_dim_rgb.1, hud_dim_rgb.2
        );
    }
    if controls.advanced {
        let override_label = adaptive_override_label(controls.adaptive_override);
        let trend = render_quality_history(&telemetry.quality_history, 48);
        let worst = top_worst_peers(frames, participant_telemetry, now, 3);
        println!(
            "\x1b[38;2;{};{};{}m| adv override={} sort={} boost={} mosh={} noise={} edge={} rain={} theme={} auto-theme={} cycle:AUTO->BOOST->WARM->BAL->SAFE->AUTO |\x1b[0m",
            hud_dim_rgb.0,
            hud_dim_rgb.1,
            hud_dim_rgb.2,
            override_label,
            sort_mode_label(controls.sort_worst_first),
            luma_boost_label(controls.luma_boost_level),
            controls.datamosh,
            controls.noise_overlay,
            controls.edge_enhance,
            controls.rain_overlay,
            theme.label,
            controls.auto_theme
        );
        println!(
            "\x1b[38;2;{};{};{}m| adv totals rx[v:{} a:{}] tx[v:{} a:{}] ping_samples:{} (x reset)                |\x1b[0m",
            hud_dim_rgb.0,
            hud_dim_rgb.1,
            hud_dim_rgb.2,
            telemetry.rx_video.total,
            telemetry.rx_audio.total,
            telemetry.tx_video.total,
            telemetry.tx_audio.total,
            telemetry.rtt_samples
        );
        println!(
            "\x1b[38;2;{};{};{}m| adv trend [{}]                                             |\x1b[0m",
            hud_dim_rgb.0, hud_dim_rgb.1, hud_dim_rgb.2, trend
        );
        println!(
            "\x1b[38;2;{};{};{}m| adv worst {}                                                  |\x1b[0m",
            hud_dim_rgb.0, hud_dim_rgb.1, hud_dim_rgb.2, worst
        );
    }
    println!(
        "\x1b[38;2;{};{};{}m+--------------------------------------------------------------------------------+\x1b[0m",
        hud_rgb.0, hud_rgb.1, hud_rgb.2
    );

    if ids.is_empty() {
        for (row_idx, line) in matrix_idle_lines(80, 24, tick).into_iter().enumerate() {
            let color = if row_idx % 2 == 0 {
                theme.idle_row_even
            } else {
                theme.idle_row_odd
            };
            println!(
                "\x1b[38;2;{};{};{}m{}\x1b[0m",
                color.0, color.1, color.2, line
            );
        }
        let _ = std::io::stdout().flush();
        return;
    }

    for participant_id in ids {
        if let Some(frame) = frames.get(&participant_id) {
            let age = now.saturating_sub(frame.updated_at_ms);
            let (rms, audio_age) = audio
                .get(&participant_id)
                .map(|v| (v.rms, now.saturating_sub(v.updated_at_ms)))
                .unwrap_or((0, 0));
            let pstats = participant_telemetry
                .get(&participant_id)
                .cloned()
                .unwrap_or_default();
            let p_quality_score = participant_quality_score(now, &pstats, age);
            let p_quality_bar = render_quality_bar(p_quality_score);
            let p_quality_label = quality_label(p_quality_score);
            let p_loss = render_loss_percent(&pstats.video);
            let p_jitter = render_jitter_ms(&pstats.video);
            let meter = render_audio_meter(rms, 26);
            println!(
                "\x1b[38;2;{};{};{}m+ {} | {} {} {} | v:{}fps loss:{} jit:{} | age={}ms | rms={} {} ({}ms) +\x1b[0m",
                theme.feed_header.0,
                theme.feed_header.1,
                theme.feed_header.2,
                participant_id,
                p_quality_label,
                p_quality_bar,
                format!("{}x{}", frame.width, frame.height),
                pstats.video.rate_per_sec,
                p_loss,
                p_jitter,
                age,
                rms,
                meter,
                audio_age
            );
            for (row_idx, line) in ascii_lines_from_luma(
                frame,
                shades,
                controls.glitch,
                controls.datamosh,
                controls.noise_overlay,
                controls.edge_enhance,
                controls.rain_overlay,
                luma_boost_permille(controls.luma_boost_level),
                tick,
            )
            .into_iter()
            .enumerate()
            {
                let color = if row_idx % 2 == 0 {
                    theme.feed_row_even
                } else {
                    theme.feed_row_odd
                };
                println!(
                    "\x1b[38;2;{};{};{}m{}\x1b[0m",
                    color.0, color.1, color.2, line
                );
            }
        }
    }
    let _ = std::io::stdout().flush();
}

fn render_audio_meter(rms: u16, width: usize) -> String {
    let max_rms = 2_400usize;
    let fill = usize::from(rms).min(max_rms).saturating_mul(width) / max_rms;
    let mut out = String::with_capacity(width + 2);
    out.push('[');
    for idx in 0..width {
        out.push(if idx < fill { '#' } else { '.' });
    }
    out.push(']');
    out
}

fn matrix_idle_lines(width: usize, height: usize, tick: u64) -> Vec<String> {
    let glyphs = b" .`^\",:;Il!i~+_-?][}{1)(|\\/*tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";
    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut line = String::with_capacity(width);
        for x in 0..width {
            let value = (x as u64)
                .saturating_mul(31)
                .saturating_add((y as u64).saturating_mul(17))
                .saturating_add(tick.saturating_mul(13))
                .saturating_add(((x as u64) ^ (y as u64)).saturating_mul(9))
                % 100;
            let ch = if value > 96 {
                glyphs[(value as usize) % glyphs.len()] as char
            } else if value > 90 && (x + y + tick as usize).is_multiple_of(2) {
                '|'
            } else if value > 80 {
                '.'
            } else {
                ' '
            };
            line.push(ch);
        }
        lines.push(line);
    }
    lines
}

fn ascii_lines_from_luma(
    frame: &AsciiVideoFrame,
    shades: &str,
    glitch: bool,
    datamosh: bool,
    noise_overlay: bool,
    edge_enhance: bool,
    rain_overlay: bool,
    luma_boost_permille: u16,
    tick: u64,
) -> Vec<String> {
    let out_w = 64usize;
    let out_h = 18usize;
    let src_w = usize::from(frame.width);
    let src_h = usize::from(frame.height);
    let shade_chars = shades.chars().collect::<Vec<_>>();
    let shade_len = shade_chars.len().max(2);
    let mut lines = Vec::with_capacity(out_h);
    for y in 0..out_h {
        let sy = y.saturating_mul(src_h).checked_div(out_h).unwrap_or(0);
        let mut line = String::with_capacity(out_w);
        let mosh_shift = if datamosh && src_w > 0 {
            let base = (((tick as usize)
                .saturating_add(y.saturating_mul(3))
                .saturating_mul(5))
                % 9) as isize
                - 4;
            if (y + tick as usize).is_multiple_of(5) {
                base.saturating_mul(2)
            } else {
                base
            }
        } else {
            0
        };
        for x in 0..out_w {
            let sx_base = x.saturating_mul(src_w).checked_div(out_w).unwrap_or(0);
            let sx = if src_w > 0 {
                (sx_base as isize + mosh_shift).rem_euclid(src_w as isize) as usize
            } else {
                sx_base
            };
            let idx = sy.saturating_mul(src_w).saturating_add(sx);
            let px = frame.luma.get(idx).copied().unwrap_or(0);
            let mapped = if edge_enhance {
                let right_x = (sx + 1).min(src_w.saturating_sub(1));
                let down_y = (sy + 1).min(src_h.saturating_sub(1));
                let right_idx = sy.saturating_mul(src_w).saturating_add(right_x);
                let down_idx = down_y.saturating_mul(src_w).saturating_add(sx);
                let right = frame.luma.get(right_idx).copied().unwrap_or(px);
                let down = frame.luma.get(down_idx).copied().unwrap_or(px);
                let edge = u16::from(px.abs_diff(right))
                    .saturating_add(u16::from(px.abs_diff(down)))
                    .min(255) as u8;
                ((u16::from(px).saturating_mul(3) + u16::from(edge).saturating_mul(5)) / 8) as u8
            } else {
                px
            };
            let boosted = ((u32::from(mapped).saturating_mul(u32::from(luma_boost_permille)))
                / 1_000)
                .min(255) as u8;
            let mut shade_idx = usize::from(boosted)
                .saturating_mul(shade_len - 1)
                .checked_div(255)
                .unwrap_or(0);
            if glitch && (x + y + tick as usize) % 29 == 0 {
                shade_idx = (shade_idx + 2).min(shade_len - 1);
            }
            if glitch && (y + tick as usize).is_multiple_of(6) {
                shade_idx = shade_idx.saturating_sub(1);
            }
            let mut ch = shade_chars[shade_idx];
            if rain_overlay {
                let rain_seed = (x as u64)
                    .saturating_mul(37)
                    .saturating_add((y as u64).saturating_mul(19))
                    .saturating_add(tick.saturating_mul(17))
                    % 97;
                if rain_seed >= 92 {
                    ch = '|';
                } else if rain_seed >= 88 {
                    ch = '.';
                }
            }
            if noise_overlay {
                let noise_seed = (x as u64)
                    .saturating_mul(97)
                    .saturating_add((y as u64).saturating_mul(67))
                    .saturating_add(tick.saturating_mul(41))
                    % 101;
                if noise_seed >= 98 {
                    ch = '@';
                } else if noise_seed >= 95 {
                    ch = '#';
                } else if noise_seed >= 91 {
                    ch = '.';
                }
            }
            line.push(ch);
        }
        lines.push(line);
    }
    lines
}

async fn room_chat(args: RoomChatArgs) -> Result<()> {
    if !args.legacy_ui {
        return room_chat_conference(args).await;
    }
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
                                    participant_id,
                                    host,
                                    co_host,
                                    can_moderate,
                                    can_record_local,
                                    epoch
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
                                    grant.target_participant_id,
                                    grant.role,
                                    grant.granted_by,
                                    grant.issued_at_ms
                                );
                            }
                            KaigiFrame::RoleRevoke(revoke) => {
                                println!(
                                    "role_revoke target={} role={:?} revoked_by={} issued_at_ms={}",
                                    revoke.target_participant_id,
                                    revoke.role,
                                    revoke.revoked_by,
                                    revoke.issued_at_ms
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
                        signature_hex: session_policy_signature_hex(&participant_id, &state, at_ms),
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
            assert!(
                err.to_string()
                    .contains("join link v1 must not include exp/nonce/sig fields")
            );
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
            assert!(
                err.to_string()
                    .contains("join link signature verification failed")
            );
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
            assert!(
                err.to_string()
                    .contains("join link exp exceeds max future window")
            );
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
            parse_target_platform("vision-os").expect("parse vision-os"),
            TargetPlatform::VisionOS
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
        let mut policy = LocalSessionPolicyState {
            policy_epoch: 3,
            max_participants: 250,
            ..Default::default()
        };
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

    #[test]
    fn derive_anon_group_key_is_deterministic() {
        let wrap = "11".repeat(32);
        let first = derive_anon_group_key(&wrap).expect("derive key");
        let second = derive_anon_group_key(&wrap).expect("derive key again");
        assert_eq!(first, second);
    }

    #[test]
    fn map_tui_key_to_control_maps_expected_keys() {
        let key = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("m"));
        let key = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("/quit"));
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("/end"));
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("tab"));
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("p"));
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("a"));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("c"));
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("e"));
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("r"));
        let key = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("b"));
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("j"));
        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("n"));
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("k"));
        let key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("t"));
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("d"));
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("w"));
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("x"));
        let key = KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("0"));
        let key = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), Some("4"));
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_tui_key_to_control_input(key), Some("/quit"));
    }

    #[test]
    fn map_tui_key_to_control_ignores_unmapped() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(map_tui_key_to_control_input(key), None);
    }

    #[test]
    fn anon_group_payload_encrypt_decrypt_roundtrip() {
        let key = [7u8; 32];
        let plaintext = b"video-segment-payload".to_vec();
        let frame = encrypt_anon_group_payload(
            &key,
            "anon-handle-1",
            9,
            AnonymousPayloadKind::VideoSegment,
            &plaintext,
        )
        .expect("encrypt");
        let decrypted = decrypt_anon_group_payload(&key, "anon-handle-1", &frame).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn ascii_lines_from_luma_outputs_stable_dimensions() {
        let mut luma = Vec::with_capacity(64);
        for _y in 0..8 {
            for x in 0..8u8 {
                luma.push(x.saturating_mul(30));
            }
        }
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma,
            updated_at_ms: 0,
        };
        let lines = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            false,
            1_000,
            0,
        );
        assert_eq!(lines.len(), 18);
        assert!(lines.iter().all(|line| line.len() == 64));
    }

    #[test]
    fn ascii_lines_from_luma_edge_mode_changes_gradient_rendering() {
        let mut luma = Vec::with_capacity(64);
        for idx in 0..64u8 {
            luma.push(idx.saturating_mul(4));
        }
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma,
            updated_at_ms: 0,
        };
        let flat = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            false,
            1_000,
            0,
        );
        let edged = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            true,
            false,
            1_000,
            0,
        );
        assert_eq!(flat.len(), edged.len());
        assert_ne!(flat, edged);
    }

    #[test]
    fn ascii_lines_from_luma_rain_overlay_changes_output() {
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma: vec![128; 64],
            updated_at_ms: 0,
        };
        let plain = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            false,
            1_000,
            123,
        );
        let rainy = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            true,
            1_000,
            123,
        );
        assert_eq!(plain.len(), rainy.len());
        assert_ne!(plain, rainy);
    }

    #[test]
    fn ascii_lines_from_luma_datamosh_changes_output() {
        let mut luma = Vec::with_capacity(64);
        for _y in 0..8 {
            for x in 0..8u8 {
                luma.push(x.saturating_mul(30));
            }
        }
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma,
            updated_at_ms: 0,
        };
        let clean = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            false,
            1_000,
            220,
        );
        let mosh = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            true,
            false,
            false,
            false,
            1_000,
            220,
        );
        assert_eq!(clean.len(), mosh.len());
        assert_ne!(clean, mosh);
    }

    #[test]
    fn ascii_lines_from_luma_noise_overlay_changes_output() {
        let mut luma = Vec::with_capacity(64);
        for idx in 0..64u8 {
            luma.push(idx.saturating_mul(3));
        }
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma,
            updated_at_ms: 0,
        };
        let clean = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            false,
            false,
            false,
            1_000,
            220,
        );
        let noisy = ascii_lines_from_luma(
            &frame,
            " .:-=+*#%@",
            false,
            false,
            true,
            false,
            false,
            1_000,
            220,
        );
        assert_eq!(clean.len(), noisy.len());
        assert_ne!(clean, noisy);
    }

    #[test]
    fn matrix_idle_lines_outputs_requested_size() {
        let lines = matrix_idle_lines(80, 24, 12);
        assert_eq!(lines.len(), 24);
        assert!(lines.iter().all(|line| line.len() == 80));
    }

    #[test]
    fn render_audio_meter_is_width_bounded() {
        let meter = render_audio_meter(u16::MAX, 20);
        assert_eq!(meter.len(), 22);
        assert!(meter.starts_with('['));
        assert!(meter.ends_with(']'));
    }

    #[test]
    fn render_boot_bar_caps_fill_to_width() {
        assert_eq!(render_boot_bar(8, 3), "[###.....]");
        assert_eq!(render_boot_bar(8, 99), "[########]");
    }

    #[test]
    fn ascii_density_shades_returns_non_empty_sets() {
        for idx in 0..=5 {
            assert!(!ascii_density_shades(idx).is_empty());
        }
    }

    #[test]
    fn density_cycle_and_labels_are_stable() {
        assert_eq!(density_label(0), "LITE");
        assert_eq!(density_label(1), "GRID");
        assert_eq!(density_label(2), "ULTRA");
        assert_eq!(density_label(3), "BOLD");
        assert_eq!(density_label(4), "CHROME");
        assert_eq!(cycle_density_idx(0), 1);
        assert_eq!(cycle_density_idx(1), 2);
        assert_eq!(cycle_density_idx(2), 3);
        assert_eq!(cycle_density_idx(3), 4);
        assert_eq!(cycle_density_idx(4), 0);
    }

    #[test]
    fn sanitize_snapshot_label_normalizes_identifiers() {
        assert_eq!(sanitize_snapshot_label("Alice/Room:01"), "alice-room-01");
        assert_eq!(sanitize_snapshot_label(""), "snapshot");
    }

    #[test]
    fn record_flow_telemetry_computes_rate() {
        let mut flow = FlowTelemetry::default();
        record_flow_telemetry(&mut flow, 100);
        record_flow_telemetry(&mut flow, 350);
        record_flow_telemetry(&mut flow, 700);
        record_flow_telemetry(&mut flow, 1_100);
        assert_eq!(flow.rate_per_sec, 4);
        assert_eq!(flow.total, 4);
        assert_eq!(flow.last_at_ms, 1_100);
    }

    #[test]
    fn classify_conference_quality_distinguishes_states() {
        let mut telemetry = ConferenceTelemetry::default();
        assert_eq!(
            classify_conference_quality(10_000, &telemetry, false),
            "IDLE"
        );

        telemetry.rx_video.last_at_ms = 9_800;
        telemetry.rx_video.rate_per_sec = 9;
        assert_eq!(
            classify_conference_quality(10_000, &telemetry, true),
            "NEON-GOOD"
        );

        telemetry.rx_video.last_at_ms = 8_000;
        telemetry.rx_video.rate_per_sec = 1;
        assert_eq!(
            classify_conference_quality(10_000, &telemetry, true),
            "DEGRADED"
        );
    }

    #[test]
    fn render_flow_age_handles_missing_and_present_values() {
        assert_eq!(render_flow_age(u64::MAX), "--");
        assert_eq!(render_flow_age(321), "321ms");
    }

    #[test]
    fn record_rtt_sample_updates_last_and_ewma() {
        let mut telemetry = ConferenceTelemetry::default();
        record_rtt_sample(&mut telemetry, 120);
        assert_eq!(telemetry.rtt_last_ms, 120);
        assert_eq!(telemetry.rtt_ewma_ms, 120);
        assert_eq!(telemetry.rtt_samples, 1);

        record_rtt_sample(&mut telemetry, 200);
        assert_eq!(telemetry.rtt_last_ms, 200);
        assert_eq!(telemetry.rtt_samples, 2);
        assert!(telemetry.rtt_ewma_ms >= 120);
        assert!(telemetry.rtt_ewma_ms <= 200);
    }

    #[test]
    fn render_rtt_handles_empty_and_populated_telemetry() {
        let telemetry = ConferenceTelemetry::default();
        assert_eq!(render_rtt(&telemetry), "--");

        let telemetry = ConferenceTelemetry {
            rtt_last_ms: 90,
            rtt_ewma_ms: 110,
            rtt_samples: 3,
            ..ConferenceTelemetry::default()
        };
        assert_eq!(render_rtt(&telemetry), "90ms/110ms");
    }

    #[test]
    fn quality_level_from_label_maps_expected_buckets() {
        assert_eq!(quality_level_from_label("NEON-GOOD"), 4);
        assert_eq!(quality_level_from_label("AUDIO-WARM"), 3);
        assert_eq!(quality_level_from_label("NOISY"), 2);
        assert_eq!(quality_level_from_label("DEGRADED"), 1);
        assert_eq!(quality_level_from_label("IDLE"), 0);
    }

    #[test]
    fn quality_history_sampling_throttles_and_caps() {
        let mut telemetry = ConferenceTelemetry::default();
        record_quality_history_sample(&mut telemetry, 1_000, 4);
        record_quality_history_sample(&mut telemetry, 1_200, 1);
        assert_eq!(telemetry.quality_history, vec![4]);
        for i in 0..80u64 {
            record_quality_history_sample(
                &mut telemetry,
                2_000 + i.saturating_mul(900),
                (i % 5) as u8,
            );
        }
        assert!(telemetry.quality_history.len() <= 64);
    }

    #[test]
    fn render_quality_history_handles_empty_and_fixed_width() {
        assert_eq!(render_quality_history(&[], 10), "--");
        let history = vec![0, 1, 2, 3, 4];
        let rendered = render_quality_history(&history, 8);
        assert_eq!(rendered.len(), 8);
        assert!(rendered.ends_with(" .:*#"));
    }

    #[test]
    fn record_rx_sequence_telemetry_tracks_loss_and_jitter() {
        let mut flow = FlowTelemetry::default();
        record_rx_sequence_telemetry(&mut flow, 10, 100, 100);
        record_rx_sequence_telemetry(&mut flow, 11, 200, 100);
        record_rx_sequence_telemetry(&mut flow, 14, 380, 100);
        assert_eq!(flow.sequence_packets, 3);
        assert_eq!(flow.missing_packets, 2);
        assert_eq!(flow.last_seq, Some(14));
        assert!(flow.jitter_ewma_ms > 0);
    }

    #[test]
    fn render_loss_and_jitter_handle_empty_and_populated() {
        let mut flow = FlowTelemetry::default();
        assert_eq!(render_loss_percent(&flow), "--");
        assert_eq!(render_jitter_ms(&flow), "--");

        record_rx_sequence_telemetry(&mut flow, 1, 100, 100);
        record_rx_sequence_telemetry(&mut flow, 3, 260, 100);
        assert_eq!(render_loss_percent(&flow), "33.3%");
        assert!(render_jitter_ms(&flow).ends_with("ms"));
    }

    #[test]
    fn classify_conference_quality_reports_noisy_with_high_loss() {
        let mut telemetry = ConferenceTelemetry::default();
        telemetry.rx_video.last_at_ms = 9_900;
        telemetry.rx_video.rate_per_sec = 8;
        telemetry.rx_video.sequence_packets = 10;
        telemetry.rx_video.missing_packets = 3;
        assert_eq!(
            classify_conference_quality(10_000, &telemetry, true),
            "NOISY"
        );
    }

    #[test]
    fn participant_quality_score_penalizes_loss_and_jitter() {
        let mut stats = ParticipantMediaTelemetry::default();
        stats.video.rate_per_sec = 7;
        stats.video.sequence_packets = 12;
        stats.video.missing_packets = 3;
        stats.video.jitter_ewma_ms = 220;
        stats.video.last_at_ms = 9_900;
        let score = participant_quality_score(10_000, &stats, 80);
        assert!(score <= 3);
    }

    #[test]
    fn quality_bar_and_label_render_expected_values() {
        assert_eq!(render_quality_bar(5), "[#####]");
        assert_eq!(render_quality_bar(2), "[##---]");
        assert_eq!(quality_label(5), "NEON");
        assert_eq!(quality_label(3), "WARM");
        assert_eq!(quality_label(1), "BAD");
    }

    #[test]
    fn choose_adaptive_video_profile_escalates_under_bad_network() {
        let mut telemetry = ConferenceTelemetry::default();
        let baseline = choose_adaptive_video_profile(&telemetry);
        assert_eq!(baseline, (100, 20, 0));

        telemetry.rx_video.sequence_packets = 20;
        telemetry.rx_video.missing_packets = 6;
        telemetry.rx_video.jitter_ewma_ms = 320;
        telemetry.rtt_ewma_ms = 700;
        let degraded = choose_adaptive_video_profile(&telemetry);
        assert_eq!(degraded, (220, 34, 3));
    }

    #[test]
    fn adaptive_mode_label_maps_levels() {
        assert_eq!(adaptive_mode_label(0), "BOOST");
        assert_eq!(adaptive_mode_label(1), "WARM");
        assert_eq!(adaptive_mode_label(2), "BAL");
        assert_eq!(adaptive_mode_label(3), "SAFE");
        assert_eq!(adaptive_mode_label(9), "UNK");
    }

    #[test]
    fn adaptive_override_label_maps_none_and_levels() {
        assert_eq!(adaptive_override_label(None), "AUTO");
        assert_eq!(adaptive_override_label(Some(0)), "BOOST");
        assert_eq!(adaptive_override_label(Some(3)), "SAFE");
    }

    #[test]
    fn adaptive_profile_for_level_matches_expected_profiles() {
        assert_eq!(adaptive_profile_for_level(0), (100, 20, 0));
        assert_eq!(adaptive_profile_for_level(1), (130, 24, 1));
        assert_eq!(adaptive_profile_for_level(2), (160, 28, 2));
        assert_eq!(adaptive_profile_for_level(3), (220, 34, 3));
    }

    #[test]
    fn cycle_adaptive_override_rotates_and_resets() {
        assert_eq!(cycle_adaptive_override(None), Some(0));
        assert_eq!(cycle_adaptive_override(Some(0)), Some(1));
        assert_eq!(cycle_adaptive_override(Some(1)), Some(2));
        assert_eq!(cycle_adaptive_override(Some(2)), Some(3));
        assert_eq!(cycle_adaptive_override(Some(3)), None);
        assert_eq!(cycle_adaptive_override(Some(9)), None);
    }

    #[test]
    fn adaptive_override_from_shortcut_maps_expected_inputs() {
        assert_eq!(adaptive_override_from_shortcut("0"), Some(None));
        assert_eq!(adaptive_override_from_shortcut("1"), Some(Some(0)));
        assert_eq!(adaptive_override_from_shortcut("2"), Some(Some(1)));
        assert_eq!(adaptive_override_from_shortcut("3"), Some(Some(2)));
        assert_eq!(adaptive_override_from_shortcut("4"), Some(Some(3)));
        assert_eq!(adaptive_override_from_shortcut("9"), None);
    }

    #[test]
    fn adaptive_audio_profile_for_level_matches_expected_profiles() {
        assert_eq!(adaptive_audio_profile_for_level(0), (100, 1_000));
        assert_eq!(adaptive_audio_profile_for_level(1), (110, 920));
        assert_eq!(adaptive_audio_profile_for_level(2), (130, 800));
        assert_eq!(adaptive_audio_profile_for_level(3), (160, 680));
    }

    #[test]
    fn sort_mode_label_maps_values() {
        assert_eq!(sort_mode_label(false), "ID");
        assert_eq!(sort_mode_label(true), "WORST");
    }

    #[test]
    fn dashboard_theme_cycle_and_labels_are_stable() {
        assert_eq!(dashboard_theme_label(0), "MATRIX");
        assert_eq!(dashboard_theme_label(1), "NEON-ICE");
        assert_eq!(dashboard_theme_label(2), "SYNTHWAVE");
        assert_eq!(dashboard_theme_label(3), "BLADE");
        assert_eq!(cycle_dashboard_theme(0), 1);
        assert_eq!(cycle_dashboard_theme(3), 0);
    }

    #[test]
    fn resolve_theme_index_respects_auto_mode() {
        assert_eq!(resolve_theme_index(1, false, 100), 1);
        assert_eq!(resolve_theme_index(1, true, 0), 1);
        assert_eq!(resolve_theme_index(1, true, 48), 3);
    }

    #[test]
    fn luma_boost_profiles_and_cycle_are_stable() {
        assert_eq!(luma_boost_label(0), "DIM");
        assert_eq!(luma_boost_label(1), "NORM");
        assert_eq!(luma_boost_label(2), "HOT");
        assert_eq!(luma_boost_label(3), "MAX");
        assert_eq!(luma_boost_permille(0), 900);
        assert_eq!(luma_boost_permille(1), 1_000);
        assert_eq!(luma_boost_permille(2), 1_200);
        assert_eq!(luma_boost_permille(3), 1_450);
        assert_eq!(cycle_luma_boost_level(0), 1);
        assert_eq!(cycle_luma_boost_level(1), 2);
        assert_eq!(cycle_luma_boost_level(2), 3);
        assert_eq!(cycle_luma_boost_level(3), 0);
    }

    #[test]
    fn pulse_rgb_brightens_without_overflow() {
        let color = pulse_rgb((250, 240, 230), 5, 8);
        assert_eq!(color, (255, 255, 255));
        let color = pulse_rgb((10, 20, 30), 0, 8);
        assert_eq!(color, (10, 20, 30));
    }

    #[test]
    fn sorted_feed_ids_can_prioritize_worst_quality() {
        let frame = AsciiVideoFrame {
            width: 8,
            height: 8,
            luma: vec![128; 64],
            updated_at_ms: 9_980,
        };
        let mut frames = HashMap::new();
        frames.insert("a".to_string(), frame.clone());
        frames.insert("b".to_string(), frame);

        let mut pstats = HashMap::<String, ParticipantMediaTelemetry>::new();
        let mut a = ParticipantMediaTelemetry::default();
        a.video.rate_per_sec = 8;
        a.video.sequence_packets = 20;
        a.video.missing_packets = 0;
        a.video.jitter_ewma_ms = 20;
        a.video.last_at_ms = 9_980;
        pstats.insert("a".to_string(), a);

        let mut b = ParticipantMediaTelemetry::default();
        b.video.rate_per_sec = 2;
        b.video.sequence_packets = 20;
        b.video.missing_packets = 8;
        b.video.jitter_ewma_ms = 320;
        b.video.last_at_ms = 9_600;
        pstats.insert("b".to_string(), b);

        let mut controls = ConferenceControls::default();
        controls.sort_worst_first = false;
        let ids_default = sorted_feed_ids(&frames, &pstats, &controls, 10_000);
        assert_eq!(ids_default, vec!["a".to_string(), "b".to_string()]);

        controls.sort_worst_first = true;
        let ids_worst = sorted_feed_ids(&frames, &pstats, &controls, 10_000);
        assert_eq!(ids_worst.first(), Some(&"b".to_string()));
    }

    #[test]
    fn top_worst_peers_orders_by_quality_then_age() {
        let mut frames = HashMap::new();
        frames.insert(
            "a".to_string(),
            AsciiVideoFrame {
                width: 8,
                height: 8,
                luma: vec![128; 64],
                updated_at_ms: 9_980,
            },
        );
        frames.insert(
            "b".to_string(),
            AsciiVideoFrame {
                width: 8,
                height: 8,
                luma: vec![128; 64],
                updated_at_ms: 9_000,
            },
        );
        frames.insert(
            "c".to_string(),
            AsciiVideoFrame {
                width: 8,
                height: 8,
                luma: vec![128; 64],
                updated_at_ms: 9_900,
            },
        );

        let mut pstats = HashMap::<String, ParticipantMediaTelemetry>::new();
        let mut a = ParticipantMediaTelemetry::default();
        a.video.rate_per_sec = 8;
        a.video.sequence_packets = 20;
        a.video.missing_packets = 0;
        a.video.jitter_ewma_ms = 10;
        a.video.last_at_ms = 9_980;
        pstats.insert("a".to_string(), a);

        let mut b = ParticipantMediaTelemetry::default();
        b.video.rate_per_sec = 1;
        b.video.sequence_packets = 20;
        b.video.missing_packets = 9;
        b.video.jitter_ewma_ms = 420;
        b.video.last_at_ms = 9_000;
        pstats.insert("b".to_string(), b);

        let mut c = ParticipantMediaTelemetry::default();
        c.video.rate_per_sec = 4;
        c.video.sequence_packets = 20;
        c.video.missing_packets = 2;
        c.video.jitter_ewma_ms = 80;
        c.video.last_at_ms = 9_900;
        pstats.insert("c".to_string(), c);

        let worst = top_worst_peers(&frames, &pstats, 10_000, 3);
        assert!(worst.starts_with("b:"));
        assert!(worst.contains("a:"));
        assert!(worst.contains("c:"));
    }

    #[test]
    fn top_worst_peers_handles_empty_input() {
        let frames = HashMap::<String, AsciiVideoFrame>::new();
        let pstats = HashMap::<String, ParticipantMediaTelemetry>::new();
        assert_eq!(top_worst_peers(&frames, &pstats, 1_000, 3), "--");
    }
}
