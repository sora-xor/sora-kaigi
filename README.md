# sora-kaigi

Video + audio conferencing built on Kaigi over Sora Nexus / SoraNet powered by Hyperledger Iroha 3.

This repo is intentionally split into small building blocks. Right now the focus is on the
networking “spine” (SoraNet relay handshake + Kaigi stream open) so we can test the end-to-end
plumbing before UI/audio/video work.

## Current Status

- `kaigi-soranet-client`: QUIC client + SoraNet application-layer handshake (frames match
  `../iroha/tools/soranet-relay`) + Kaigi stream open frame (34-byte header).
- `kaigi-wire`: framed control-plane protocol (`u32(be) len` + Norito payload) carried over a Kaigi
  stream (roster/events/chat/participant state + dev payment signalling), plus anonymous-mode
  encrypted control envelopes and escrow proof frames.
- `kaigi-hub-echo`: WebSocket “hub adapter” dev harness that accepts the relay’s Kaigi handshake
  (`KaigiStreamOpen`, Norito-encoded), groups connections by `room_id`, and broadcasts
  roster/events/chat.
  - Enforces: mic OFF + camera OFF + screen share OFF on join.
  - Enforces: limited concurrent screen shares (default: 1; configurable via room config).
  - Host moderation: mute/stop video/stop share/kick (signalled via `Moderation` frames).
  - Pay-per-use enforcement (nano-XOR). Uses a per-room rate (defaults to non-zero), host can
    update at runtime via `RoomConfigUpdate` (`/rate`, normalized to `>= 1` unless
    `--allow-free-calls` is enabled).
  - By default, payment frames must include `tx_hash_hex` when the room rate is non-zero.
  - Anonymous mode (`privacy=zk`): blind relay behavior for encrypted control payloads + key-update
    fanout + escrow proof acknowledgements, with stale-proof disconnect guard
    (`--anon-escrow-proof-max-stale-secs`).
- `kaigi-cli`: demo tool: connect to a relay, open a Kaigi stream, and talk to the hub.
  - `relay-echo`: send `Hello`/`Chat`/`Ping`, print received frames (smoke test).
  - `room-chat`: interactive room chat (`/mic`, `/video`, `/share`, `/rate`, `/maxshare`, `/mute*`, `/videooff`, `/shareoff`, `/kick`, `/end` host-only, `/pay`, `/quit`).
  - `make-join-link` / `decode-join-link`: shareable room routing links (`kaigi://join?...`) with optional Torii/billing/lifecycle metadata.
  - `kaigi-lifecycle`: wrapper for `iroha app kaigi` (`create`, `join`, `leave`, `end`, `record-usage`) with ZK/privacy payload passthrough fields.
  - `write-route-update`: helper for provisioning relay spool route entries on disk.
  - `list-routes`: decode current `kaigi-stream/*.norito` route records from spool catalogs.

## Dev Notes

SoraNet relay routing currently depends on a provisioned `channel_id` in the relay’s spool route
catalog (filesystem). You can still pass `--channel <32-byte-hex>` directly, or generate/share
links with `kaigi-cli make-join-link` and then join via `kaigi-cli room-chat --join-link ...`.
Join links can also embed optional billing destination (`pay_to=<account-id>`) and Kaigi lifecycle
metadata (`kaigi_domain`, `kaigi_call_name`, `kaigi_privacy_mode`) to reduce the number of flags
needed when joining.

## Quick Usage (Requires a Running Relay)

1. Run the hub echo server:

```bash
cargo run -p kaigi-hub-echo -- \
  --listen 127.0.0.1:9000 \
  --anon-zk-extra-fee-per-minute-nano 250000
```

2. Start a `soranet-relay` (from `../iroha`) configured to route `kaigi_stream.hub_ws_url` to
   `ws://127.0.0.1:9000` and with a provisioned Kaigi route for your chosen `channel_id`.

3. Connect (interactive room chat):

```bash
cargo run -p kaigi-cli -- room-chat \
  --relay 127.0.0.1:5000 \
  --torii http://127.0.0.1:8080 \
  --server-name localhost \
  --insecure \
  --channel <64-hex-bytes> \
  --display-name "Alice" \
  --hdr-display \
  --pay-iroha-config ../iroha/demo/alice.toml \
  --pay-to <billing-account-id>
```

4. Optional: generate a shareable join link and use it instead of passing relay/channel manually:

```bash
cargo run -p kaigi-cli -- make-join-link \
  --relay 127.0.0.1:5000 \
  --torii http://127.0.0.1:8080 \
  --channel <64-hex-bytes> \
  --server-name localhost \
  --insecure \
  --pay-to <billing-account-id> \
  --kaigi-domain sora \
  --kaigi-call-name standup \
  --kaigi-privacy-mode zk
```

Then:

```bash
export KAIGI_PAY_IROHA_CONFIG=../iroha/demo/bob.toml
cargo run -p kaigi-cli -- room-chat \
  --join-link 'kaigi://join?...' \
  --display-name "Bob"
```

HDR note: `room-chat` auto-detects HDR display capability (macOS best-effort) unless
`--no-hdr-auto` is passed. `--hdr-display` / `--hdr-capture` remain explicit overrides.

Audio note: there is no separate “connect to audio” flow; audio path is active on join.

Routing note: `room-chat` requires Torii handshake discovery (`--torii`, `KAIGI_TORII`, or
`--join-link` with `torii=...`) by default. Use `--allow-local-handshake` only for local harness
testing.
The same Torii requirement applies to `make-join-link` (and `write-route-update --relay ...`
when emitting `join_link=` output), unless `--allow-local-handshake` is provided.

Host note: `/end` is host-only and ends the meeting for all participants (including host).
`room-chat` blocks `/end` locally when host role is known to belong to someone else.
If host role is still unknown, it asks you to wait for `room_config` before `/end`.

Lifecycle note: `room-chat` can mirror call state to Nexus automatically by passing
`--kaigi-iroha-config --kaigi-domain --kaigi-call-name --kaigi-participant` (or using
`KAIGI_LIFECYCLE_IROHA_CONFIG` / `KAIGI_DOMAIN` / `KAIGI_CALL_NAME` / `KAIGI_PARTICIPANT`);
it will submit
`join` on connect and `leave` on disconnect, with optional `--kaigi-record-usage`.
Lifecycle mirroring override flags are rejected unless `--kaigi-iroha-config` is provided.
`--kaigi-billed-gas` / usage proof flags require `--kaigi-record-usage`.
`--kaigi-participant` without lifecycle config is also rejected.
`--kaigi-join-commitment-alias` requires `--kaigi-join-commitment-hex`.
`--kaigi-nullifier-issued-at-ms` requires at least one of
`--kaigi-join-nullifier-hex` or `--kaigi-leave-nullifier-hex`.
When `/end` is used, it submits `end` before disconnecting (and skips `leave` if `end` succeeds).

## Kaigi Lifecycle (On-ledger)

Use `kaigi-lifecycle` when you want to mirror room lifecycle to Nexus/Iroha call state:

```bash
cargo run -p kaigi-cli -- kaigi-lifecycle \
  --iroha-config ../iroha/demo/alice.toml \
  create --domain sora --call-name standup --host <account-id> --room-policy authenticated
```

Then join/leave/end/record usage with the corresponding subcommands:
`join`, `leave`, `end`, `record-usage`.
`create --room-policy` accepts `public`, `authenticated`, or `auth` (case-insensitive).
`create --privacy-mode` accepts `transparent`, `zk`, `zk_roster_v1`, or `zk-roster-v1` (case-insensitive).
For privacy-enabled calls, `create --privacy-mode zk --zk-extra-fee-per-minute-nano <u64>`
adds the zk surcharge directly into the submitted gas rate (rejected for transparent mode).
If `--gas-rate-per-minute + --zk-extra-fee-per-minute-nano` exceeds `u64`, the CLI rejects
the command.

For privacy-enabled calls, `kaigi-lifecycle` accepts upstream ZK fields such as
`--commitment-hex`, `--nullifier-hex`, `--roster-root-hex`, `--proof-hex`, and
`record-usage --usage-commitment-hex ... --proof-hex ...`.
Lifecycle hex args accept optional `0x`/`0X` prefixes and are normalized before the upstream
`iroha` call.
`--commitment-alias` requires `--commitment-hex`, and
`--nullifier-issued-at-ms` requires `--nullifier-hex`.

## Anonymous Mode (`privacy=zk`)

- `room-chat` automatically switches to anonymous mode when `kaigi_privacy_mode` resolves to `zk`
  (`zk`, `zk_roster_v1`, or `zk-roster-v1`; from flag, env, or join link metadata).
- Anonymous mode uses opaque participant handles, X25519 key updates, and encrypted control
  envelopes (`EncryptedControl`).
  - Anonymous participant handles must be non-empty, ASCII, <= 128 chars, and must not include
    `@` or whitespace/control characters.
  - Hub enforces monotonic `GroupKeyUpdate.epoch` per sender to reject stale key replays.
  - Hub requires `EncryptedControl.epoch` to match the sender's current key epoch.
  - Hub rejects malformed key updates (`participant_handle` must be non-empty).
  - Hub bounds encrypted payload size (ciphertext <= 64KiB hex chars per recipient entry).
  - Hub rejects implausibly short encrypted ciphertext payloads (< 32 hex chars).
  - Hub caps encrypted fanout to 256 recipients per `EncryptedControl` frame.
  - Hub rejects encrypted recipient handles not present in the current anonymous roster.
  - Hub caps anonymous participants per room (default 256, configurable via `--anon-max-participants`).
  - Hub logs anonymous admission rejections with a per-room rejection counter.
- Optional shielded prepay flow:
  - `--anon-escrow-prepay-nano <u64>` submits `iroha app zk shield` before chat starts.
    Requires `--pay-iroha-config` and `--pay-from`.
  - Optional zk surcharge: `--anon-zk-extra-fee-per-minute-nano <u64>` with
    `--anon-expected-duration-secs <u64>` adds extra XOR to the shielded prepay amount
    (requires non-zero `--anon-escrow-prepay-nano` and positive expected duration).
    If the computed total exceeds `u64` nano-XOR, the CLI rejects the configuration.
  - Client sends periodic/on-demand `EscrowProof` frames (no clear amount on wire),
    controlled by `--anon-escrow-proof-interval-secs` (`payer_handle` and `escrow_id` must be
    non-empty ASCII identifiers,
    <= 128 chars, and must not include `@` or whitespace/control characters; `proof_hex` must be
    non-empty valid hex with max 64KiB hex chars).
    `AnonHello` alone does not satisfy escrow freshness; an `EscrowProof` is required before
    the stale window expires.
    Connections in anonymous rooms that never send `AnonHello` are also disconnected when
    the same stale window expires.
    Hub requires `escrow_id` to remain stable for each anonymous participant session.
  - Optional teardown unshield is available via `--anon-unshield-on-exit` plus unshield args
    (`--anon-unshield-to`, `--anon-unshield-inputs`, `--anon-unshield-proof-json`,
    `--pay-iroha-config`, and non-zero `--anon-escrow-prepay-nano`).
    `--anon-unshield-inputs` must be a comma-separated list of 32-byte hex values; optional
    `--anon-unshield-root-hint-hex` must be 32-byte hex.
    `--anon-unshield-inputs` is capped at 256 entries.
    Duplicate nullifiers in `--anon-unshield-inputs` are rejected.
    Unshield detail args are rejected unless `--anon-unshield-on-exit` is set.
- Anonymous escrow flags are rejected unless privacy mode is `zk`.
- Transparent payment-loop flags (`--pay-rate-per-minute-nano`, `--pay-auto`,
  `--no-pay-auto`, `--allow-unsettled-payments`) are rejected in anonymous mode.

## Payments (Dev Harness)

- Hub enforcement: `kaigi-hub-echo` defaults to `1_000_000` nano-XOR/min and requires
  `Payment.tx_hash_hex` when the room rate is non-zero. Override defaults with
  `--xor-rate-per-minute-nano`, `--billing-grace-secs`, and `--billing-check-interval-secs`.
  Optional anonymous surcharge: `--anon-zk-extra-fee-per-minute-nano <u64>` adds extra
  nano-XOR/min to the room rate when the room first enters anonymous mode (`AnonHello`).
  Overflow during surcharge application is rejected.
  High `--anon-max-participants` values (>1024) and high
  `--anon-escrow-proof-max-stale-secs` values (>3600) emit startup warnings.
  `--anon-escrow-proof-max-stale-secs=0` also emits a startup warning because stale enforcement
  is disabled.
  Use `--allow-unhashed-payments` only for local dev without on-ledger transfers.
  Zero-rate rooms are rejected unless `--allow-free-calls` is set.
- The first participant to send `Hello` becomes host and can update the rate live via
  `/rate <nano_per_min>` (normalized to `>= 1` unless hub started with `--allow-free-calls`).
- Client pay loop:
  - Auto (follow hub): enabled by default.
  - Fixed: `kaigi-cli room-chat --no-pay-auto --pay-rate-per-minute-nano <u64>`
- Real XOR transfers: by default, `room-chat` requires `--pay-iroha-config <path>` and
  `--pay-to <account-id>` for paid calls, and submits transfers via upstream `iroha` CLI
  (optionally `--pay-iroha-bin <path>` / `--pay-from <account-id>`).
  - Env shortcuts: `KAIGI_PAY_IROHA_CONFIG` (or `IROHA_CONFIG`) and `KAIGI_PAY_TO` are applied
    when the corresponding flags are omitted; join links can also embed `pay_to=...`.
- Dev-only bypass: `kaigi-cli room-chat --allow-unsettled-payments` sends payment frames without
  on-ledger transfers.
