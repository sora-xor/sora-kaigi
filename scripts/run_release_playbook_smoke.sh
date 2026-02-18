#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="RELEASE-PLAYBOOK-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/release-playbook-smoke.log"
REPORT_FILE="${OUT_DIR}/release-playbook-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "OPS-001|release"
  "OPS-002|rollback"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""

for case in "${CASES[@]}"; do
  IFS='|' read -r scenario_id mode <<<"${case}"
  echo "=== ${scenario_id} :: validate_release_playbooks.py --mode ${mode} ===" | tee -a "${LOG_FILE}"
  case_status="passed"
  if ! python3 scripts/validate_release_playbooks.py --mode "${mode}" 2>&1 | tee -a "${LOG_FILE}"; then
    case_status="failed"
    suite_status="failed"
  fi

  entry="{\"scenario_id\":\"${scenario_id}\",\"status\":\"${case_status}\",\"mode\":\"${mode}\"}"
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

echo "Release playbook smoke status: ${suite_status}"
echo "Release playbook smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
