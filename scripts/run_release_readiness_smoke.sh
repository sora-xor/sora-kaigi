#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="target/conformance"
declare -a EXTRA_ASSUME_PASSED=()

if [[ $# -gt 0 && "${1}" != --* ]]; then
  OUT_DIR="$1"
  shift
fi

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
    --assume-passed)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --assume-passed requires a scenario id value" >&2
        exit 2
      fi
      EXTRA_ASSUME_PASSED+=("$2")
      shift 2
      ;;
    --help|-h)
      cat <<'EOF'
Usage: bash scripts/run_release_readiness_smoke.sh [--out-dir <path>] [--assume-passed <scenario_id>] [out_dir]
EOF
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

SUITE_ID="RELEASE-READINESS-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/release-readiness-smoke.log"
REPORT_FILE="${OUT_DIR}/release-readiness-smoke-report.json"
READINESS_REPORT_FILE="${OUT_DIR}/release-readiness-report.json"
READINESS_LOG_FILE="${OUT_DIR}/release-readiness.log"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a SCENARIOS=(
  "OPS-003"
  "OPS-004"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""
declare -a ASSUME_ARGS=(
  --assume-passed OPS-003
  --assume-passed OPS-004
)
if [[ ${#EXTRA_ASSUME_PASSED[@]} -gt 0 ]]; then
  for assumed in "${EXTRA_ASSUME_PASSED[@]}"; do
    ASSUME_ARGS+=(--assume-passed "${assumed}")
  done
fi

for scenario_id in "${SCENARIOS[@]}"; do
  case_label=""
  case_status="passed"

  case "${scenario_id}" in
    OPS-003)
      case_label="python3 scripts/validate_critical_defects.py"
      echo "=== ${scenario_id} :: ${case_label} ===" | tee -a "${LOG_FILE}"
      if ! python3 scripts/validate_critical_defects.py 2>&1 | tee -a "${LOG_FILE}"; then
        case_status="failed"
        suite_status="failed"
      fi
      ;;
    OPS-004)
      case_label="python3 scripts/generate_release_readiness_report.py"
      echo "=== ${scenario_id} :: ${case_label} ===" | tee -a "${LOG_FILE}"
      if ! python3 scripts/generate_release_readiness_report.py \
        --reports-dir "${OUT_DIR}" \
        --output-report "${READINESS_REPORT_FILE}" \
        --log-file "${READINESS_LOG_FILE}" \
        "${ASSUME_ARGS[@]}" \
        2>&1 | tee -a "${LOG_FILE}"; then
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
  "readiness_report_file": "${READINESS_REPORT_FILE}",
  "readiness_log_file": "${READINESS_LOG_FILE}",
  "results": [${results_json}]
}
EOF

echo "Release readiness smoke status: ${suite_status}"
echo "Release readiness smoke report: ${REPORT_FILE}"
echo "Release readiness detail report: ${READINESS_REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
