# sora-kaigi Roadmap

Last updated: 2026-02-16

## Vision

Build a decentralized, Zoom-style conferencing product on Sora Nexus/SoraNet with full feature parity across:

- Native: macOS, iOS, iPadOS, Windows, Android, Linux
- Web fallback: IPFS-hosted browser app (Chromium, Safari, Firefox stable)

This roadmap is for meetings only. Webinar functionality is out of scope and will be built as a separate product (`taikai`).

## Product Constraints (Locked)

- No centralized backend services.
- Clients connect directly to Sora Nexus/SoraNet nodes and relay infrastructure.
- End-to-end encryption (E2EE) is default for meetings.
- Role model is mandatory: host, co-host, participant.
- Local recording is required in v1 for host and participants (policy-controlled).
- Browser support target is current stable Chromium, Safari, and Firefox.
- HDR capture+render is required on supported hardware/software paths; SDR fallback is required everywhere else.
- Release strategy is single big-bang GA after all parity gates pass.
- Scale target is 500 interactive participants per meeting.

## Current Baseline

The repository currently provides the networking/control-plane spine:

- `kaigi-soranet-client` QUIC + relay handshake
- `kaigi-wire` framed control-plane protocol
- `kaigi-hub-echo` hub adapter harness
- `kaigi-cli` lifecycle/payment/moderation harness

These components remain the integration foundation for parity workstreams.

## Progress Snapshot (Current Repo)

- [x] Roadmap/spec artifacts added (`roadmap.md`, parity matrix, protocol vNext, test plan, CI contract workflow)
- [x] `kaigi-wire` vNext frame types added with roundtrip tests
- [x] Hub role and policy control path added (`RoleGrant`, `RoleRevoke`, `SessionPolicy`, `PermissionsSnapshot`)
- [x] Hub moderation permissions expanded to host/co-host
- [x] Hub policy enforcement added for room lock, waiting-room admit/deny, guest policy, and max participants on join
- [x] Hub handling added for capability/media/recording/E2EE frames with validation gates
- [x] Hub now rejects client-injected hub-managed state frames (`PermissionsSnapshot`, `ParticipantPresenceDelta`) with sender-only errors
- [x] Hub verifies vNext frame signatures using deterministic dev-harness signature checks
- [x] Signed moderation flow added via `ModerationSigned` frame with hub signature verification and CLI signed-emission path
- [x] Hub now rebroadcasts accepted moderation action frames (`Moderation` and `ModerationSigned`) for room-visible audit visibility, with end-to-end handler tests for success/issuer mismatch/bad signature/replay
- [x] Hub now rebroadcasts accepted `RoleGrant`/`RoleRevoke` frames for role-change audit visibility, with end-to-end handler tests
- [x] Hub role mutation fanout now has end-to-end handler coverage for `ParticipantPresenceDelta.role_changes` entries on accepted `RoleGrant`/`RoleRevoke` flows
- [x] Hub enforces per-signer monotonic action timestamps for replay/stale rejection (`Moderation`, `RoleGrant`, `RoleRevoke`, `SessionPolicy`, `E2EEKeyEpoch`)
- [x] Hub replay/stale rejection now has end-to-end handler tests for `RoleGrant`, `RoleRevoke`, `SessionPolicy`, and `E2EEKeyEpoch` paths
- [x] Hub signature validation now has end-to-end handler coverage for tampered `RoleGrant`, `RoleRevoke`, `SessionPolicy`, and `E2EEKeyEpoch` frames (`SEC-004`)
- [x] Hub now preserves signed `SessionPolicy.updated_at_ms` in room state and rebroadcasted policy frames (prevents signature drift)
- [x] Hub join policy now rejects duplicate `participant_id` claims and rejects post-join `participant_id` mutation on re-hello
- [x] Hub waiting-room admit/deny moderation flow now has end-to-end handler tests, including pending-admission notifications and deny disconnect behavior
- [x] Waiting-room admit/deny flow now has end-to-end handler coverage for presence-delta behavior (admit emits joined delta; deny emits no join/leave delta for pending-only participant)
- [x] Hub guest policy enforcement now has end-to-end handler coverage for guest join rejection when guest access is disabled
- [x] Hub room-lock policy now has end-to-end handler coverage for locked-room join rejection
- [x] Hub recording policy enforcement now has end-to-end handler tests for allowed local recording notices and policy-blocked start attempts
- [x] Hub key-rotation acknowledgment flow now has end-to-end handler tests for valid broadcasts, invalid ack-epoch rejection, replay/stale ack-epoch rejection, and non-monotonic `received_at_ms` rejection
- [x] Hub now enforces default `SessionPolicy.e2ee_required=true`, rejecting plaintext chat/state until sender `E2EEKeyEpoch` bootstrap, with end-to-end handler tests
- [x] `SessionPolicy` now carries `e2ee_required` (wire + hub + CLI signatures), and `room-chat` adds `/e2eerequired on|off` policy control
- [x] Hub media-profile negotiation now deterministically falls back HDR->SDR when capability requirements are not met, with end-to-end handler tests for fallback and HDR-preserve paths
- [x] Hub baseline media join flow now has end-to-end handler coverage for forced join defaults (mic/video/share OFF) plus deterministic post-join mic/video state updates (`MEDIA-001`)
- [x] Hub screen-share concurrency (`max_screen_shares`) now has end-to-end handler coverage for deny-at-limit and allow-after-release behavior
- [x] Hub moderation action coverage now includes end-to-end handler tests for `DisableScreenShare` and `Kick` (target error + close signaling)
- [x] Hub recording flow coverage now includes end-to-end `RecordingNotice(state=stopped)` broadcast behavior
- [x] Hub reconnect resilience coverage now includes end-to-end tests for role/policy continuity (`SCALE-002`), including co-host and host role restoration by `participant_id` under room-lock/waiting-room policy
- [x] Hub fanout now emits moderator-visible, rate-limited backpressure notices with aggregate dropped counters when participant outbound queues are saturated, with end-to-end handler coverage (`SCALE-003`)
- [x] Hub roster stability coverage now includes end-to-end join/leave churn tests that assert deterministic roster consistency (`SCALE-001`)
- [x] Hub presence-delta stability coverage now includes end-to-end monotonic `ParticipantPresenceDelta.sequence` checks across join/role-change/leave flows (`SCALE-001`)
- [x] Hub long-duration stability coverage now includes accelerated soak tests for sustained control-plane/media/key-rotation/recording loops with invariant checks (`SCALE-004`)
- [x] `SCALE-004` soak evidence automation added via `scripts/run_scale004_soak.sh` and CI artifact upload (`target/conformance/scale004-soak-report.json`, `scale004-soak.log`)
- [x] Security/protocol smoke evidence automation added via `scripts/run_security_protocol_smoke.sh` and CI artifact upload (`security-protocol-smoke-report.json`, `security-protocol-smoke.log`)
- [x] Media/HDR/recording smoke evidence automation added via `scripts/run_media_hdr_recording_smoke.sh` and CI artifact upload (`media-hdr-recording-smoke-report.json`, `media-hdr-recording-smoke.log`)
- [x] Control-plane reliability smoke evidence automation added via `scripts/run_controlplane_reliability_smoke.sh` and CI artifact upload (`controlplane-reliability-smoke-report.json`, `controlplane-reliability-smoke.log`)
- [x] Conformance evidence bundle automation added via `scripts/run_conformance_evidence_bundle.sh` with CI bundle artifact upload (`conformance-evidence-bundle-report.json`, `conformance-evidence-bundle.log`)
- [x] Platform/browser parity smoke evidence automation added via `scripts/run_platform_contract_smoke.sh` and CI artifact upload (`platform-contract-smoke-report.json`, `platform-contract-smoke.log`)
- [x] Conformance coverage gate automation added via `scripts/run_conformance_coverage_check.sh` and CI artifact upload (`conformance-coverage-report.json`, `conformance-coverage.log`)
- [x] Security/protocol smoke evidence now includes `P-CONF-001` frame roundtrip coverage
- [x] CI contract sync guard added (`scripts/validate_ci_contract_sync.py`) to enforce workflow scenario/platform matrices stay aligned with frozen docs
- [x] Frozen platform contract artifact (`docs/platform-contract.json`) exported from CLI contract output and enforced in CI (`scripts/export_platform_contract_json.sh`)
- [x] Frozen media capability profile contract added (`docs/media-capability-profiles.json`) with CI validation (`scripts/validate_media_capability_profiles.py`)
- [x] Frozen HDR transport profile contract added (`docs/hdr-transport-profiles.json`) with CI validation (`scripts/validate_hdr_transport_profiles.py`)
- [x] Frozen HDR target-device results contract added (`docs/hdr-target-device-results.json`) with CI validation (`scripts/validate_hdr_target_device_results.py`)
- [x] Frozen platform blocker ledger added (`docs/platform-blockers.json`) with CI validation (`scripts/validate_platform_blockers.py`)
- [x] Frozen client app workspace contract added (`docs/client-app-workspaces.json`) with CI validation (`scripts/validate_client_app_workspaces.py`) and tracked workspace paths under `clients/`
- [x] Frozen client release-track contract added (`docs/client-release-tracks.json`) with CI validation (`scripts/validate_client_release_tracks.py`) for deterministic native/web release pipelines
- [x] Client release/rollback playbooks now include deterministic per-workspace contract tables aligned to `docs/client-release-tracks.json`, with CI validation (`scripts/validate_client_release_playbook_alignment.py`)
- [x] Frozen native-to-web fallback drill contract added (`docs/client-fallback-drills.json`) with CI validation (`scripts/validate_client_fallback_drills.py`) for per-native recovery-time fallback rehearsal
- [x] Frozen native-to-web fallback drill results contract added (`docs/client-fallback-drill-results.json`) with CI validation (`scripts/validate_client_fallback_drill_results.py`) for measured RTO and mandatory-browser fallback coverage
- [x] Frozen client release manifest contract added (`docs/client-release-manifest.json`) with CI validation (`scripts/validate_client_release_manifest.py`) for deterministic per-workspace artifact publication metadata
- [x] Frozen client release readiness gates contract added (`docs/client-release-readiness-gates.json`) with CI validation (`scripts/validate_client_release_readiness_gates.py`) to bind release-ready state to manifest and fallback evidence
- [x] Frozen client rollback manifest contract added (`docs/client-rollback-manifest.json`) with CI validation (`scripts/validate_client_rollback_manifest.py`) for deterministic per-workspace rollback artifact pointers aligned to release-track and release-manifest contracts
- [x] Client rollback manifest smoke evidence automation added via `scripts/run_client_rollback_manifest_smoke.sh` and CI artifact upload (`client-rollback-manifest-smoke-report.json`, `client-rollback-manifest-smoke.log`)
- [x] Frozen A/V baseline profile contract added (`docs/av-baseline-profiles.json`) with CI validation (`scripts/validate_av_baseline_profiles.py`)
- [x] Frozen screen-share constraints contract added (`docs/screen-share-constraints.json`) with CI validation (`scripts/validate_screen_share_constraints.py`)
- [x] Release and rollback playbooks added (`docs/release-playbook.md`, `docs/rollback-playbook.md`) with CI validation (`scripts/validate_release_playbooks.py`)
- [x] Critical defect ledger added (`docs/critical-defects.json`) with CI zero-defect validation (`scripts/validate_critical_defects.py`)
- [x] Release readiness automation added (`scripts/run_release_readiness_smoke.sh`, `scripts/generate_release_readiness_report.py`) with CI artifact upload (`release-readiness-smoke-report.json`, `release-readiness-report.json`)
- [x] Conformance evidence index automation added (`scripts/run_conformance_evidence_index.sh`) to generate scenario-to-report markdown and CI artifact upload
- [x] M0 specs frozen: protocol vNext contract, join-link schema/signature rules, role/permission model, parity matrix, and conformance test plan
- [x] M1 security + transport foundation gates completed for harness/protocol workstream scope
- [x] CLI join-link v2 security controls added (`exp`/`nonce`/`sig`), including expiry, signature verification, and in-process nonce replay rejection (legacy `v1` still supported)
- [x] CLI join-link v2 now enforces max future-expiry window and bounded nonce-cache capacity (fail-closed when full), with explicit tests
- [x] CLI `decode-join-link` now validates signed links without consuming replay nonces used by actual join flows
- [x] CLI `room-chat` now consumes join-link replay nonces after local preflight validation (avoids early nonce burn on local config errors)
- [x] CLI `platform-contract` command added to emit frozen browser/native parity contract JSON for downstream app teams
- [x] `room-chat` CLI commands added for vNext role/policy/media/security frame flows
- [x] CI scenario contract matrix now enforces `MOD-004` moderation audit visibility coverage
- [x] CI scenario contract matrix now enforces `MOD-001` host/co-host role assignment coverage
- [x] CI scenario contract matrix now enforces `SCALE-002` and `SCALE-003` scenario ID coverage
- [x] CI scenario contract matrix now enforces `P-CONF-002`, `P-CONF-003`, and `MEDIA-001` scenario ID coverage
- [x] CI scenario contract matrix now enforces `MEDIA-003` screen-share constraints coverage
- [x] CI scenario contract matrix now enforces `MEDIA-004` A/V baseline profile coverage
- [x] CI scenario contract matrix now enforces `HDR-003` HDR transport metadata coverage
- [x] CI scenario contract matrix now enforces `HDR-004` and `HDR-005` target-device/platform-blocker exit criteria coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-007` client app workspace contract coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-008` client release-track contract coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-009` client playbook/contract alignment coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-010` native-to-web fallback drill contract coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-011` native-to-web fallback drill results coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-012` client release manifest contract coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-013` client release readiness gates coverage
- [x] CI scenario contract matrix now enforces `PLATFORM-014` client rollback manifest contract coverage
- [x] Conformance coverage gate currently validates 59 mandatory scenario IDs (including `PLATFORM-014`) with passing evidence generation through `scripts/run_conformance_evidence_bundle.sh`
- [x] CI scenario contract matrix now enforces `OPS-001` and `OPS-002` release/rollback playbook coverage
- [x] CI scenario contract matrix now enforces `OPS-003`/`OPS-004` critical-defect and readiness-report coverage
- [x] CI scenario contract matrix now enforces `PARITY-001`/`PARITY-002`/`PARITY-003`/`PARITY-004`/`PARITY-005`/`PARITY-006`/`PARITY-007`/`PARITY-008`/`PARITY-009` parity governance and M3/GA/waiver-policy coverage
- [x] CI scenario contract matrix now enforces `OPS-005`/`OPS-006` hardening-gate and single-train GA-approval coverage
- [x] Platform hardening gate contract added (`docs/hardening-gates.json`) with CI validation (`scripts/validate_hardening_gates.py`)
- [x] Single-train GA approval contract added (`docs/ga-approvals.json`) with CI validation (`scripts/validate_ga_approvals.py`)
- [x] M3 exit-criteria automation added (`scripts/run_m3_exit_criteria_smoke.sh`, `scripts/validate_m3_exit_criteria.py`) with CI artifact upload (`m3-exit-criteria-smoke-report.json`)
- [x] GA parity-gate automation added (`scripts/run_parity_ga_smoke.sh`, `scripts/validate_parity_ga_gate.py`) with CI artifact upload (`parity-ga-smoke-report.json`, `parity-ga-smoke.log`)
- [x] Parity downgrade-guard automation added (`scripts/run_parity_downgrade_guard_smoke.sh`, `scripts/validate_parity_downgrade_guard.py`) with CI artifact upload (`parity-downgrade-guard-smoke-report.json`, `parity-downgrade-guard-smoke.log`)
- [x] Parity waiver-policy automation added (`scripts/run_parity_waiver_policy_smoke.sh`, `scripts/validate_parity_waiver_policy.py`) with CI artifact upload (`parity-waiver-policy-smoke-report.json`, `parity-waiver-policy-smoke.log`)
- [x] Parity waiver fixture-manifest automation added (`scripts/run_parity_waiver_fixture_manifest_smoke.sh`, `scripts/validate_parity_waiver_fixture_manifest.py`) with CI artifact upload (`parity-waiver-fixture-manifest-smoke-report.json`, `parity-waiver-fixture-manifest-smoke.log`)
- [x] Parity waiver fixture-coverage automation added (`scripts/run_parity_waiver_fixture_coverage_smoke.sh`, `scripts/validate_parity_waiver_fixture_coverage.py`) with CI artifact upload (`parity-waiver-fixture-coverage-smoke-report.json`, `parity-waiver-fixture-coverage-smoke.log`)
- [x] Parity waiver-policy negative-fixture automation added (`scripts/run_parity_waiver_policy_negative_smoke.sh`) with CI artifact upload (`parity-waiver-policy-negative-smoke-report.json`, `parity-waiver-policy-negative-smoke.log`)
- [x] Hardening/GA smoke automation added (`scripts/run_hardening_ga_smoke.sh`) with CI artifact upload (`hardening-ga-smoke-report.json`, `hardening-ga-smoke.log`)

## Milestones and Exit Criteria

## M0: Spec Freeze (target: 2026-03-15)

- [x] Freeze protocol vNext frame contracts in `docs/protocol-vnext.md`
- [x] Freeze join-link schema and signature rules
- [x] Freeze role/permission policy model (host/co-host/participant)
- [x] Freeze parity and conformance matrix in `docs/parity-matrix.md`

Exit criteria:

- [x] All platform/client teams can implement without unresolved interface questions

## M1: Security + Transport Foundation (target: 2026-05-15)

- [x] E2EE default with key rotation policy implemented in protocol/hub
- [x] Signed moderation/role actions with verification rules
- [x] Replay and stale-token rejection for join links and session actions
- [x] Control-plane stability under high churn toward 500-participant target

Exit criteria:

- [x] Protocol/security conformance suite passes in CI contract checks and local harness runs

## M2: Media + HDR Foundation Across All Platforms (target: 2026-08-15)

- [x] Camera/mic/speaker baseline on all native platforms and web
- [x] Screen share support with per-platform constraints documented
- [x] HDR capture/render negotiation and transport metadata defined and implemented
- [x] Deterministic SDR fallback/tone mapping behavior across all platforms

Exit criteria:

- [x] No platform blocks for core meeting operations
- [x] HDR and fallback scenarios pass target-device tests

## M3: Full Feature Parity Completion (target: 2026-11-15)

- [x] Host/co-host moderation parity across all platforms
- [x] Local recording parity and policy controls across all platforms
- [x] Reactions/hand raise/chat/roster parity
- [x] Waiting room and room lock policy parity
- [x] Web app behavior matches native feature contract where browser APIs permit equivalent UX

Exit criteria:

- [x] `docs/parity-matrix.md` rows are at least Beta for all platforms
- [x] All mandatory conformance scenarios in `docs/test-plan.md` pass

## M4: Hardening + Big-Bang GA (target: 2027-01-31)

- [x] Performance, reliability, and security gates pass for all platforms
- [x] Release and rollback playbooks complete for native and IPFS web artifacts
- [x] Known critical severity defects are zero

Exit criteria:

- [x] GA approval for all target platforms in one release train

## Workstreams

1. Protocol and Wire Evolution
- Extend `kaigi-wire` with role, session policy, key-rotation, capability, and recording notice frames.
- Preserve backward compatibility for currently deployed harness behavior where possible.

2. Security and Identity
- Hybrid identity: wallet-based host/co-host plus restricted guests.
- Signed and auditable high-risk actions (role changes, moderation, room policy).

3. Media and HDR
- Adaptive media profiles (SDR/HDR) with explicit capability negotiation.
- Platform-specific capture/render pipelines with shared acceptance criteria.

4. Client Applications
- Native implementations: macOS, iOS, iPadOS, Windows, Android, Linux.
- Web app on IPFS with direct Nexus/SoraNet connectivity and parity contract coverage.

5. Conformance, QA, and Scale
- Unified scenario catalog and pass/fail reporting by platform.
- Continuous compatibility and regression checks from docs and harness suites.

## Definition of Done for Parity GA

- Every mandatory row in `docs/parity-matrix.md` is marked `GA` for every platform.
- Every mandatory scenario in `docs/test-plan.md` has passing evidence in CI/manual reports.
- No unresolved P0/P1 security or reliability defects.
- Protocol docs and runtime behavior are aligned (no undocumented wire behavior).

## Out of Scope

- Webinar product (`taikai`)
- PSTN/telephony
- Centralized SaaS control-plane services

## Repository Deliverables

- `roadmap.md` (this file): scope, milestones, and gates
- `docs/parity-matrix.md`: per-platform parity tracking
- `docs/platform-contract.json`: machine-readable frozen parity contract artifact
- `docs/media-capability-profiles.json`: machine-readable frozen media/HDR capture-render profile contract
- `docs/hdr-transport-profiles.json`: machine-readable frozen per-platform HDR transport metadata + tone-mapping contract
- `docs/hdr-target-device-results.json`: machine-readable frozen per-platform HDR target-device pass evidence
- `docs/av-baseline-profiles.json`: machine-readable frozen per-platform camera/mic/speaker baseline contract
- `docs/screen-share-constraints.json`: machine-readable frozen per-platform screen-share constraints contract
- `docs/parity-status-waivers.json`: machine-readable parity status downgrade waiver contract
- `docs/parity-waiver-policy.json`: machine-readable parity waiver lifecycle policy contract
- `docs/client-app-workspaces.json`: machine-readable client implementation track contract (native + web fallback workspace mapping)
- `docs/client-release-tracks.json`: machine-readable client release-track contract (CI build/smoke commands, artifact kind, distribution channel, and signed native release requirements)
- `docs/client-fallback-drills.json`: machine-readable native-to-web fallback drill contract (per-native trigger, web fallback mapping, and bounded recovery-time objective)
- `docs/client-fallback-drill-results.json`: machine-readable native-to-web fallback drill results contract (per-native measured RTO outcome and mandatory web-browser fallback coverage)
- `docs/client-release-manifest.json`: machine-readable client release manifest contract (per-workspace artifact URI, checksum/signature/SBOM/provenance metadata, and publish readiness state)
- `docs/client-release-readiness-gates.json`: machine-readable client release readiness gates contract (per-workspace release-ready decision bound to manifest/fallback evidence and RTO outcomes)
- `docs/client-rollback-manifest.json`: machine-readable client rollback manifest contract (per-workspace rollback artifact URI, checksum/signature/SBOM/provenance metadata, and rollback-target release-train pointer)
- `docs/fixtures/waivers/manifest.json` (+ fixture corpus): machine-readable waiver-policy negative fixture manifest and deterministic case set
- `clients/`: tracked platform workspace paths for web/native implementation tracks
- `docs/platform-blockers.json`: machine-readable frozen per-platform blocker ledger for M2 exit criteria
- `docs/hardening-gates.json`: machine-readable per-platform performance/reliability/security hardening gate contract
- `docs/ga-approvals.json`: machine-readable single-train all-platform GA approval contract
- `docs/critical-defects.json`: machine-readable critical defect ledger (must remain zero-open for release gate)
- `docs/release-playbook.md`: release train execution playbook for native + IPFS web artifacts
- `docs/rollback-playbook.md`: rollback execution playbook for native + IPFS web artifacts
- `docs/protocol-vnext.md`: protocol/interfaces/types changes
- `docs/test-plan.md`: mandatory conformance scenarios
- `.github/workflows/conformance-matrix.yml`: CI contract checks for protocol and platform coverage
