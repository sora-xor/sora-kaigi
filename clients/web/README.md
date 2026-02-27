# Web Client Track

Platforms: Web Chromium, Web Safari, Web Firefox.

Delivery contract:

- Build: TypeScript + Vue 3 + WebRTC app bundle.
- Distribution: IPFS-hosted web artifact.
- HDR policy: runtime capability negotiation and deterministic SDR fallback.

## Workspace Layout

- `clients/web/src/session/` protocol state model, reducer, transport, codec
- `clients/web/src/App.vue` session dashboard and control surface
- `clients/web/tests/` reducer/codec/smoke tests

## Local Validation

```bash
npm --prefix clients/web ci
npm --prefix clients/web run test
npm --prefix clients/web run build
npm --prefix clients/web run test:smoke
npm --prefix clients/web run test:e2e
```

## Runtime Config Schema

Web session config fields are aligned with native tracks:

- `signalingUrl`
- `fallbackUrl`
- `roomId`
- `participantId`
- `participantName`
- `walletIdentity` (optional)
- `requireSignedModeration` (boolean)
- `requirePaymentSettlement` (boolean)
- `preferWebFallbackOnPolicyFailure` (boolean)
- `supportsHdrCapture` (boolean)
- `supportsHdrRender` (boolean)

## Notes

- This track includes a reducer-driven state machine with session phases:
  - `Disconnected`, `Connecting`, `Connected`, `Degraded`, `FallbackActive`, `Error`.
- Production fallback target for all native clients remains this web workspace.
- If `participantId` is blank, the runtime derives a normalized ID from `participantName`.
- If an explicit `participantId` normalizes to an empty value, the runtime falls back to `participant`.
- `useMeetingSession` supports structured telemetry sinks via `MeetingTelemetrySink` with
  categories `connection_lifecycle`, `fallback_lifecycle`, and `policy_failure`.
