# macOS Client Track

Platform: macOS.

Delivery contract:

- Build: Swift + Xcode native app.
- Distribution: signed `.app`/`.dmg` artifact.
- Fallback: browser fallback via `clients/web`.
- HDR policy: AVFoundation HDR on supported devices, SDR fallback elsewhere.
