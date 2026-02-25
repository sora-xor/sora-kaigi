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
Usage: bash scripts/run_parity_readiness_smoke.sh [--out-dir <path>] [out_dir]
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

SUITE_ID="PARITY-READINESS-SMOKE"
SCENARIO_ID="PARITY-002"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/parity-readiness-smoke.log"
REPORT_FILE="${OUT_DIR}/parity-readiness-smoke-report.json"
READINESS_REPORT_FILE="${OUT_DIR}/parity-readiness-report.json"
READINESS_LOG_FILE="${OUT_DIR}/parity-readiness.log"
COVERAGE_REPORT_FILE="${OUT_DIR}/parity-readiness-bootstrap-coverage-report.json"
COVERAGE_LOG_FILE="${OUT_DIR}/parity-readiness-bootstrap-coverage.log"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== bootstrap :: scripts/run_conformance_coverage_check.sh ===" | tee -a "${LOG_FILE}"
if ! bash scripts/run_conformance_coverage_check.sh \
  "${OUT_DIR}" \
  --allow-missing-scenario "${SCENARIO_ID}" \
  --allow-missing-scenario "OPS-007" \
  --allow-missing-scenario "PARITY-003" \
  --allow-missing-scenario "PARITY-004" \
  --allow-missing-scenario "PARITY-005" \
  --allow-missing-scenario "PARITY-006" \
  --allow-missing-scenario "PARITY-007" \
  --allow-missing-scenario "PARITY-008" \
  --allow-missing-scenario "PARITY-009" \
  --allow-failed-scenario "${SCENARIO_ID}" \
  --allow-failed-scenario "OPS-007" \
  --allow-failed-scenario "PARITY-003" \
  --allow-failed-scenario "PARITY-004" \
  --allow-failed-scenario "PARITY-005" \
  --allow-failed-scenario "PARITY-006" \
  --allow-failed-scenario "PARITY-007" \
  --allow-failed-scenario "PARITY-008" \
  --allow-failed-scenario "PARITY-009" \
  --report-file "${COVERAGE_REPORT_FILE}" \
  --log-file "${COVERAGE_LOG_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

echo "=== ${SCENARIO_ID} :: generate parity readiness report ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/generate_parity_readiness_report.py \
  --coverage-report "${COVERAGE_REPORT_FILE}" \
  --output-report "${READINESS_REPORT_FILE}" \
  --log-file "${READINESS_LOG_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF
{
  "suite_id": "${SUITE_ID}",
  "status": "${status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "coverage_report_file": "${COVERAGE_REPORT_FILE}",
  "coverage_log_file": "${COVERAGE_LOG_FILE}",
  "readiness_report_file": "${READINESS_REPORT_FILE}",
  "readiness_log_file": "${READINESS_LOG_FILE}",
  "results": [
    {
      "scenario_id": "${SCENARIO_ID}",
      "status": "${status}"
    }
  ]
}
EOF

echo "Parity readiness smoke status: ${status}"
echo "Parity readiness smoke report: ${REPORT_FILE}"
echo "Parity readiness detail report: ${READINESS_REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
