# Rollback Playbook

Last updated: 2026-02-25

## Trigger Conditions

- Critical security, reliability, or meeting-availability regression in production.
- Cross-platform parity break affecting mandatory capabilities.
- Release artifact integrity/signature mismatch.

## Decision and Ownership

- Incident commander confirms rollback scope and severity.
- Security, client leads, and release owner approve rollback execution.
- Release communications owner publishes stakeholder updates.

## Rollback Pipeline Stages

### Stage 1: Unsigned Verification (Default)

- Validate rollback metadata and readiness without credential-backed publish actions.
- Required checks:
  - `bash scripts/run_client_rollback_manifest_smoke.sh`
  - `bash scripts/run_client_release_readiness_gates_smoke.sh`
  - `bash scripts/run_client_fallback_drill_results_smoke.sh`
- Confirm fallback-to-web pathways remain viable for all native workspaces.
- Export rollback handoff bundle for ops:
  - `target/conformance/release-readiness-report.json`
  - `docs/client-release-manifest.json`
  - `docs/client-rollback-manifest.json`
  - `docs/client-fallback-drill-results.json`

### Stage 2: Credentialed Rollback Publish

- Execute only in protected release environments with release-owner approval.
- Includes store/promote/notarization channel actions for signed native artifacts.
- Must remain disabled in shared CI and non-release local environments.

## Client Rollback Track Contract

| Workspace ID | Platforms | Distribution Channel | Signing Required | Fallback Workspace |
|---|---|---|---|---|
| android | Android | google-play | true | web |
| ios | iOS | app-store-connect | true | web |
| ipados | iPadOS | app-store-connect | true | web |
| linux | Linux | signed-package-repo | true | web |
| macos | macOS | apple-notarized-distribution | true | web |
| tvos | tvOS | app-store-connect | true | web |
| visionos | visionOS | app-store-connect | true | web |
| watchos | watchOS | app-store-connect | true | web |
| web | Web Chromium, Web Safari, Web Firefox | ipfs | false | - |
| windows | Windows | winget-msi | true | web |

## Native Rollback Tracks

### macOS

- Revoke current channel rollout and re-point distribution to last known-good notarized build.
- Verify entitlement set and signing chain on rollback artifact.

### iOS

- Halt phased rollout and promote last known-good TestFlight/App Store version.
- Validate key meeting paths after rollback promotion.

### iPadOS

- Halt phased rollout and promote last known-good TestFlight/App Store version.
- Validate iPad-specific layout and share controls on rollback build.

### tvOS

- Halt rollout and promote last known-good tvOS build in the Apple channel.
- Validate focus/navigation behavior and dashboard/connect flow on rollback build.

### watchOS

- Halt rollout and promote last known-good watchOS build in the Apple channel.
- Validate compact dashboard/connect flow and fallback signaling on rollback build.

### visionOS

- Halt phased rollout and promote last known-good TestFlight/App Store version.
- Validate immersive-mode transitions and fallback-to-web behavior on rollback build.

### Windows

- Withdraw current installer/MSIX and republish previous signed package.
- Verify upgrade/downgrade migration preserves settings and call continuity.

### Android

- Stop production rollout and promote last known-good Play artifact.
- Validate media capture and share permissions after downgrade path.

### Linux

- Revert package repository pointers to last known-good release set.
- Validate package dependency compatibility and startup health.

## IPFS Web Rollback

- Re-point release manifest to previous stable CID.
- Keep failing CID pinned for incident forensics but remove from active discovery.
- Validate Web Chromium, Web Safari, and Web Firefox join + media smoke paths on rollback CID.

## Incident Communication

1. Publish rollback start notice with reason and scope.
2. Update status checkpoints every 30 minutes until rollback completion.
3. Publish rollback completion notice with incident follow-up owner.

## Exit Criteria

- All rollback target artifacts are live and verified.
- Mandatory conformance smoke checks pass on rollback versions.
- Incident review and permanent remediation owner are recorded.
