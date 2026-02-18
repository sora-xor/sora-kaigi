# Conformance Test Plan

This test plan defines mandatory scenarios for full parity release of `sora-kaigi`.

Status:

- Frozen on 2026-02-15 for M0 implementation.
- New mandatory scenarios must be reflected in CI scenario contract checks.

## Evidence Rules

- Every scenario must produce a pass/fail record.
- Platform-specific failures must include a root-cause note and mitigation owner.
- A capability can move from `P` to `A/B/GA` in `docs/parity-matrix.md` only with linked evidence.

## Suite A: Protocol Conformance

1. `P-CONF-001` Frame roundtrip
- Encode/decode all mandatory frame variants with canonical payload examples.

2. `P-CONF-002` Malformed payload rejection
- Reject truncated frames, invalid lengths, invalid hex fields, and stale epochs.
- Reject client-injected hub-managed frames (`PermissionsSnapshot`, `ParticipantPresenceDelta`).

3. `P-CONF-003` Backward compatibility
- Legacy clients interoperate with additive vNext frames where negotiation permits.

4. `P-CONF-004` Replay resistance
- Reject replayed join links and stale signed moderation/session actions.
- Reject join links with `exp` beyond max future window and reject nonce-cache overflow.
- For action frames (`Moderation`, `ModerationSigned`, `RoleGrant`, `RoleRevoke`,
  `SessionPolicy`, `E2EEKeyEpoch`), reject non-monotonic signer timestamps.
- For `KeyRotationAck`, reject stale/replayed `ack_epoch` and non-monotonic
  `received_at_ms` per participant.

## Suite B: Security and Identity

1. `SEC-001` E2EE default enforcement
- Meeting cannot start in plaintext mode.
- With default `SessionPolicy.e2ee_required=true`, plaintext chat/state operations are rejected
  until the sender publishes `E2EEKeyEpoch(epoch>=1)`.

2. `SEC-002` Key rotation health
- Rotate key epochs during active sessions without media/control loss.
- `KeyRotationAck.ack_epoch` must not exceed sender epoch, must increase per participant, and
  valid acks are broadcast.
- `KeyRotationAck.received_at_ms` must be monotonic per participant (replay/stale timestamps are rejected).

3. `SEC-003` Guest escalation prevention
- Guest identity cannot grant or obtain host/co-host privileges.
- Duplicate participant identities and post-join identity mutation are rejected.

4. `SEC-004` Signature validation
- Invalid signer keys or tampered payload signatures are rejected.
- Includes `RoleGrant`, `RoleRevoke`, `SessionPolicy`, and `E2EEKeyEpoch` tamper cases with
  sender-only error signaling and no room-wide mutation/broadcast.

## Suite C: Moderation and Roles

1. `MOD-001` Host/co-host assignment
- Host can grant and revoke co-host role; participant cannot self-grant.
- Accepted role grant/revoke actions are visible to participants as role audit frames.
- Accepted role grant/revoke actions also emit `ParticipantPresenceDelta.role_changes`
  entries for deterministic role-state fanout.

2. `MOD-002` Moderation command parity
- Mute/video-off/share-off/kick behavior is consistent across all clients.
- `DisableScreenShare` mutates target state and emits room-visible state updates.
- `Kick` emits deterministic target error + disconnect signaling.

3. `MOD-003` Room lock and waiting room
- Admission behavior follows signed session policy.
- Locked rooms reject non-host/non-cohost `Hello` joins without mutating presence.

4. `MOD-004` Audit visibility
- Moderation actions produce visible, ordered audit events.
- Accepted moderation actions are rebroadcast as moderation audit frames (`Moderation` or
  `ModerationSigned`).

5. `MOD-005` Waiting-room admit/deny flow
- Host/co-host can admit or deny pending participants; denied participants are disconnected.
- Admit/deny actions targeting pending participants are auditable and deterministic.
- Admit emits joined `ParticipantPresenceDelta` entries; deny does not emit join/leave deltas for
  never-admitted pending participants.

6. `MOD-006` Guest policy enforcement
- When guest policy is disabled, non-account guest IDs cannot join.
- Rejected guest joins do not mutate room presence state.

## Suite D: Media and HDR

Media profile contract note:

- Frozen per-platform capture/render fallback profiles are defined in
  `docs/media-capability-profiles.json` and must stay aligned with
  `docs/platform-contract.json`.

1. `MEDIA-001` Baseline A/V join
- Camera/mic/speaker controls work across all platforms.
- Join defaults force mic/video/screen-share OFF on `Hello`, and post-join participant-state
  updates can enable mic/video deterministically when policy permits.

2. `MEDIA-002` Screen share parity
- Start/stop and remote view behavior is consistent per policy.
- `max_screen_shares` limits are enforced with deterministic denial errors and retry success once
  a share slot is released.

3. `MEDIA-003` Screen-share constraints contract
- Per-platform screen-share source/audio limitations are frozen in
  `docs/screen-share-constraints.json`.
- Constraints stay aligned with `docs/platform-contract.json` platform coverage and include
  deterministic local concurrency and consent requirements.

4. `MEDIA-004` A/V baseline contract
- Per-platform camera/mic/speaker baseline profiles are frozen in
  `docs/av-baseline-profiles.json`.
- Profiles define permission model, capture/playback API baseline, and default join state
  invariants (mic/video/share OFF on join) across all target platforms.

5. `HDR-001` HDR negotiation
- HDR is selected only when both sender and receiver support it.
- Hub preserves negotiated HDR only with `hdr_capture=true` (sender) and at least one joined
  remote `hdr_render=true`; otherwise it falls back to SDR.

6. `HDR-002` SDR fallback
- Unsupported clients receive tone-mapped SDR output without session failure.
- Unsupported HDR negotiations are coerced to `negotiated_profile=Sdr` and still broadcast.

7. `HDR-003` HDR transport metadata contract
- Per-platform HDR transport metadata profiles are frozen in
  `docs/hdr-transport-profiles.json`.
- Profiles stay aligned with `docs/media-capability-profiles.json` and define deterministic
  SDR fallback profile + tone-mapping strategy requirements.

8. `HDR-004` HDR target-device pass evidence
- Per-platform target-device HDR and SDR-fallback run results are frozen in
  `docs/hdr-target-device-results.json`.
- Every platform must have at least one passing HDR path case and one passing SDR fallback case.

9. `HDR-005` Platform block ledger clear
- Core meeting platform blocker ledger is frozen in `docs/platform-blockers.json`.
- Release gate requires zero open platform blocks across all mandatory platforms.

## Suite E: Recording

1. `REC-001` Host recording policy enforcement
- Host policy controls participant recording rights.
- When local recording policy is disabled, `RecordingNotice(state=started)` is rejected.

2. `REC-002` Local recording flow
- Start/stop local recording succeeds with visible notice.
- `RecordingNotice(state=stopped)` is broadcast to participants when issued by sender.

3. `REC-003` Unauthorized record attempt
- Policy-blocked participants cannot start recording.
- Rejected start attempts do not produce broadcast recording notices.

## Suite F: Scale and Reliability

1. `SCALE-001` 500 participant roster stability
- Join/leave churn does not corrupt roster state.
- Roster snapshots remain deterministic (sorted, unique participant IDs) after churn.
- `ParticipantPresenceDelta.sequence` remains monotonic across join/role-change/leave churn.

2. `SCALE-002` Reconnect resilience
- Temporary disconnect/rejoin preserves expected role and policy.
- Rejoining participants recover role intent by `participant_id` (host/co-host) and receive
  current `PermissionsSnapshot` state.
- Reserved host/co-host reconnects bypass room-lock/waiting-room join gates.

3. `SCALE-003` Congestion/backpressure behavior
- Meeting remains usable under adverse network conditions.
- Fanout drops caused by saturated participant queues emit moderator-visible backpressure notices.
- Backpressure notices are rate-limited and include aggregate dropped-fanout totals.

4. `SCALE-004` Long-duration stability
- 2-hour meeting run without fatal protocol/media regressions.
- Dev-harness accelerated soak should execute equivalent control-plane ticks (chat/state/media,
  key rotation, recording notices) without invariant drift.
- CI/local evidence command: `bash scripts/run_scale004_soak.sh` (writes JSON + log under
  `target/conformance/`).

## Suite G: Platform Contract and Fallback

1. `PLATFORM-001` Mandatory platform coverage
- Contract includes Web Chromium, Web Safari, Web Firefox, macOS, iOS, iPadOS, Windows, Android, and Linux.

2. `PLATFORM-002` Native web fallback enforcement
- Every native platform contract requires browser fallback support coverage.

3. `PLATFORM-003` Security baseline parity
- Every platform contract enforces E2EE default, signed high-risk actions, and replay resistance.

4. `PLATFORM-004` HDR + SDR fallback parity
- Every platform contract defines HDR-on-supported-devices and deterministic SDR tone-map fallback.

5. `PLATFORM-005` Full feature parity target contract
- Every platform contract includes moderation, waiting room/room lock, chat/reactions/hand raise,
  local recording policy, reconnect continuity, accessibility, and 500-participant scale target.

6. `PLATFORM-006` Windows native parity requirement
- Windows contract is explicitly marked native and requires browser fallback support.

7. `PLATFORM-007` Client app workspace contract
- Machine-readable client workspace contract (`docs/client-app-workspaces.json`) must define
  implementation tracks for Web Chromium/Safari/Firefox and native macOS/iOS/iPadOS/Windows/
  Android/Linux.
- Every workspace path must exist with a track README, and native tracks must reference the web
  fallback workspace contract.

8. `PLATFORM-008` Client release-track contract
- Machine-readable client release-track contract (`docs/client-release-tracks.json`) must define
  deterministic CI build/smoke commands, release channels, artifact kind, and distribution
  channel for every workspace in `docs/client-app-workspaces.json`.
- Native tracks must require signed release artifacts and explicitly map fallback to the web
  workspace; the web track must ship as an IPFS bundle with HDR validation enabled.

9. `PLATFORM-009` Client playbook/contract alignment
- Release and rollback playbooks must include deterministic per-workspace contract tables aligned
  with `docs/client-release-tracks.json` (workspace ID, platform coverage, distribution channel,
  and signing/fallback invariants).
- Every workspace in the release-track contract must appear exactly once in both playbook tables.

10. `PLATFORM-010` Native-to-web fallback drill contract
- Machine-readable fallback drill contract (`docs/client-fallback-drills.json`) must define one
  deterministic fallback drill entry for each native workspace in
  `docs/client-app-workspaces.json`.
- Every drill entry must align with release-track distribution channels and enforce a bounded
  recovery-time objective for switching users to the web fallback workspace.

11. `PLATFORM-011` Native-to-web fallback drill results contract
- Machine-readable fallback drill results contract (`docs/client-fallback-drill-results.json`)
  must report one passing run for each native workspace in
  `docs/client-fallback-drills.json`.
- Observed fallback recovery-time values must not exceed configured drill RTO limits, and result
  coverage must include all mandatory web browser platforms (Web Chromium, Web Safari,
  Web Firefox).

12. `PLATFORM-012` Client release manifest contract
- Machine-readable release manifest (`docs/client-release-manifest.json`) must contain deterministic
  artifact metadata for every workspace in `docs/client-release-tracks.json` (artifact URI,
  checksum, signature reference, SBOM reference, provenance reference, and publish timestamp).
- Manifest distribution channels and signing-verification requirements must align with
  `docs/client-release-tracks.json`; web artifact must be IPFS-addressed.

13. `PLATFORM-013` Client release readiness gates contract
- Machine-readable release readiness gates (`docs/client-release-readiness-gates.json`) must bind
  every workspace release-ready decision to deterministic evidence references.
- Native workspace gate rows must align with fallback-drill contract/result RTO values and pass
  state; web workspace gate row must explicitly mark fallback drill as `n/a`.

14. `PLATFORM-014` Client rollback manifest contract
- Machine-readable rollback manifest (`docs/client-rollback-manifest.json`) must define one
  deterministic rollback artifact pointer for every workspace in
  `docs/client-release-tracks.json`.
- Rollback artifact metadata must align with release-track distribution/signing contracts and
  reference rollback artifacts distinct from current release-manifest artifact URIs.

## Suite H: Release Operations

1. `OPS-001` Release playbook completeness
- Release playbook documents native platform tracks (macOS, iOS, iPadOS, Windows, Android, Linux)
  and IPFS-hosted web release flow (Web Chromium, Web Safari, Web Firefox).
- Release playbook includes build/signing, launch checklist, and post-release verification stages.

2. `OPS-002` Rollback playbook completeness
- Rollback playbook documents native rollback tracks (macOS, iOS, iPadOS, Windows, Android, Linux)
  and IPFS-hosted web rollback flow (Web Chromium, Web Safari, Web Firefox).
- Rollback playbook includes trigger conditions, incident communication, and rollback exit criteria.

3. `OPS-003` Zero critical-defect ledger
- `docs/critical-defects.json` exists as a machine-readable critical defect ledger artifact.
- Open critical defect set is empty for release gate evidence generation.

4. `OPS-004` Release readiness report generation
- A deterministic readiness report is generated from scenario evidence and defect ledger state.
- Report includes alpha/beta/reliability/platform/ops gate statuses and GA readiness summary fields.

5. `OPS-005` Platform hardening gates
- Machine-readable platform hardening gates (`docs/hardening-gates.json`) report passed
  performance, reliability, and security gates for every mandatory platform.

6. `OPS-006` Single-train GA approval
- Machine-readable GA approval contract (`docs/ga-approvals.json`) confirms all mandatory
  platforms are approved in the same release train.

## Suite I: Parity Status Governance

1. `PARITY-001` Parity status contract sync
- Machine-readable parity status contract (`docs/parity-status-contract.json`) is synchronized with
  `docs/parity-matrix.md` capabilities, platforms, and status values.

2. `PARITY-002` Parity readiness report generation
- Parity readiness report is generated from parity status contract plus conformance coverage.
- Report includes capability-level below-beta inventory and M3 exit readiness summary fields.

3. `PARITY-003` M3 exit criteria gate
- M3 exit gate passes only when parity status is at least Beta on every mandatory matrix row and
  mandatory conformance scenario coverage is passing.

4. `PARITY-004` GA parity gate
- GA parity gate passes only when parity status is `GA` on every mandatory matrix row and
  mandatory conformance scenario coverage is passing.

5. `PARITY-005` Parity downgrade waiver guard
- Downgrades from `GA` to `B/A/P` require explicit, non-expired waivers in
  `docs/parity-status-waivers.json`.
- Guard fails for unwaived downgrades and stale/orphaned waiver entries.

6. `PARITY-006` Parity waiver policy compliance
- Waiver entries must satisfy policy constraints defined in `docs/parity-waiver-policy.json`
  (minimum reason quality, max TTL, owner/approver format, and ticket format constraints).

7. `PARITY-007` Parity waiver policy negative fixtures
- Negative fixture suite asserts that invalid waiver entries are rejected for each policy control
  (reason length, max TTL, owner format, approver format, ticket format, target-status allowlist).

8. `PARITY-008` Parity waiver fixture manifest integrity
- Waiver fixture manifest (`docs/fixtures/waivers/manifest.json`) must remain schema-valid with
  deterministic fixture metadata (IDs, expected outcome, and failure matcher constraints).
- Every manifest entry must resolve to a schema-valid fixture file under
  `docs/fixtures/waivers/` with non-expired deterministic tokenization rules.
- Fixture list must be deterministic (sorted IDs), filename-mapped (`snake_case` ID to
  `kebab-case` JSON filename), one-waiver-per-fixture, and exhaustive with no orphan fixture files
  outside the manifest.

9. `PARITY-009` Parity waiver policy control coverage
- Waiver fixture corpus must include deterministic negative coverage for every enforced waiver
  policy control (reason min/max, TTL max, status allowlist, owner/approver/ticket format, and
  distinct owner/approver requirement).
- Coverage validation uses fixture `expect_error_contains` matchers as a contract between policy
  validator behavior and fixture corpus intent.

## Evidence Automation Helpers

- `bash scripts/run_scale004_soak.sh`
  - Produces `target/conformance/scale004-soak-report.json` and `scale004-soak.log`.
- `bash scripts/run_security_protocol_smoke.sh`
  - Produces `target/conformance/security-protocol-smoke-report.json` and
    `security-protocol-smoke.log`.
  - Runs targeted evidence cases for `P-CONF-001`, `P-CONF-002`, `P-CONF-003`, `SEC-002`, `SEC-003`,
    `SEC-004`, `MOD-001`, `MOD-003`, `MOD-004`, `MOD-005`, `MOD-006`, `MEDIA-001`,
    and `SCALE-001`.
- `bash scripts/run_media_hdr_recording_smoke.sh`
  - Produces `target/conformance/media-hdr-recording-smoke-report.json` and
    `media-hdr-recording-smoke.log`.
  - Runs targeted evidence cases for `MOD-002`, `MEDIA-002`, `HDR-001`, `HDR-002`,
    `REC-001`, `REC-002`, and `REC-003`.
- `bash scripts/run_hdr_transport_smoke.sh`
  - Produces `target/conformance/hdr-transport-smoke-report.json` and
    `hdr-transport-smoke.log`.
  - Runs targeted evidence case for `HDR-003`.
- `bash scripts/run_m2_exit_criteria_smoke.sh`
  - Produces `target/conformance/m2-exit-criteria-smoke-report.json` and
    `m2-exit-criteria-smoke.log`.
  - Runs targeted evidence cases for `HDR-004` and `HDR-005`.
- `bash scripts/run_parity_status_smoke.sh`
  - Produces `target/conformance/parity-status-smoke-report.json` and
    `parity-status-smoke.log`.
  - Runs targeted evidence case for `PARITY-001`.
- `bash scripts/run_parity_readiness_smoke.sh`
  - Produces `target/conformance/parity-readiness-smoke-report.json`,
    `parity-readiness-report.json`, and `parity-readiness.log`.
  - Runs targeted evidence case for `PARITY-002`.
- `bash scripts/run_m3_exit_criteria_smoke.sh`
  - Produces `target/conformance/m3-exit-criteria-smoke-report.json` and
    `m3-exit-criteria-smoke.log`.
  - Runs targeted evidence case for `PARITY-003`.
- `bash scripts/run_parity_ga_smoke.sh`
  - Produces `target/conformance/parity-ga-smoke-report.json` and
    `parity-ga-smoke.log`.
  - Runs targeted evidence case for `PARITY-004`.
- `bash scripts/run_parity_downgrade_guard_smoke.sh`
  - Produces `target/conformance/parity-downgrade-guard-smoke-report.json` and
    `parity-downgrade-guard-smoke.log`.
  - Runs targeted evidence case for `PARITY-005`.
- `bash scripts/run_parity_waiver_policy_smoke.sh`
  - Produces `target/conformance/parity-waiver-policy-smoke-report.json` and
    `parity-waiver-policy-smoke.log`.
  - Runs targeted evidence case for `PARITY-006`.
- `bash scripts/run_parity_waiver_policy_negative_smoke.sh`
  - Produces `target/conformance/parity-waiver-policy-negative-smoke-report.json` and
    `parity-waiver-policy-negative-smoke.log`.
  - Runs targeted evidence case for `PARITY-007`.
- `bash scripts/run_parity_waiver_fixture_manifest_smoke.sh`
  - Produces `target/conformance/parity-waiver-fixture-manifest-smoke-report.json` and
    `parity-waiver-fixture-manifest-smoke.log`.
  - Runs targeted evidence case for `PARITY-008`.
- `bash scripts/run_parity_waiver_fixture_coverage_smoke.sh`
  - Produces `target/conformance/parity-waiver-fixture-coverage-smoke-report.json` and
    `parity-waiver-fixture-coverage-smoke.log`.
  - Runs targeted evidence case for `PARITY-009`.
- `bash scripts/run_av_baseline_smoke.sh`
  - Produces `target/conformance/av-baseline-smoke-report.json` and
    `av-baseline-smoke.log`.
  - Runs targeted evidence case for `MEDIA-004`.
- `bash scripts/run_screen_share_constraints_smoke.sh`
  - Produces `target/conformance/screen-share-constraints-smoke-report.json` and
    `screen-share-constraints-smoke.log`.
  - Runs targeted evidence case for `MEDIA-003`.
- `bash scripts/run_controlplane_reliability_smoke.sh`
  - Produces `target/conformance/controlplane-reliability-smoke-report.json` and
    `controlplane-reliability-smoke.log`.
  - Runs targeted evidence cases for `P-CONF-004`, `SEC-001`, `SCALE-002`, and `SCALE-003`.
- `bash scripts/run_release_playbook_smoke.sh`
  - Produces `target/conformance/release-playbook-smoke-report.json` and
    `release-playbook-smoke.log`.
  - Runs targeted evidence cases for `OPS-001` and `OPS-002`.
- `bash scripts/run_release_readiness_smoke.sh`
  - Produces `target/conformance/release-readiness-smoke-report.json`,
    `release-readiness-report.json`, and `release-readiness.log`.
  - Runs targeted evidence cases for `OPS-003` and `OPS-004`.
- `bash scripts/run_hardening_ga_smoke.sh`
  - Produces `target/conformance/hardening-ga-smoke-report.json` and
    `hardening-ga-smoke.log`.
  - Runs targeted evidence cases for `OPS-005` and `OPS-006`.
- `bash scripts/run_client_app_workspaces_smoke.sh`
  - Produces `target/conformance/client-app-workspaces-smoke-report.json` and
    `client-app-workspaces-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-007`.
- `bash scripts/run_client_release_tracks_smoke.sh`
  - Produces `target/conformance/client-release-tracks-smoke-report.json` and
    `client-release-tracks-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-008`.
- `bash scripts/run_client_release_playbook_alignment_smoke.sh`
  - Produces `target/conformance/client-release-playbook-alignment-smoke-report.json` and
    `client-release-playbook-alignment-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-009`.
- `bash scripts/run_client_fallback_drills_smoke.sh`
  - Produces `target/conformance/client-fallback-drills-smoke-report.json` and
    `client-fallback-drills-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-010`.
- `bash scripts/run_client_fallback_drill_results_smoke.sh`
  - Produces `target/conformance/client-fallback-drill-results-smoke-report.json` and
    `client-fallback-drill-results-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-011`.
- `bash scripts/run_client_release_manifest_smoke.sh`
  - Produces `target/conformance/client-release-manifest-smoke-report.json` and
    `client-release-manifest-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-012`.
- `bash scripts/run_client_release_readiness_gates_smoke.sh`
  - Produces `target/conformance/client-release-readiness-gates-smoke-report.json` and
    `client-release-readiness-gates-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-013`.
- `bash scripts/run_client_rollback_manifest_smoke.sh`
  - Produces `target/conformance/client-rollback-manifest-smoke-report.json` and
    `client-rollback-manifest-smoke.log`.
  - Runs targeted evidence case for `PLATFORM-014`.
- `bash scripts/run_conformance_evidence_bundle.sh`
  - Produces `target/conformance/conformance-evidence-bundle-report.json` and
    `conformance-evidence-bundle.log`.
  - Runs all evidence helpers (`SCALE-004`, security/protocol smoke, media/HDR/recording smoke,
    HDR transport smoke, M2 exit-criteria smoke, A/V baseline smoke, parity-status smoke,
    screen-share constraints smoke, control-plane reliability smoke, release-playbook smoke,
    hardening/GA smoke, parity-readiness smoke, M3 exit-criteria smoke,
    parity GA smoke, parity downgrade guard smoke, parity waiver policy smoke,
    parity waiver policy negative-fixture smoke,
    release-readiness smoke, platform-contract smoke, client-app-workspaces smoke,
    client-release-tracks smoke, client-release-playbook-alignment smoke,
    client-fallback-drills smoke, client-fallback-drill-results smoke,
    client-release-manifest smoke, client-release-readiness-gates smoke,
    client-rollback-manifest smoke,
    conformance coverage check, and conformance evidence index generation) in one command.
- `bash scripts/run_platform_contract_smoke.sh`
  - Produces `target/conformance/platform-contract-smoke-report.json` and
    `platform-contract-smoke.log`.
  - Runs targeted evidence cases for `PLATFORM-001` through `PLATFORM-006`.
- `bash scripts/run_conformance_coverage_check.sh`
  - Produces `target/conformance/conformance-coverage-report.json` and
    `conformance-coverage.log`.
  - Validates that every mandatory scenario ID in this test plan has at least one passing
    evidence record in generated report artifacts.
- `bash scripts/run_conformance_evidence_index.sh`
  - Produces `target/conformance/conformance-evidence-index-report.json`,
    `conformance-evidence-index.md`, and `conformance-evidence-index.log`.
  - Generates a scenario-to-evidence index markdown from coverage and bundle reports.

## Mandatory Platform Runs

Run all suites for:

- Web Chromium (stable)
- Web Safari (stable)
- Web Firefox (stable)
- macOS
- iOS
- iPadOS
- Windows
- Android
- Linux

## Release Gates

- Alpha gate:
- All Protocol and Security suites pass on every platform.
- Beta gate:
- Moderation, Media, and Recording suites pass on every platform.
- GA gate:
- All suites pass, including scale/reliability scenarios, with no unresolved critical defects.
- Parity status is `GA` across all mandatory capability rows and platforms.
