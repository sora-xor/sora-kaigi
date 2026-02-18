# Release Playbook

Last updated: 2026-02-15

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

## Client Release Track Contract

| Workspace ID | Platforms | Artifact Kind | Distribution Channel | Signing Required | HDR Validation Required |
|---|---|---|---|---|---|
| android | Android | play-aab | google-play | true | true |
| ios | iOS | app-store-ipa | app-store-connect | true | true |
| ipados | iPadOS | app-store-ipa | app-store-connect | true | true |
| linux | Linux | signed-linux-package | signed-package-repo | true | true |
| macos | macOS | signed-dmg | apple-notarized-distribution | true | true |
| web | Web Chromium, Web Safari, Web Firefox | ipfs-web-bundle | ipfs | false | true |
| windows | Windows | signed-msi | winget-msi | true | true |

## Native Release Tracks

### macOS

- Build signed notarized application package.
- Verify camera/mic/screen-share entitlements and hardened runtime settings.

### iOS

- Build signed App Store/TestFlight artifact with production configuration profile.
- Validate ReplayKit and media capability declarations.

### iPadOS

- Build signed App Store/TestFlight artifact with iPad-specific UI support enabled.
- Validate split-view constraints and share pipeline configuration.

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
