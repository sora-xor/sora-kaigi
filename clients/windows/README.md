# Windows Client Track

Platform: Windows.

Delivery contract:

- Build: native Windows app (WinUI 3 track with C# protocol/session core).
- Distribution: signed installer artifact.
- Fallback: browser fallback via `clients/web`.
- HDR policy: DXGI/Media Foundation HDR on supported devices, SDR fallback elsewhere.

## Workspace Layout

- `clients/windows/src/Kaigi.Windows/` protocol/reducer/codec core
- `SessionRuntime` in `clients/windows/src/Kaigi.Windows/Core.cs` for deterministic reconnect backoff, rehello/resume handshake orchestration, ping/pong handling, and fallback activation
- `clients/windows/tests/Kaigi.Windows.Tests/` xUnit coverage
- `clients/windows/Kaigi.Windows.sln` solution entrypoint

## Runtime Telemetry

Windows runtime exposes structured telemetry hooks via `IMeetingTelemetrySink` on
`SessionRuntime`:

- `connection_lifecycle`: connect/reconnect scheduling, transport lifecycle, phase transitions, app
  background/foreground handling, connectivity handoff, and audio interruption/route events.
- `fallback_lifecycle`: fallback activation/recovery including `rto_ms` on recovery events.
- `policy_failure`: signed moderation/payment/e2ee policy rejection events.

## Local Validation (Windows host)

```powershell
dotnet restore clients/windows/Kaigi.Windows.sln
dotnet build clients/windows/Kaigi.Windows.sln -c Release
dotnet test clients/windows/Kaigi.Windows.sln -c Release
```

## Runtime Config Schema

Windows config schema aligns with native/web contracts:

- signaling endpoint
- fallback URL
- room/channel ID
- participant identity (`ParticipantId`; when blank, runtime derives a normalized ASCII ID from `ParticipantName`, allowing only `a-z`, `0-9`, `_`, `-`)
- optional wallet identity
- policy flags for signed moderation and payment settlement
