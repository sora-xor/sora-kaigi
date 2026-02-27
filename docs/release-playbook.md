# Release Playbook

Last updated: 2026-02-25

## Scope

This playbook defines the single release train process for native clients and the IPFS-hosted web
fallback for `sora-kaigi`.

## Preconditions

- `docs/test-plan.md` mandatory scenarios have passing evidence.
- `docs/parity-matrix.md` mandatory rows are at the required stage for the release gate.
- No unresolved P0/P1 security or reliability defects remain.
- Signed artifacts and checksums are generated for every release target.

## Artifact Build and Signing

1. Build native client artifacts from the tagged release commit.
2. Build web artifact bundle and content-addressed IPFS package from the same commit.
3. Generate checksums, software bill of materials, and signatures for every artifact.
4. Publish signed release manifest with artifact hashes and rollback pointer references.

## Native Pipeline Stages

### Stage 1: Unsigned CI (Default)

- This stage is credential-free and must pass on every PR/release branch.
- Aggregated UI E2E helper: `bash scripts/run_client_ui_e2e_smoke.sh --out-dir target/conformance`
- Android command: `bash scripts/run_native_android_smoke.sh --out-dir target/conformance`
- Apple command (macOS/iOS/iPadOS/tvOS/watchOS + optional visionOS): `bash scripts/run_native_apple_smoke.sh --out-dir target/conformance --skip-xcodegen`
- Apple CI default: visionOS smoke is skipped when `CI=true` (set `SKIP_VISIONOS=0` to enable it in a credentialed Apple environment).
- Web command: `bash scripts/run_web_client_smoke.sh --out-dir target/conformance`
- Linux command: `bash scripts/run_linux_client_smoke.sh --out-dir target/conformance`
- Windows command: `bash scripts/run_windows_client_smoke.sh --out-dir target/conformance`
- Contract/readiness commands:
  - `bash scripts/run_client_release_tracks_smoke.sh`
  - `bash scripts/run_client_fallback_drills_smoke.sh`
  - `bash scripts/run_client_fallback_drill_results_smoke.sh`
  - `bash scripts/run_client_release_manifest_smoke.sh`
  - `bash scripts/run_client_release_readiness_gates_smoke.sh`
  - `bash scripts/run_client_rollback_manifest_smoke.sh`

### Stage 1A: Ops Handoff Package (Required Before Tagging)

- Owner: native release owner.
- Reviewers: operations owner + incident commander backup.
- Build an explicit handoff package from CI evidence and release metadata using one command:
  - `bash scripts/run_native_ops_handoff_package.sh --out-dir target/conformance --metadata docs/client-release-manifest-input.template.json`
  - Optional fast rerun (if evidence already exists): `bash scripts/run_native_ops_handoff_package.sh --out-dir target/conformance --metadata docs/client-release-manifest-input.template.json --skip-conformance-refresh`
- Mandatory handoff artifacts to share with ops:
  - `target/conformance/native-ops-handoff-package/`
  - `target/conformance/native-ops-handoff-package.tar.gz`
  - `target/conformance/native-ops-handoff-package-report.json`
- Required acknowledgment: ops owner confirms these artifacts for the release train before GA tagging.

### Stage 2: Signed Distribution (Credentialed)

- This stage is disabled by default in shared CI and local developer runs.
- Enable only in a credentialed release environment with explicit release-owner approval.
- Required credential classes:
  - Apple signing identities + provisioning + notarization credentials
  - Play Console service credentials + release signing keys
- Store-upload and notarization actions are explicitly out-of-scope for Stage 1 and must not run
  unless credential gates are intentionally opened.

## Client Release Track Contract

| Workspace ID | Platforms | Artifact Kind | Distribution Channel | Signing Required | HDR Validation Required |
|---|---|---|---|---|---|
| android | Android | play-aab | google-play | true | true |
| ios | iOS | app-store-ipa | app-store-connect | true | true |
| ipados | iPadOS | app-store-ipa | app-store-connect | true | true |
| linux | Linux | signed-linux-package | signed-package-repo | true | true |
| macos | macOS | signed-dmg | apple-notarized-distribution | true | true |
| tvos | tvOS | app-store-ipa | app-store-connect | true | true |
| visionos | visionOS | app-store-ipa | app-store-connect | true | true |
| watchos | watchOS | app-store-ipa | app-store-connect | true | true |
| web | Web Chromium, Web Safari, Web Firefox | ipfs-web-bundle | ipfs | false | true |
| windows | Windows | signed-msi | winget-msi | true | true |

## Native Release Tracks

## Runtime Observability Requirements

- Native runtimes emit structured session telemetry hooks:
  - Apple/Android: `MeetingTelemetrySink`
  - Windows: `IMeetingTelemetrySink`
  - Linux: `MeetingTelemetrySink`
- Web runtime (`useMeetingSession`) exposes an equivalent `MeetingTelemetrySink` contract used by
  native fallback drill orchestration.
- Mandatory event categories for GA: `connection_lifecycle`, `fallback_lifecycle`, and
  `policy_failure`.
- Fallback drill verification must include captured `fallback_recovered` `rto_ms` values.

### macOS

- Build signed notarized application package.
- Verify camera/mic/screen-share entitlements and hardened runtime settings.

### iOS

- Build signed App Store/TestFlight artifact with production configuration profile.
- Validate ReplayKit and media capability declarations.

### iPadOS

- Build signed App Store/TestFlight artifact with iPad-specific UI support enabled.
- Validate split-view constraints and share pipeline configuration.

### tvOS

- Build signed tvOS artifact in the Apple distribution pipeline.
- Validate Siri Remote focus/navigation behavior and dashboard/connect flow parity.

### watchOS

- Build signed watchOS companion artifact in the Apple distribution pipeline.
- Validate compact dashboard/connect flow parity and fallback status signaling.

### visionOS

- Build signed App Store/TestFlight artifact with visionOS simulator/device configuration.
- Validate immersive-space transitions, media capture lifecycles, and fallback handoff behavior.

### Windows

- Build signed installer/MSIX package for production channel.
- Verify screen-capture permission flow and codec capability fallback behavior.

### Android

- Build signed Play release artifact (AAB/APK as required by channel).
- Verify MediaProjection and foreground-service declarations for screen share.

### Linux

- Build signed package artifacts for supported distribution targets.
- Verify PipeWire/portal integration defaults and fallback behavior.

## IPFS Web Release

- Build deterministic web bundle for IPFS publish.
- Pin content-addressed artifacts to production pin set.
- Publish release manifest mapping release tag to CID and integrity metadata.
- Verify browser compatibility on Web Chromium, Web Safari, and Web Firefox.

## Launch Checklist

1. Publish native artifacts to staged channels.
2. Publish IPFS CIDs and update release discovery metadata.
3. Run release-day smoke checks for join, media, moderation, and recording policy paths.
4. Confirm rollback pointers are recorded for every platform artifact.

## Post-Release Verification

- Monitor conformance and crash signals for all target platforms.
- Confirm no cross-platform parity regressions in first production window.
- Keep previous release artifacts available until rollback window expires.
