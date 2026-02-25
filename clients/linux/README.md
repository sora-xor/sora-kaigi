# Linux Client Track

Platform: Linux.

Delivery contract:

- Build: native Rust desktop client core (`kaigi-linux-client`).
- Distribution: signed package artifacts.
- Fallback: browser fallback via `clients/web`.
- HDR policy: compositor-dependent HDR with deterministic SDR fallback.

## Workspace Layout

- `crates/kaigi-linux-client/` protocol/reducer/codec core and tests
- `clients/linux/` release-track metadata and platform notes

## Runtime Telemetry

Linux runtime exposes structured telemetry hooks via `MeetingTelemetrySink` on
`SessionRuntime`:

- `connection_lifecycle`: connect/reconnect scheduling, transport lifecycle, phase transitions, app
  background/foreground handling, connectivity handoff, and audio interruption/route events.
- `fallback_lifecycle`: fallback activation/recovery including `rto_ms` on recovery events.
- `policy_failure`: signed moderation/payment/e2ee policy rejection events.

## Local Validation

```bash
cargo test -p kaigi-linux-client
cargo build --release -p kaigi-linux-client
cargo run -p kaigi-linux-client
```

## Runtime Config Schema

Linux config schema aligns with native/web contracts:

- signaling endpoint
- fallback URL
- room/channel ID
- participant identity (`participant_id`; when blank, runtime derives a normalized ID from `participant_name`, and if normalization yields empty it falls back to `participant`)
- optional wallet identity
- policy flags for signed moderation and payment settlement
