#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="CTRL-RELIABILITY-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/controlplane-reliability-smoke.log"
REPORT_FILE="${OUT_DIR}/controlplane-reliability-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "P-CONF-004|kaigi-cli|tests::join_link_v2_rejects_replay_nonce"
  "P-CONF-004|kaigi-cli|tests::join_link_v2_rejects_exp_too_far_in_future"
  "P-CONF-004|kaigi-cli|tests::join_link_nonce_cache_rejects_when_full"
  "P-CONF-004|kaigi-hub-echo|tests::moderation_signed_handle_frame_rejects_replay_sent_at_ms"
  "P-CONF-004|kaigi-hub-echo|tests::role_grant_handle_frame_rejects_replay_issued_at_ms"
  "P-CONF-004|kaigi-hub-echo|tests::role_revoke_handle_frame_rejects_replay_issued_at_ms"
  "P-CONF-004|kaigi-hub-echo|tests::session_policy_handle_frame_rejects_replay_updated_at_ms"
  "P-CONF-004|kaigi-hub-echo|tests::e2ee_key_epoch_handle_frame_rejects_replay_sent_at_ms"
  "SEC-001|kaigi-hub-echo|tests::chat_handle_frame_requires_e2ee_key_epoch_when_policy_enabled"
  "SEC-001|kaigi-hub-echo|tests::participant_state_handle_frame_requires_e2ee_key_epoch_when_policy_enabled"
  "SCALE-002|kaigi-hub-echo|tests::reconnect_rejoin_restores_cohost_role_and_policy"
  "SCALE-002|kaigi-hub-echo|tests::reconnect_rejoin_restores_host_role_after_temporary_disconnect"
  "SCALE-003|kaigi-hub-echo|tests::broadcast_frame_notifies_moderators_when_fanout_backpressures"
  "SCALE-003|kaigi-hub-echo|tests::broadcast_frame_rate_limits_backpressure_notices"
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

echo "Control-plane reliability smoke status: ${suite_status}"
echo "Control-plane reliability smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
