#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="PARITY-GA-SMOKE"
SCENARIO_ID="PARITY-004"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/parity-ga-smoke.log"
REPORT_FILE="${OUT_DIR}/parity-ga-smoke-report.json"
COVERAGE_REPORT_FILE="${OUT_DIR}/conformance-coverage-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

if [[ ! -f "${COVERAGE_REPORT_FILE}" ]]; then
  echo "=== bootstrap :: scripts/run_conformance_coverage_check.sh ===" | tee -a "${LOG_FILE}"
  if ! bash scripts/run_conformance_coverage_check.sh \
    "${OUT_DIR}" \
    --allow-missing-scenario "${SCENARIO_ID}" \
    --allow-missing-scenario "PARITY-005" \
    --allow-missing-scenario "PARITY-006" \
    --allow-missing-scenario "PARITY-007" \
    --allow-missing-scenario "PARITY-008" \
    --allow-missing-scenario "PARITY-009" \
    2>&1 | tee -a "${LOG_FILE}"; then
    status="failed"
  fi
fi

echo "=== ${SCENARIO_ID} :: validate parity GA gate ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_parity_ga_gate.py \
  --coverage-report "${COVERAGE_REPORT_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
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

echo "Parity GA smoke status: ${status}"
echo "Parity GA smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
