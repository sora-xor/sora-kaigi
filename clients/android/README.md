# Android Client Track

Platform: Android.

Delivery contract:

- Build: Kotlin/NDK native app.
- Distribution: Play Store/internal APK/AAB artifact.
- Fallback: browser fallback via `clients/web`.
- HDR policy: Camera2/Codec HDR on supported devices, SDR fallback elsewhere.

## Prerequisites

- Android Studio / Android SDK (API 35 installed)
- JDK 21 for Gradle/Kotlin

## Run Locally

1. Verify root wrapper:

```bash
./gradlew -v
```

2. Ensure SDK path is configured (local only):

```bash
cat > clients/android/local.properties <<'EOF'
sdk.dir=/Users/<you>/Library/Android/sdk
EOF
```

3. Build and test:

```bash
./gradlew :app:testReleaseUnitTest
./gradlew :app:bundleRelease
```

4. Install debug app on emulator/device:

```bash
./gradlew :app:installDebug
```

5. Native smoke harness:

```bash
bash scripts/run_native_android_smoke.sh --out-dir target/conformance
```

`run_native_android_smoke.sh` defaults `GRADLE_USER_HOME` to repo-local
`.gradle/` and reuses `~/.gradle/wrapper/dists` when available, so smoke runs do
not require writing to global Gradle cache paths.

## Artifacts

- AAB: `clients/android/app/build/outputs/bundle/release/app-release.aab`

## Runtime Config Schema

Android native session config fields are aligned with Apple shared config:

- `signalingUrl`
- `fallbackUrl`
- `roomId`
- `participant`
- `participantId` (optional; falls back to normalized `participant` when blank)
- `walletIdentity` (optional)
- `requireSignedModeration` (boolean)
- `requirePaymentSettlement` (boolean)
- `preferWebFallbackOnPolicyFailure` (boolean)
- `supportsHdrCapture` (optional override)
- `supportsHdrRender` (optional override)

## Observability Hooks

Android runtime exposes `MeetingTelemetrySink` (default `NoOpMeetingTelemetrySink`) on
`MeetingViewModel` for structured native telemetry events:

- `connection_lifecycle`: connect attempts, reconnect scheduling, transport transitions, and
  network availability changes, app background/foreground transitions, and audio interruption/route
  lifecycle events.
- `fallback_lifecycle`: fallback activation and fallback recovery (`rto_ms` attribute).
- `policy_failure`: signed-policy and moderation rejection surfaces (`code` + `message`).

Audio focus interruption callbacks are edge-mapped (`loss* -> began`, `gain* -> ended`) so
reconnect recovery only triggers after an observed interruption.

## Release Handoff (Unsigned Stage)

Before a GA handoff, Android release owners should generate the standard
ops handoff bundle:

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

3. Provide these artifacts to release operations:
- `target/conformance/native-ops-handoff-package/`
- `target/conformance/native-ops-handoff-package.tar.gz`
- `target/conformance/native-ops-handoff-package-report.json`
