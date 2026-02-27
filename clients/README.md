# Client Workspaces

This directory contains platform-specific client implementation tracks for `sora-kaigi`.

Mandatory tracks:

- `clients/web` (Web Chromium, Web Safari, Web Firefox)
- `clients/macos`
- `clients/ios`
- `clients/ipados`
- `clients/visionos`
- `clients/windows`
- `clients/android`
- `clients/linux`

Implemented Apple-native supplemental surfaces:

- `clients/apple/tvOS`
- `clients/apple/watchOS`

These are validated in Apple native smoke/UI suites (`APPLE-BUILD-005`/`APPLE-BUILD-006`) and are
represented as supplemental native workspace rows in `docs/client-app-workspaces.json`.

The machine-readable source of truth for these tracks is
`docs/client-app-workspaces.json`.

Release pipeline contract for these tracks is defined in
`docs/client-release-tracks.json`.

Native-to-web fallback drill contract for these tracks is defined in
`docs/client-fallback-drills.json`.

Native-to-web fallback drill results contract for these tracks is defined in
`docs/client-fallback-drill-results.json`.

Per-workspace release artifact manifest contract for these tracks is defined in
`docs/client-release-manifest.json`.

Per-workspace release readiness gate contract for these tracks is defined in
`docs/client-release-readiness-gates.json`.

Per-workspace rollback artifact manifest contract for these tracks is defined in
`docs/client-rollback-manifest.json`.
