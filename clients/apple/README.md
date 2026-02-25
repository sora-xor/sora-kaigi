# Apple Native Tracks (iOS, iPadOS, visionOS, macOS)

Shared Apple-native implementation lives under `clients/apple/` and is consumed
by platform app targets/schemes:

- `KaigiIOS` (`clients/ios/`)
- `KaigiIPadOS` (`clients/ipados/`)
- `KaigiVisionOS` (`clients/visionos/`)
- `KaigiMacOS` (`clients/macos/`)

## Runtime Config Schema

Apple shared `MeetingConfig` fields are aligned with Android config:

- `signalingURLText`
- `fallbackURLText`
- `roomID`
- `participantID` (optional; falls back to normalized `participantName` when blank)
- `participantName`
- `walletIdentity` (optional)
- `requireSignedModeration` (boolean)
- `requirePaymentSettlement` (boolean)
- `preferWebFallbackOnPolicyFailure` (boolean)
- `supportsHDRCapture` (optional override)
- `supportsHDRRender` (optional override)

## Observability Hooks

Apple shared runtime exposes `MeetingTelemetrySink` (default `NoOpMeetingTelemetrySink`) on
`MeetingSession` for structured native telemetry events:

- `connection_lifecycle`: connect attempts, reconnect scheduling, transport transitions, and
  network availability changes, app background/foreground transitions, and media interruption/route
  lifecycle events.
- `fallback_lifecycle`: fallback activation and fallback recovery (`rto_ms` attribute).
- `policy_failure`: signed-policy and moderation rejection surfaces (`code` + `message`).

Apple shared runtime also surfaces platform media lifecycle hooks:

- `onAudioInterruptionBegan` / `onAudioInterruptionEnded(shouldReconnect:)`
- `onAudioRouteChanged(reason:)`
- `onScreenCaptureCapabilityChanged(available:source:)`

`MeetingDashboardView` wires these to AVAudioSession notifications (iOS/visionOS) and macOS
screen-capture capability preflight checks.

## Local Validation

```bash
bash scripts/run_native_apple_smoke.sh --platform ios --out-dir target/conformance --skip-xcodegen
bash scripts/run_native_apple_smoke.sh --platform ipados --out-dir target/conformance --skip-xcodegen
bash scripts/run_native_apple_smoke.sh --platform visionos --out-dir target/conformance --skip-xcodegen
bash scripts/run_native_apple_smoke.sh --platform macos --out-dir target/conformance --skip-xcodegen
```

CI default:
- `scripts/run_native_apple_smoke.sh` skips `visionOS` when `CI=true`.
- Set `SKIP_VISIONOS=0` to explicitly enable the visionOS suite in CI.
- Outside CI, `ALLOW_SIMULATOR_SKIPS` defaults to `1` for `--platform all`; when
  CoreSimulator discovery is unavailable, iOS/iPadOS/visionOS suites are marked
  `skipped` instead of failing the whole run.
- Set `ALLOW_SIMULATOR_SKIPS=0` to enforce strict local simulator availability.

If simulator device discovery is unavailable in a restricted environment, use
compile-only validation for Apple test bundles:

```bash
xcodebuild build-for-testing -workspace Kaigi.xcworkspace -scheme KaigiMacOS -destination 'platform=macOS'
xcodebuild build-for-testing -workspace Kaigi.xcworkspace -scheme KaigiIPadOS -destination 'generic/platform=iOS Simulator'
```

`KaigiVisionOS` build/test still requires the visionOS platform/runtime to be installed.

## Release Handoff (Unsigned Stage)

1. Full handoff package:

```bash
bash scripts/run_native_ops_handoff_package.sh \
  --out-dir target/conformance \
  --metadata docs/client-release-manifest-input.template.json
```

2. Fast rerun (reuse existing conformance evidence):

```bash
bash scripts/run_native_ops_handoff_package.sh \
  --out-dir target/conformance \
  --metadata docs/client-release-manifest-input.template.json \
  --skip-conformance-refresh
```

3. Handoff package for ops:
- `target/conformance/native-ops-handoff-package/`
- `target/conformance/native-ops-handoff-package.tar.gz`
- `target/conformance/native-ops-handoff-package-report.json`
