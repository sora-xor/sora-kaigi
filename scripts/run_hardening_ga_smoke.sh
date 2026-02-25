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
Usage: bash scripts/run_hardening_ga_smoke.sh [--out-dir <path>] [out_dir]
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
SUITE_ID="HARDENING-GA-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/hardening-ga-smoke.log"
REPORT_FILE="${OUT_DIR}/hardening-ga-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "OPS-005|hardening"
  "OPS-006|ga_approval"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""

for case in "${CASES[@]}"; do
  IFS='|' read -r scenario_id mode <<<"${case}"
  echo "=== ${scenario_id} :: ${mode} ===" | tee -a "${LOG_FILE}"
  case_status="passed"
  case "${mode}" in
    hardening)
      if ! python3 scripts/validate_hardening_gates.py 2>&1 | tee -a "${LOG_FILE}"; then
        case_status="failed"
        suite_status="failed"
      fi
      ;;
    ga_approval)
      if ! python3 scripts/validate_ga_approvals.py 2>&1 | tee -a "${LOG_FILE}"; then
        case_status="failed"
        suite_status="failed"
      fi
      ;;
    *)
      echo "unknown mode in suite: ${mode}" | tee -a "${LOG_FILE}"
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

echo "Hardening/GA smoke status: ${suite_status}"
echo "Hardening/GA smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
