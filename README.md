# sora-kaigi

Video + audio conferencing built on Kaigi over Sora Nexus / SoraNet powered by Hyperledger Iroha 3.

This repo is intentionally split into small building blocks. The networking “spine” (SoraNet relay
handshake + Kaigi stream open) is now wired to an experimental console conferencing runtime with
ASCII-rendered video and synthetic A/V media frames for end-to-end transport testing.

## Current Status

- `kaigi-soranet-client`: QUIC client + SoraNet application-layer handshake (frames match
  `../iroha/tools/soranet-relay`) + Kaigi stream open frame (34-byte header).
- `kaigi-wire`: framed control-plane protocol (`u32(be) len` + Norito payload) carried over a Kaigi
  stream (roster/events/chat/participant state + dev payment signalling), plus anonymous-mode
  encrypted control envelopes, media capability/track/media payload frames, and anonymous
  group-encrypted media envelopes.
- `kaigi-hub-echo`: WebSocket “hub adapter” dev harness that accepts the relay’s Kaigi handshake
  (`KaigiStreamOpen`, Norito-encoded), groups connections by `room_id`, and broadcasts
  roster/events/chat.
  - Enforces: mic OFF + camera OFF + screen share OFF on join.
  - Enforces: limited concurrent screen shares (default: 1; configurable via room config).
  - Host/co-host moderation: mute/stop video/stop share/kick (signalled via `Moderation` and
    `ModerationSigned` frames). Accepted moderation actions are rebroadcast for audit visibility.
  - Media profile negotiation enforces HDR capability prerequisites and coerces unsupported HDR
    paths to SDR fallback.
  - Pay-per-use enforcement (nano-XOR). Uses a per-room rate (defaults to non-zero), host can
    update at runtime via `RoomConfigUpdate` (`/rate`, normalized to `>= 1` unless
    `--allow-free-calls` is enabled).
  - By default, payment frames must include `tx_hash_hex` when the room rate is non-zero.
  - Default session policy requires E2EE bootstrap (`E2EEKeyEpoch`) before plaintext chat/state
    control frames are accepted.
  - Anonymous mode (`privacy=zk`): blind relay behavior for encrypted control payloads + key-update
    fanout + hub-driven anonymous group key rotation + encrypted media envelope fanout + escrow
    proof acknowledgements, with stale-proof disconnect guard
    (`--anon-escrow-proof-max-stale-secs`).
- `kaigi-cli`: demo tool: connect to a relay, open a Kaigi stream, and talk to the hub.
  - `relay-echo`: send `Hello`/`Chat`/`Ping`, print received frames (smoke test).
  - `room-chat`: conference runtime (default) with keyboard media controls and optional fullscreen
    ASCII TUI (`--tui`). Legacy slash-command chat is still available via hidden `--legacy-ui`.
  - `ascii-live`: cyberpunk console renderer alias for `room-chat --tui`.
  - `ascii-play`: local video-file -> ASCII cyberpunk renderer (ffmpeg-backed, no relay required).
  - `make-join-link` / `decode-join-link`: shareable room routing links (`kaigi://join?...`) with optional Torii/billing/lifecycle metadata.
  - `platform-contract`: emits frozen browser/native parity contract JSON (`all` or `--platform <target>`).
  - `kaigi-lifecycle`: wrapper for `iroha app kaigi` (`create`, `join`, `leave`, `end`, `record-usage`) with ZK/privacy payload passthrough fields.
  - `write-route-update`: helper for provisioning relay spool route entries on disk.
  - `list-routes`: decode current `kaigi-stream/*.norito` route records from spool catalogs.

## Roadmap and Specs

- Product roadmap: [`roadmap.md`](roadmap.md)
- Platform parity matrix (frozen M0): [`docs/parity-matrix.md`](docs/parity-matrix.md)
- Frozen platform contract artifact: [`docs/platform-contract.json`](docs/platform-contract.json)
- Frozen media capability profiles: [`docs/media-capability-profiles.json`](docs/media-capability-profiles.json)
- Frozen HDR transport profiles: [`docs/hdr-transport-profiles.json`](docs/hdr-transport-profiles.json)
- Frozen HDR target-device run results: [`docs/hdr-target-device-results.json`](docs/hdr-target-device-results.json)
- Frozen A/V baseline profiles: [`docs/av-baseline-profiles.json`](docs/av-baseline-profiles.json)
- Frozen screen-share constraints: [`docs/screen-share-constraints.json`](docs/screen-share-constraints.json)
- Frozen parity status contract: [`docs/parity-status-contract.json`](docs/parity-status-contract.json)
- Parity status waiver contract: [`docs/parity-status-waivers.json`](docs/parity-status-waivers.json)
- Parity waiver policy contract: [`docs/parity-waiver-policy.json`](docs/parity-waiver-policy.json)
- Client app workspaces contract: [`docs/client-app-workspaces.json`](docs/client-app-workspaces.json)
- Client release-track contract: [`docs/client-release-tracks.json`](docs/client-release-tracks.json)
- Client fallback-drills contract: [`docs/client-fallback-drills.json`](docs/client-fallback-drills.json)
- Client fallback-drill-results contract: [`docs/client-fallback-drill-results.json`](docs/client-fallback-drill-results.json)
- Client release-manifest contract: [`docs/client-release-manifest.json`](docs/client-release-manifest.json)
- Client release-readiness-gates contract: [`docs/client-release-readiness-gates.json`](docs/client-release-readiness-gates.json)
- Client rollback-manifest contract: [`docs/client-rollback-manifest.json`](docs/client-rollback-manifest.json)
- Parity waiver fixture manifest: [`docs/fixtures/waivers/manifest.json`](docs/fixtures/waivers/manifest.json)
- Platform hardening gate contract: [`docs/hardening-gates.json`](docs/hardening-gates.json)
- Single-train GA approval contract: [`docs/ga-approvals.json`](docs/ga-approvals.json)
- Platform blocker ledger: [`docs/platform-blockers.json`](docs/platform-blockers.json)
- Critical defect ledger: [`docs/critical-defects.json`](docs/critical-defects.json)
- Release playbook: [`docs/release-playbook.md`](docs/release-playbook.md)
- Rollback playbook: [`docs/rollback-playbook.md`](docs/rollback-playbook.md)
- Protocol vNext contract (frozen M0): [`docs/protocol-vnext.md`](docs/protocol-vnext.md)
- Conformance test plan (frozen M0): [`docs/test-plan.md`](docs/test-plan.md)
- CI conformance contract workflow: [`.github/workflows/conformance-matrix.yml`](.github/workflows/conformance-matrix.yml)

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

3. Connect (cyberpunk ASCII conference TUI):

```bash
cargo run -p kaigi-cli -- ascii-live \
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

`ascii-live` is equivalent to `room-chat --tui`.
`ascii-live` controls: `m` mic, `v` camera, `s` screen share, `p` pause,
`a` adaptive override cycle, `0..4` direct adaptive mode (`0=AUTO`, `1=BOOST`, `2=WARM`,
`3=BAL`, `4=SAFE`), `c` cyberpunk theme cycle, `t` auto theme-cycle toggle, `d` density-cycle, `b` luma boost-cycle, `g` glitch toggle, `j` datamosh toggle, `n` noise overlay toggle, `e` edge-enhance toggle, `w` worst-feed sort toggle, `x` telemetry reset, `+/-` density,
`r` rain-overlay toggle, `k` snapshot export, `left/right/up/down` focus, `tab` advanced HUD, `h` help-overlay toggle, `q` quit, `End` end-call.
The HUD includes a live status strip with quality classification, RTT, RX/TX media rates,
and estimated RX loss/jitter. Each participant feed also shows a per-peer quality bar,
video loss, and jitter summary. The synthetic sender adapts video cadence/quantizer based on
measured network quality (`BOOST`/`WARM`/`BAL`/`SAFE`) with optional manual override, and applies
matching adaptive audio pacing/gain tiers. `tab` toggles an advanced HUD section with adaptive
cycle legend, live RX/TX/ping counters, a quality trend sparkline, and top worst peers.
`c` rotates built-in neon palettes (`MATRIX`, `NEON-ICE`, `SYNTHWAVE`, `BLADE`) in real time.
TUI startup includes a short neon boot sequence before the live dashboard appears.

Local file playback (no relay/hub):

```bash
cargo run -p kaigi-cli -- ascii-play \
  --input ./demo.mp4 \
  --fps 18 \
  --width 96 \
  --height 54 \
  --density 2 \
  --theme 2 \
  --glitch
```

`ascii-play` requires `ffmpeg` (or `--ffmpeg-bin <path>`), and supports `--loop` for endless playback.
During playback: `c` cycles themes, `t` toggles auto theme-cycle, `d` cycles density presets, `b` cycles luma boost profiles, `+/-` adjusts density, `g` toggles glitch, `j` toggles datamosh row-shift, `n` toggles static noise overlay, `e` toggles edge-enhance, `r` toggles rain overlay, `k` writes a snapshot, `p` pauses, `q` quits.
Snapshots are saved under `ascii-snapshots/` as timestamped `.txt` files.

4. Optional: generate a shareable join link and use it instead of passing relay/channel manually:

```bash
cargo run -p kaigi-cli -- make-join-link \
  --relay 127.0.0.1:5000 \
  --torii http://127.0.0.1:8080 \
  --channel <64-hex-bytes> \
  --expires-in-secs 3600 \
  --server-name localhost \
  --insecure \
  --pay-to <billing-account-id> \
  --kaigi-domain sora \
  --kaigi-call-name standup \
  --kaigi-privacy-mode zk
```

`make-join-link` emits signed `v=2` links by default (`exp`/`nonce`/`sig`); pass `--legacy-v1`
only for backward compatibility with older consumers.
Signed join links enforce bounded expiration windows (`--expires-in-secs` max: 604800 / 7 days)
and bounded in-process nonce replay cache capacity (fail-closed when full).
`decode-join-link` validates signature/expiry fields without consuming replay nonces.
`room-chat` consumes join-link replay nonces only after local argument/default validation passes.

Then:

```bash
export KAIGI_PAY_IROHA_CONFIG=../iroha/demo/bob.toml
cargo run -p kaigi-cli -- room-chat \
  --join-link 'kaigi://join?...' \
  --display-name "Bob"
```

HDR note: `room-chat` / `ascii-live` auto-detect HDR display capability (macOS best-effort) unless
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
Reconnect note: the hub preserves host/co-host role intent by `participant_id` during temporary
disconnects; reconnecting reserved moderators can rejoin locked/waiting-room sessions without
manual readmit and receive updated permission snapshots.
Backpressure note: when participant outbound queues saturate, the hub emits moderator-visible
notice frames (`Error`) with rate-limited, aggregate dropped-fanout counts.
Scale soak note: run `bash scripts/run_scale004_soak.sh` to generate a `SCALE-004` evidence
report (`target/conformance/scale004-soak-report.json`) and raw test log.
Security/protocol smoke note: run `bash scripts/run_security_protocol_smoke.sh` to generate a
multi-scenario evidence report (`target/conformance/security-protocol-smoke-report.json`) and raw
test log.
Media/HDR/recording smoke note: run `bash scripts/run_media_hdr_recording_smoke.sh` to generate a
multi-scenario evidence report (`target/conformance/media-hdr-recording-smoke-report.json`) and
raw test log.
HDR transport smoke note: run `bash scripts/run_hdr_transport_smoke.sh` to generate a `HDR-003`
evidence report (`target/conformance/hdr-transport-smoke-report.json`) and raw test log.
M2 exit-criteria smoke note: run `bash scripts/run_m2_exit_criteria_smoke.sh` to generate
`HDR-004` / `HDR-005` evidence (`target/conformance/m2-exit-criteria-smoke-report.json`) and raw
test log.
Parity-status smoke note: run `bash scripts/run_parity_status_smoke.sh` to generate `PARITY-001`
evidence (`target/conformance/parity-status-smoke-report.json`) and raw test log.
Parity-readiness smoke note: run `bash scripts/run_parity_readiness_smoke.sh` to generate
`PARITY-002` evidence plus parity readiness detail
(`target/conformance/parity-readiness-report.json`).
M3 exit-criteria smoke note: run `bash scripts/run_m3_exit_criteria_smoke.sh` to generate
`PARITY-003` evidence (`target/conformance/m3-exit-criteria-smoke-report.json`).
Parity GA smoke note: run `bash scripts/run_parity_ga_smoke.sh` to generate
`PARITY-004` evidence (`target/conformance/parity-ga-smoke-report.json`).
Parity downgrade guard smoke note: run `bash scripts/run_parity_downgrade_guard_smoke.sh` to
generate `PARITY-005` evidence
(`target/conformance/parity-downgrade-guard-smoke-report.json`).
Parity waiver policy smoke note: run `bash scripts/run_parity_waiver_policy_smoke.sh` to
generate `PARITY-006` evidence
(`target/conformance/parity-waiver-policy-smoke-report.json`).
Parity waiver fixture manifest smoke note: run
`bash scripts/run_parity_waiver_fixture_manifest_smoke.sh` to generate `PARITY-008` evidence
(`target/conformance/parity-waiver-fixture-manifest-smoke-report.json`).
Parity waiver fixture coverage smoke note: run
`bash scripts/run_parity_waiver_fixture_coverage_smoke.sh` to generate `PARITY-009` evidence
(`target/conformance/parity-waiver-fixture-coverage-smoke-report.json`).
Parity waiver policy negative-fixture smoke note: run
`bash scripts/run_parity_waiver_policy_negative_smoke.sh` to generate `PARITY-007` evidence
(`target/conformance/parity-waiver-policy-negative-smoke-report.json`).
A/V baseline smoke note: run `bash scripts/run_av_baseline_smoke.sh` to generate a `MEDIA-004`
evidence report (`target/conformance/av-baseline-smoke-report.json`) and raw test log.
Screen-share constraints smoke note: run `bash scripts/run_screen_share_constraints_smoke.sh` to
generate a `MEDIA-003` evidence report
(`target/conformance/screen-share-constraints-smoke-report.json`) and raw test log.
Control-plane reliability smoke note: run `bash scripts/run_controlplane_reliability_smoke.sh`
to generate a multi-scenario evidence report
(`target/conformance/controlplane-reliability-smoke-report.json`) and raw test log.
Release playbook smoke note: run `bash scripts/run_release_playbook_smoke.sh` to generate
`OPS-001` / `OPS-002` evidence
(`target/conformance/release-playbook-smoke-report.json`) and raw test log.
Release readiness smoke note: run `bash scripts/run_release_readiness_smoke.sh` to generate
`OPS-003` / `OPS-004` evidence plus a readiness summary
(`target/conformance/release-readiness-report.json`).
Hardening/GA smoke note: run `bash scripts/run_hardening_ga_smoke.sh` to generate
`OPS-005` / `OPS-006` evidence
(`target/conformance/hardening-ga-smoke-report.json`).
Platform-contract smoke note: run `bash scripts/run_platform_contract_smoke.sh` to generate a
browser/native parity evidence report
(`target/conformance/platform-contract-smoke-report.json`) and raw test log.
Client-app-workspaces smoke note: run `bash scripts/run_client_app_workspaces_smoke.sh` to
generate `PLATFORM-007` evidence
(`target/conformance/client-app-workspaces-smoke-report.json`).
Client-release-tracks smoke note: run `bash scripts/run_client_release_tracks_smoke.sh` to
generate `PLATFORM-008` evidence
(`target/conformance/client-release-tracks-smoke-report.json`).
Client-release-playbook-alignment smoke note: run
`bash scripts/run_client_release_playbook_alignment_smoke.sh` to generate `PLATFORM-009`
evidence (`target/conformance/client-release-playbook-alignment-smoke-report.json`).
Client-fallback-drills smoke note: run `bash scripts/run_client_fallback_drills_smoke.sh` to
generate `PLATFORM-010` evidence
(`target/conformance/client-fallback-drills-smoke-report.json`).
Client-fallback-drill-results smoke note: run
`bash scripts/run_client_fallback_drill_results_smoke.sh` to generate `PLATFORM-011` evidence
(`target/conformance/client-fallback-drill-results-smoke-report.json`).
Client-release-manifest smoke note: run `bash scripts/run_client_release_manifest_smoke.sh` to
generate `PLATFORM-012` evidence
(`target/conformance/client-release-manifest-smoke-report.json`).
Client-release-readiness-gates smoke note: run
`bash scripts/run_client_release_readiness_gates_smoke.sh` to generate `PLATFORM-013` evidence
(`target/conformance/client-release-readiness-gates-smoke-report.json`).
Client-rollback-manifest smoke note: run
`bash scripts/run_client_rollback_manifest_smoke.sh` to generate `PLATFORM-014` evidence
(`target/conformance/client-rollback-manifest-smoke-report.json`).
Coverage note: run `bash scripts/run_conformance_coverage_check.sh` to verify every mandatory
scenario in `docs/test-plan.md` has passing evidence in generated reports
(`target/conformance/conformance-coverage-report.json` + `.log`).
Evidence index note: run `bash scripts/run_conformance_evidence_index.sh` to generate a markdown
scenario-to-evidence index (`target/conformance/conformance-evidence-index.md`).
Bundle note: run `bash scripts/run_conformance_evidence_bundle.sh` to execute all evidence
suites in one command and generate `target/conformance/conformance-evidence-bundle-report.json`
plus bundle log output (including coverage validation and evidence index generation).
Contract note: run `cargo run -p kaigi-cli -- platform-contract --pretty` to print the frozen
cross-platform/browser parity contract JSON (or scope with `--platform web-safari`, etc.).
Artifact note: run `bash scripts/export_platform_contract_json.sh` to refresh
`docs/platform-contract.json` from the current CLI/platform-contract implementation.
Sync note: run `python3 scripts/validate_ci_contract_sync.py` to ensure workflow scenario/platform
matrices remain aligned with frozen docs contracts.
Media profile note: run `python3 scripts/validate_media_capability_profiles.py` to validate
`docs/media-capability-profiles.json` against `docs/platform-contract.json`.
HDR transport note: run `python3 scripts/validate_hdr_transport_profiles.py` to validate
`docs/hdr-transport-profiles.json` against platform/media contracts.
HDR target-device note: run `python3 scripts/validate_hdr_target_device_results.py` to validate
`docs/hdr-target-device-results.json` covers passing HDR+SDR fallback runs on every platform.
A/V baseline note: run `python3 scripts/validate_av_baseline_profiles.py` to validate
`docs/av-baseline-profiles.json` against `docs/platform-contract.json`.
Platform blocker note: run `python3 scripts/validate_platform_blockers.py` to validate
`docs/platform-blockers.json` reports zero open platform blocks.
Client workspace note: run `python3 scripts/validate_client_app_workspaces.py` to validate
`docs/client-app-workspaces.json` against mandatory platform coverage and tracked workspace paths.
Client release-track note: run `python3 scripts/validate_client_release_tracks.py` to validate
`docs/client-release-tracks.json` against workspace coverage, fallback mapping, and signed native
release requirements.
Client release playbook note: run `python3 scripts/validate_client_release_playbook_alignment.py`
to validate deterministic release/rollback playbook tables against
`docs/client-release-tracks.json`.
Client fallback drill note: run `python3 scripts/validate_client_fallback_drills.py` to validate
`docs/client-fallback-drills.json` against workspace + release-track contracts and native fallback
recovery-time objectives.
Client fallback drill results note: run `python3 scripts/validate_client_fallback_drill_results.py`
to validate `docs/client-fallback-drill-results.json` against drill RTO limits and mandatory web
browser fallback coverage.
Client release manifest note: run `python3 scripts/validate_client_release_manifest.py` to
validate `docs/client-release-manifest.json` against release-track distribution/signing contracts
and artifact metadata integrity fields.
Client release readiness note: run `python3 scripts/validate_client_release_readiness_gates.py` to
validate `docs/client-release-readiness-gates.json` against release manifest + fallback drill
evidence alignment and bounded RTO outcomes.
Client rollback manifest note: run `python3 scripts/validate_client_rollback_manifest.py` to
validate `docs/client-rollback-manifest.json` against release-track/release-manifest contracts
and rollback artifact pointer integrity.
Parity status note: run `python3 scripts/validate_parity_status_contract.py` to validate
`docs/parity-status-contract.json` stays synchronized with `docs/parity-matrix.md`.
M3 exit note: run `python3 scripts/validate_m3_exit_criteria.py` to validate M3 exit readiness
from parity status + conformance coverage evidence.
Parity GA gate note: run `python3 scripts/validate_parity_ga_gate.py` to validate final GA parity
status across mandatory rows/platforms with passing coverage evidence.
Parity downgrade guard note: run `python3 scripts/validate_parity_downgrade_guard.py` to enforce
explicit non-expired waiver requirements for any GA status downgrade in
`docs/parity-status-waivers.json`.
Parity waiver policy note: run `python3 scripts/validate_parity_waiver_policy.py` to enforce
waiver quality constraints from `docs/parity-waiver-policy.json`.
Parity waiver fixture manifest note: run `python3 scripts/validate_parity_waiver_fixture_manifest.py`
to validate fixture corpus/manifest integrity for waiver-policy negative coverage.
Parity waiver fixture coverage note: run `python3 scripts/validate_parity_waiver_fixture_coverage.py`
to verify fixture corpus contains deterministic negative-path coverage for every waiver policy control.
Screen-share constraints note: run `python3 scripts/validate_screen_share_constraints.py` to
validate `docs/screen-share-constraints.json` against `docs/platform-contract.json`.
Playbook note: run `python3 scripts/validate_release_playbooks.py` to validate
`docs/release-playbook.md` and `docs/rollback-playbook.md` against mandatory platform coverage.
Hardening gate note: run `python3 scripts/validate_hardening_gates.py` to validate
`docs/hardening-gates.json` across mandatory platforms.
GA approval note: run `python3 scripts/validate_ga_approvals.py` to validate
`docs/ga-approvals.json` for single-train all-platform approval.
Critical-defect note: run `python3 scripts/validate_critical_defects.py` to validate
`docs/critical-defects.json` reports zero open critical defects.

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
- Anonymous mode uses opaque participant handles, X25519 key updates, encrypted control
  envelopes (`EncryptedControl`), hub-managed group key rotations (`AnonGroupKeyRotate`),
  and encrypted media envelopes (`AnonEncryptedPayload`).
  - Anonymous participant handles must be non-empty, ASCII, <= 128 chars, and must not include
    `@` or whitespace/control characters.
  - Hub enforces monotonic `GroupKeyUpdate.epoch` per sender to reject stale key replays.
  - Hub requires `EncryptedControl.epoch` to match the sender's current key epoch.
  - Hub rejects malformed key updates (`participant_handle` must be non-empty).
  - Hub bounds encrypted payload size (ciphertext <= 64KiB hex chars per recipient entry).
  - Hub rejects implausibly short encrypted ciphertext payloads (< 32 hex chars).
  - Hub caps encrypted fanout to 256 recipients per `EncryptedControl` frame.
  - Hub rejects encrypted recipient handles not present in the current anonymous roster.
  - Hub rejects plaintext media capability/track/video/audio frames in anonymous rooms.
  - Hub rotates and rebroadcasts anonymous media group key material when anonymous membership
    changes (join/leave).
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
