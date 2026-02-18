#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="PARITY-DOWNGRADE-GUARD-SMOKE"
SCENARIO_ID="PARITY-005"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/parity-downgrade-guard-smoke.log"
REPORT_FILE="${OUT_DIR}/parity-downgrade-guard-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== ${SCENARIO_ID} :: validate parity downgrade guard ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_parity_downgrade_guard.py 2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
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
  "results": [
    {
      "scenario_id": "${SCENARIO_ID}",
      "status": "${status}"
    }
  ]
}
EOF_JSON

echo "Parity downgrade guard smoke status: ${status}"
echo "Parity downgrade guard smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
