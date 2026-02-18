#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="PLATFORM-CONTRACT-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/platform-contract-smoke.log"
REPORT_FILE="${OUT_DIR}/platform-contract-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "PLATFORM-001|kaigi-platform-contract|tests::all_target_platforms_have_contracts"
  "PLATFORM-002|kaigi-platform-contract|tests::native_platforms_require_web_fallback"
  "PLATFORM-003|kaigi-platform-contract|tests::all_platforms_enforce_security_baseline"
  "PLATFORM-004|kaigi-platform-contract|tests::all_platforms_define_media_hdr_and_sdr_fallback"
  "PLATFORM-005|kaigi-platform-contract|tests::all_platforms_target_full_feature_parity"
  "PLATFORM-006|kaigi-platform-contract|tests::windows_is_native_with_web_fallback"
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

cat >"${REPORT_FILE}" <<EOF_JSON
{
  "suite_id": "${SUITE_ID}",
  "status": "${suite_status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF_JSON

echo "Platform-contract smoke status: ${suite_status}"
echo "Platform-contract smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
