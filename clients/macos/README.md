# macOS Client Track

Platform: macOS.
Shared Apple module details: `clients/apple/README.md`.

Delivery contract:

- Build: Swift + Xcode native app.
- Distribution: signed `.app`/`.dmg` artifact.
- Fallback: browser fallback via `clients/web`.
- HDR policy: AVFoundation HDR on supported devices, SDR fallback elsewhere.

## Run Locally

1. Generate the Xcode project (once per spec change):

```bash
xcodegen generate --spec Kaigi.yml
```

2. Build and run from Xcode:

- Open `Kaigi.xcworkspace`
- Select scheme `KaigiMacOS`
- Press Run

3. Build from CLI:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiMacOS -configuration Debug build CODE_SIGNING_ALLOWED=NO
```

4. Run tests:

```bash
xcodebuild -workspace Kaigi.xcworkspace -scheme KaigiMacOS -destination 'platform=macOS,arch=arm64' test CODE_SIGNING_ALLOWED=NO
```

## App Artifact Path

After CLI build:

`~/Library/Developer/Xcode/DerivedData/Kaigi-*/Build/Products/Debug/KaigiMacOS.app`
