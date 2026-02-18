#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="PARITY-WAIVER-POLICY-NEGATIVE-SMOKE"
SCENARIO_ID="PARITY-007"
MANIFEST_FILE="docs/fixtures/waivers/manifest.json"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/parity-waiver-policy-negative-smoke.log"
REPORT_FILE="${OUT_DIR}/parity-waiver-policy-negative-smoke-report.json"
CHECKS_JSON_FILE="${OUT_DIR}/parity-waiver-policy-negative-checks.json"
EXEC_LOG_FILE="${OUT_DIR}/parity-waiver-policy-negative-exec.log"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== ${SCENARIO_ID} :: validate fixture manifest ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_parity_waiver_fixture_manifest.py \
  --manifest "${MANIFEST_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

echo "=== ${SCENARIO_ID} :: execute fixture corpus ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/execute_parity_waiver_fixtures.py \
  --manifest "${MANIFEST_FILE}" \
  --checks-json "${CHECKS_JSON_FILE}" \
  --log-file "${EXEC_LOG_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

checks_json="[]"
if [[ -f "${CHECKS_JSON_FILE}" ]]; then
  checks_json="$(cat "${CHECKS_JSON_FILE}")"
fi

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF_JSON
{
  "suite_id": "${SUITE_ID}",
  "status": "${status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "checks_file": "${CHECKS_JSON_FILE}",
  "fixture_execution_log_file": "${EXEC_LOG_FILE}",
  "checks": ${checks_json},
  "results": [
    {
      "scenario_id": "${SCENARIO_ID}",
      "status": "${status}"
    }
  ]
}
EOF_JSON

echo "Parity waiver policy negative smoke status: ${status}"
echo "Parity waiver policy negative smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
