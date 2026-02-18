#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="CLIENT-APP-WORKSPACES-SMOKE"
SCENARIO_ID="PLATFORM-007"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/client-app-workspaces-smoke.log"
REPORT_FILE="${OUT_DIR}/client-app-workspaces-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== ${SCENARIO_ID} :: validate client app workspaces ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_client_app_workspaces.py 2>&1 | tee -a "${LOG_FILE}"; then
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

echo "Client app workspaces smoke status: ${status}"
echo "Client app workspaces smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
