#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="M2-EXIT-CRITERIA-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/m2-exit-criteria-smoke.log"
REPORT_FILE="${OUT_DIR}/m2-exit-criteria-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a SCENARIOS=(
  "HDR-004"
  "HDR-005"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""

for scenario_id in "${SCENARIOS[@]}"; do
  case_label=""
  case_status="passed"

  case "${scenario_id}" in
    HDR-004)
      case_label="python3 scripts/validate_hdr_target_device_results.py"
      echo "=== ${scenario_id} :: ${case_label} ===" | tee -a "${LOG_FILE}"
      if ! python3 scripts/validate_hdr_target_device_results.py 2>&1 | tee -a "${LOG_FILE}"; then
        case_status="failed"
        suite_status="failed"
      fi
      ;;
    HDR-005)
      case_label="python3 scripts/validate_platform_blockers.py"
      echo "=== ${scenario_id} :: ${case_label} ===" | tee -a "${LOG_FILE}"
      if ! python3 scripts/validate_platform_blockers.py 2>&1 | tee -a "${LOG_FILE}"; then
        case_status="failed"
        suite_status="failed"
      fi
      ;;
    *)
      echo "unknown scenario id in suite: ${scenario_id}" | tee -a "${LOG_FILE}"
      case_status="failed"
      suite_status="failed"
      ;;
  esac

  entry="{\"scenario_id\":\"${scenario_id}\",\"status\":\"${case_status}\"}"
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

echo "M2 exit criteria smoke status: ${suite_status}"
echo "M2 exit criteria smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
