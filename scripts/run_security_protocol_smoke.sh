#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="target/conformance"
POSITIONAL_OUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --out-dir requires a value" >&2
        exit 2
      fi
      OUT_DIR="$2"
      shift 2
      ;;
    --help|-h)
      cat <<'EOF'
Usage: bash scripts/run_security_protocol_smoke.sh [--out-dir <path>] [out_dir]
EOF
      exit 0
      ;;
    -*)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
    *)
      if [[ -n "${POSITIONAL_OUT_DIR}" ]]; then
        echo "Only one positional output directory is supported: $1" >&2
        exit 2
      fi
      POSITIONAL_OUT_DIR="$1"
      shift
      ;;
  esac
done

if [[ -n "${POSITIONAL_OUT_DIR}" ]]; then
  OUT_DIR="${POSITIONAL_OUT_DIR}"
fi
SUITE_ID="SEC-PROTO-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/security-protocol-smoke.log"
REPORT_FILE="${OUT_DIR}/security-protocol-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "P-CONF-001|kaigi-wire|tests::vnext_frames_roundtrip"
  "P-CONF-002|kaigi-wire|tests::decode_rejects_malformed_payload_bytes"
  "P-CONF-002|kaigi-hub-echo|tests::permissions_snapshot_handle_frame_rejects_client_injection"
  "P-CONF-002|kaigi-hub-echo|tests::participant_presence_delta_handle_frame_rejects_client_injection"
  "P-CONF-003|kaigi-wire|tests::legacy_frames_roundtrip_with_vnext_decoder"
  "SEC-002|kaigi-hub-echo|tests::key_rotation_ack_handle_frame_broadcasts_when_ack_within_sender_epoch"
  "SEC-002|kaigi-hub-echo|tests::key_rotation_ack_handle_frame_rejects_ack_above_sender_epoch"
  "SEC-002|kaigi-hub-echo|tests::key_rotation_ack_handle_frame_rejects_replay_or_stale_ack_epoch"
  "SEC-002|kaigi-hub-echo|tests::key_rotation_ack_handle_frame_rejects_replay_received_at_ms"
  "SEC-004|kaigi-hub-echo|tests::role_grant_handle_frame_rejects_bad_signature"
  "SEC-004|kaigi-hub-echo|tests::role_revoke_handle_frame_rejects_bad_signature"
  "SEC-004|kaigi-hub-echo|tests::session_policy_handle_frame_rejects_bad_signature"
  "SEC-004|kaigi-hub-echo|tests::e2ee_key_epoch_handle_frame_rejects_bad_signature"
  "SEC-003|kaigi-hub-echo|tests::join_policy_rejects_duplicate_participant_id"
  "SEC-003|kaigi-hub-echo|tests::join_policy_rehello_rejects_participant_id_change"
  "SEC-003|kaigi-hub-echo|tests::role_grant_rejects_non_host_sender"
  "MOD-001|kaigi-hub-echo|tests::role_grant_handle_frame_applies_and_broadcasts_audit"
  "MOD-001|kaigi-hub-echo|tests::role_revoke_handle_frame_applies_and_broadcasts_audit"
  "MOD-003|kaigi-hub-echo|tests::hello_handle_frame_rejects_join_when_room_lock_enabled"
  "MOD-004|kaigi-hub-echo|tests::moderation_signed_handle_frame_applies_and_broadcasts_audit"
  "MOD-005|kaigi-hub-echo|tests::waiting_room_admit_handle_frame_promotes_pending_participant"
  "MOD-005|kaigi-hub-echo|tests::waiting_room_deny_handle_frame_disconnects_pending_participant"
  "MOD-006|kaigi-hub-echo|tests::hello_handle_frame_rejects_guest_when_guest_policy_disabled"
  "MEDIA-001|kaigi-hub-echo|tests::media_baseline_join_forces_media_off_then_allows_mic_video_updates"
  "SCALE-001|kaigi-hub-echo|tests::participant_presence_delta_sequence_is_monotonic_across_join_role_and_leave"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""

for case in "${CASES[@]}"; do
  IFS='|' read -r scenario_id package test_name <<<"${case}"
  echo "=== ${scenario_id} :: ${package} :: ${test_name} ===" | tee -a "${LOG_FILE}"
  case_status="passed"
  if ! cargo test -p "${package}" "${test_name}" -- --exact --nocapture 2>&1 | tee -a "${LOG_FILE}"; then
    case_status="failed"
    suite_status="failed"
  fi

  entry="{\"scenario_id\":\"${scenario_id}\",\"package\":\"${package}\",\"test_name\":\"${test_name}\",\"status\":\"${case_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${entry}"
  else
    results_json="${results_json},${entry}"
  fi
done

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF
{
  "suite_id": "${SUITE_ID}",
  "status": "${suite_status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF

echo "Security/protocol smoke status: ${suite_status}"
echo "Security/protocol smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
