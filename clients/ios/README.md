# iOS Client Track

Platform: iOS.
Shared Apple module details: `clients/apple/README.md`.

Delivery contract:

- Build: Swift + Xcode native app.
- Distribution: App Store/TestFlight artifact.
- Fallback: browser fallback via `clients/web`.
- HDR policy: AVFoundation HDR on supported devices, SDR fallback elsewhere.

## Run Locally

1. Generate the Xcode project:

```bash
xcodegen generate --spec Kaigi.yml
```

2. Open and run from Xcode:

- Open `Kaigi.xcworkspace`
- Select scheme `KaigiIOS`
- Choose an iPhone simulator
- Press Run

3. CLI simulator build:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiIOS -configuration Debug -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO
```

4. CLI tests:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiIOS -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.0' test CODE_SIGNING_ALLOWED=NO
```
