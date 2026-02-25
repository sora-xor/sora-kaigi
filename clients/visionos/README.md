# visionOS Client Track

Platform: visionOS.
Shared Apple module details: `clients/apple/README.md`.

Implementation source of truth:

- App entrypoint: `clients/apple/visionOS/KaigiVisionOSApp.swift`
- Shared protocol/session runtime: `clients/apple/Shared/`

Delivery profile:

- Build: Swift + Xcode native app target.
- Distribution: Apple platform package/signing flow (credentialed stage).
- Fallback: browser fallback via `clients/web`.
- HDR policy: AVFoundation HDR on supported devices, deterministic SDR fallback otherwise.

## Run Locally

1. Generate the Xcode project:

```bash
xcodegen generate --spec Kaigi.yml
```

2. Open and run from Xcode:

- Open `Kaigi.xcworkspace`
- Select scheme `KaigiVisionOS`
- Choose an Apple Vision Pro simulator
- Press Run

3. CLI simulator build:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiVisionOS -destination 'generic/platform=visionOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

4. CLI tests:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiVisionOS -destination 'platform=visionOS Simulator,name=Apple Vision Pro' test CODE_SIGNING_ALLOWED=NO
```

## CI Policy

- visionOS is optional in CI by default.
- `scripts/run_native_apple_smoke.sh` skips visionOS when `CI=true` unless `SKIP_VISIONOS=0` is explicitly set.
