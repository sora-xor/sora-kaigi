# Protocol vNext Contract (Frozen)

This document defines the planned control-plane evolution for full meeting parity across native and web clients.

Status:

- Frozen on 2026-02-15 for M0 implementation.
- Post-freeze changes must be additive or require explicit protocol version negotiation.

## Goals

- Preserve decentralized operation over SoraNet/Nexus relay paths.
- Keep E2EE as default meeting mode.
- Add explicit role, policy, capability, and key-epoch semantics needed for parity.

## Versioning

- Current protocol remains accepted for existing harness clients.
- vNext introduces additive frame types first.
- Breaking wire changes require a new protocol version negotiation flag in the stream-open handshake.

## New Control Frames (Frozen)

All frames continue to use `u32(be) length + Norito payload`.

1. `RoleGrant`
- Fields: `target_participant_id`, `role`, `granted_by`, `signature`, `issued_at_ms`
- Purpose: assign `co_host` and bounded capabilities.

2. `RoleRevoke`
- Fields: `target_participant_id`, `role`, `revoked_by`, `signature`, `issued_at_ms`
- Purpose: remove role-based privileges.

3. `PermissionsSnapshot`
- Fields: `participant_id`, `effective_permissions`, `epoch`
- Purpose: deterministic local permission state.

4. `SessionPolicy`
- Fields: `room_lock`, `waiting_room_enabled`, `recording_policy`, `guest_policy`, `e2ee_required`, `max_participants`, `policy_epoch`, `updated_by`, `signature`
- Purpose: authoritative room behavior and host policy changes.

5. `DeviceCapability`
- Fields: `participant_id`, `codecs`, `hdr_capture`, `hdr_render`, `max_streams`, `updated_at_ms`
- Purpose: runtime capability negotiation.

6. `MediaProfileNegotiation`
- Fields: `participant_id`, `preferred_profile`, `negotiated_profile`, `color_primaries`, `transfer_fn`, `codec`, `epoch`
- Purpose: SDR/HDR selection and compatibility.

7. `RecordingNotice`
- Fields: `participant_id`, `state` (`started`/`stopped`), `mode` (`local`), `policy_basis`, `issued_at_ms`, `issued_by`
- Purpose: transparent recording consent and audit behavior.

8. `E2EEKeyEpoch`
- Fields: `participant_id`, `epoch`, `public_key`, `signature`
- Purpose: rotate and bind session keys to participant identity.

9. `KeyRotationAck`
- Fields: `participant_id`, `ack_epoch`, `received_at_ms`
- Purpose: detect lagging clients and stale decrypt paths.

10. `ParticipantPresenceDelta`
- Fields: `joined[]`, `left[]`, `role_changes[]`, `sequence`
- Purpose: scale-friendly roster updates at high participant counts.

11. Moderation action extensions
- New `ModerationAction` variants:
  - `AdmitFromWaiting`
  - `DenyFromWaiting`
- Purpose: enforce waiting-room admission policy with explicit host/co-host actions.

12. `ModerationSigned`
- Fields: `sent_at_ms`, `target`, `action`, `issued_by`, `signature`
- Purpose: signed moderation command envelope for host/co-host actions, while preserving legacy
  unsigned `Moderation` compatibility during transition.

## Join Link Schema (`kaigi://join?...`) (Frozen)

Required query parameters (all versions):

- `v`: link schema version (`1` legacy, `2` signed)
- `relay`: relay endpoint
- `channel`: kaigi channel id (32-byte hex)

Additional required query parameters for `v=2`:

- `exp`: expiration timestamp (unix ms)
- `nonce`: replay-resistant nonce (16-byte hex)
- `sig`: deterministic signature over canonicalized link payload (dev harness)

Optional query parameters:

- `torii`: Nexus/Torii endpoint
- `authenticated`: RouteOpen authenticated flag (`0`/`1`)
- `insecure`: TLS validation override (`0`/`1`, dev only)
- `sni`: TLS SNI override
- `pay_to`: billing destination account
- `kaigi_domain`: lifecycle domain
- `kaigi_call_name`: lifecycle call name
- `kaigi_privacy_mode`: privacy hint (`transparent`/`zk`)

Validation rules:

- Reject expired links.
- Reject links whose `exp` is beyond the maximum allowed future window.
- Reject malformed `v=2` canonical payload signatures.
- Reject replay beyond allowed nonce/epoch window.
- Enforce bounded replay-nonce cache capacity (fail closed when full).
- Non-join inspection/diagnostic flows validate link security without consuming replay nonces.
- Keep `v=1` parsing for backward compatibility during migration.

## Frozen Role/Permission Model

- `host`: authoritative role assignment, policy mutation, and meeting-end control.
- `co_host`: delegated moderation/policy controls as explicitly granted by host.
- `participant`: baseline meeting participation without policy mutation privileges.
- Role mutations are represented by signed `RoleGrant` / `RoleRevoke` actions and reflected in
  `PermissionsSnapshot` and `ParticipantPresenceDelta.role_changes`.

## Security Invariants

- E2EE is enabled by default for all meeting sessions.
- When `SessionPolicy.e2ee_required=true`, plaintext control operations are rejected until the
  sender has published `E2EEKeyEpoch(epoch>=1)` for the current session.
- Role and policy mutations must be signed by authorized actor keys.
- Clients reject out-of-epoch key material and stale action signatures.
- Hub rejects replay/stale action frames via per-signer monotonic timestamp checks on
  `sent_at_ms` (`Moderation`, `E2EEKeyEpoch`) and `issued_at_ms`/`updated_at_ms`
  (`RoleGrant`, `RoleRevoke`, `SessionPolicy`).
- Participant identities are unique per room; duplicate claims and post-join identity mutation are rejected.
- Guest users cannot self-escalate roles.
- `RecordingNotice(state=started)` is rejected when session policy disables local recording.
- `KeyRotationAck.ack_epoch` must not exceed the sender's latest accepted `E2EEKeyEpoch`.
- Dev harness note: current implementation validates deterministic frame signatures for
  moderation/role/policy/E2EE mutation frames; production rollout should replace this with
  wallet-key cryptographic verification bound to Nexus identities.

## Relay/Hub Behavioral Contract

- Enforce monotonic policy and key epochs.
- Broadcast role/policy snapshots on join and on epoch changes.
- Rebroadcasted `SessionPolicy` frames preserve the signed `updated_at_ms` and signature payload.
- Rebroadcast accepted `RoleGrant`/`RoleRevoke` frames for role-change audit visibility.
- Keep deterministic ordering for moderation actions.
- Rebroadcast accepted moderation action frames (`Moderation` and `ModerationSigned`) to room
  participants for moderation audit visibility.
- Keep waiting-room participants in pending state until `AdmitFromWaiting`; `DenyFromWaiting`
  disconnects the pending participant with an explicit denial error frame.
- Enforce `SessionPolicy.e2ee_required=true` by default for newly created rooms.
- For `MediaProfileNegotiation`, preserve HDR only when sender capability includes
  `hdr_capture=true` and at least one joined remote participant reports `hdr_render=true`;
  otherwise coerce `negotiated_profile` to `Sdr` and broadcast without failing the session.
- Enforce `max_screen_shares` on `ParticipantState.screen_share_enabled=true`; when capacity is
  exhausted, keep sender share state disabled and return a deterministic denial error.
- `ModerationAction::Kick` sends an explicit target error frame before close signaling in the dev
  harness.
- Preserve role continuity across reconnects by restoring reserved host/co-host role intent when a
  participant rejoins with the same `participant_id`, including room-lock/waiting-room bypass for
  reserved moderators.
- Provide bounded fanout; when participant outbound queues saturate, emit moderator-visible
  backpressure notices with rate-limited aggregate dropped counts.

## Backward Compatibility

- Accept legacy control frames during transition.
- Emit vNext-only frames behind negotiated capability until all clients upgrade.
- Maintain clear protocol error codes for unsupported frame handling.

## Change Control (Post-Freeze)

- Additive frame fields/variants must preserve existing decode behavior and include conformance
  coverage in `docs/test-plan.md`.
- Breaking behavior requires explicit protocol version negotiation and a documented migration path.
- Join-link signed (`v=2`) schema is frozen for required fields (`exp`, `nonce`, `sig`); adding
  required fields requires a new link schema version.

## Implementation Mapping

1. `crates/kaigi-wire`
- Add new frame enums, serialization, and roundtrip tests.

2. `crates/kaigi-hub-echo`
- Enforce role/policy/key invariants and epoch checks.

3. `crates/kaigi-cli`
- Add command-level controls and debug prints for new frame classes.

4. Client apps (future repos/workspaces)
- Implement capability negotiation and policy-driven UX gates.

5. `crates/kaigi-platform-contract`
- Export frozen browser/native parity contract targets (including web fallback requirements) for
  downstream client implementations and conformance tooling.

6. `docs/media-capability-profiles.json`
- Define frozen per-platform media/HDR capture-render profile constraints and SDR fallback
  requirements that map to `DeviceCapability` and `MediaProfileNegotiation` behavior.

7. `docs/screen-share-constraints.json`
- Define frozen per-platform screen-share capture/audio limitations that map to
  `ParticipantState.share_enabled` behavior and `max_screen_shares` policy enforcement.

8. `docs/av-baseline-profiles.json`
- Define frozen per-platform camera/mic/speaker baseline and default join-state invariants that map
  to `ParticipantState` media toggles and `MEDIA-001`/`MEDIA-004` conformance gates.

9. `docs/hdr-transport-profiles.json`
- Define frozen per-platform HDR transport metadata and deterministic SDR fallback/tone-mapping
  requirements that map to `MediaProfileNegotiation` behavior and `HDR-001`/`HDR-002`/`HDR-003`
  conformance gates.

10. `docs/hdr-target-device-results.json`
- Define frozen per-platform target-device pass evidence for HDR and SDR fallback paths that map to
  `HDR-004` conformance gate requirements.

11. `docs/platform-blockers.json`
- Define frozen per-platform blocker ledger used by `HDR-005` to assert no open platform blocks
  remain for core meeting operations.
