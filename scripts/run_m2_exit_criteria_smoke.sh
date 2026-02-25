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
Usage: bash scripts/run_m2_exit_criteria_smoke.sh [--out-dir <path>] [out_dir]
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
