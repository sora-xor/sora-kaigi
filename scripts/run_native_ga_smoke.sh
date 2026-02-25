#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

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
Usage: bash scripts/run_native_ga_smoke.sh [--out-dir <path>] [out_dir]
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

SUITE_ID="NATIVE-GA-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/native-ga-smoke.log"
REPORT_FILE="${OUT_DIR}/native-ga-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

run_suite() {
  local suite_name="$1"
  shift
  local suite_status="passed"

  echo "=== ${suite_name} :: $* ===" | tee -a "${LOG_FILE}"
  if ! "$@" --out-dir "${OUT_DIR}" 2>&1 | tee -a "${LOG_FILE}"; then
    suite_status="failed"
    status="failed"
  fi

  local entry
  entry="{\"suite\":\"${suite_name}\",\"status\":\"${suite_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${entry}"
  else
    results_json="${results_json},${entry}"
  fi
}

run_suite "android_native" bash "${SCRIPT_DIR}/run_native_android_smoke.sh"

if command -v xcodebuild >/dev/null 2>&1; then
  run_suite "apple_native" bash "${SCRIPT_DIR}/run_native_apple_smoke.sh"
else
  echo "xcodebuild not available; skipping Apple native suite" | tee -a "${LOG_FILE}"
  entry="{\"suite\":\"apple_native\",\"status\":\"skipped\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${entry}"
  else
    results_json="${results_json},${entry}"
  fi
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
  "results": [${results_json}]
}
EOF_JSON

echo "Native GA smoke status: ${status}"
echo "Native GA smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
