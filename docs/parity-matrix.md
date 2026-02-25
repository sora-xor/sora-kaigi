# Parity Matrix

Status:

- Frozen on 2026-02-15 for M0 implementation.
- Capability status transitions remain evidence-gated per `docs/test-plan.md`.

Legend:

- `P`: Planned
- `A`: Alpha
- `B`: Beta
- `GA`: General Availability

Parity gate rule:

- GA release requires every mandatory row to be `GA` on all target platforms.

## Mandatory Platforms

- Web Chromium (IPFS-hosted)
- Web Safari (IPFS-hosted)
- Web Firefox (IPFS-hosted)
- macOS
- iOS
- iPadOS
- visionOS
- Windows
- Android
- Linux

## Core Meeting Capability Matrix

| Capability | Web Chromium | Web Safari | Web Firefox | macOS | iOS | iPadOS | visionOS | Windows | Android | Linux |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Wallet host/co-host identity | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Guest join with restricted policy | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Host/co-host/participant role model | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Moderation: mute/video-off/share-off/kick | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Waiting room + room lock | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Roster, chat, reactions, hand raise | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| E2EE default + key rotation | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Camera and microphone controls | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Screen share | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Local recording (policy-controlled) | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Reconnect and session continuity | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| 500 interactive participant support | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Accessibility shortcuts and labels | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |

## HDR Matrix

| HDR Capability | Web Chromium | Web Safari | Web Firefox | macOS | iOS | iPadOS | visionOS | Windows | Android | Linux |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Device HDR capability detection | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| HDR capture on supported devices | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| HDR render on supported displays | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| SDR fallback with tone mapping | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| UX indicator for active media profile | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |

## Moderation and Governance Matrix

| Governance Capability | Web Chromium | Web Safari | Web Firefox | macOS | iOS | iPadOS | visionOS | Windows | Android | Linux |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Host assignment and transfer | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Co-host assignment and revocation | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Recording policy enforcement | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |
| Signed moderation action audit trail | GA | GA | GA | GA | GA | GA | GA | GA | GA | GA |

## Upgrade Policy

- Status transitions must be evidence-backed by scenario IDs in `docs/test-plan.md`.
- Status changes require links to test artifacts in PR descriptions.
- Downgrades from `GA` to `B/A/P` require explicit temporary waivers in
  `docs/parity-status-waivers.json`.
- Waiver quality/lifetime constraints are enforced by `docs/parity-waiver-policy.json`.
- Camera/mic/speaker baseline expectations must stay aligned with
  `docs/av-baseline-profiles.json`.
- HDR transport metadata and SDR tone-mapping requirements must stay aligned with
  `docs/hdr-transport-profiles.json`.
- HDR target-device evidence and platform blocker state must stay aligned with
  `docs/hdr-target-device-results.json` and `docs/platform-blockers.json`.
- Screen-share behavior and limitations must stay aligned with
  `docs/screen-share-constraints.json`.
