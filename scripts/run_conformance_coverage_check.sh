#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="target/conformance"
TEST_PLAN="docs/test-plan.md"
REPORT_FILE=""
LOG_FILE=""
declare -a FORWARD_ARGS=()

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
    --test-plan)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --test-plan requires a value" >&2
        exit 2
      fi
      TEST_PLAN="$2"
      shift 2
      ;;
    --report-file)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --report-file requires a value" >&2
        exit 2
      fi
      REPORT_FILE="$2"
      shift 2
      ;;
    --log-file)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --log-file requires a value" >&2
        exit 2
      fi
      LOG_FILE="$2"
      shift 2
      ;;
    --help|-h)
      cat <<'EOF'
Usage: bash scripts/run_conformance_coverage_check.sh [--out-dir <path>] [--test-plan <path>] [--report-file <path>] [--log-file <path>] [extra validate_conformance_coverage.py args]
EOF
      exit 0
      ;;
    *)
      FORWARD_ARGS+=("$1")
      shift
      ;;
  esac
done

if [[ -z "${REPORT_FILE}" ]]; then
  REPORT_FILE="${OUT_DIR}/conformance-coverage-report.json"
fi
if [[ -z "${LOG_FILE}" ]]; then
  LOG_FILE="${OUT_DIR}/conformance-coverage.log"
fi

mkdir -p "${OUT_DIR}"
mkdir -p "$(dirname "${REPORT_FILE}")"
mkdir -p "$(dirname "${LOG_FILE}")"

if [[ ${#FORWARD_ARGS[@]} -gt 0 ]]; then
  python3 scripts/validate_conformance_coverage.py \
    --test-plan "${TEST_PLAN}" \
    --reports-dir "${OUT_DIR}" \
    --report-file "${REPORT_FILE}" \
    --log-file "${LOG_FILE}" \
    "${FORWARD_ARGS[@]}"
else
  python3 scripts/validate_conformance_coverage.py \
    --test-plan "${TEST_PLAN}" \
    --reports-dir "${OUT_DIR}" \
    --report-file "${REPORT_FILE}" \
    --log-file "${LOG_FILE}"
fi
