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
Usage: bash scripts/run_client_ui_e2e_smoke.sh [--out-dir <path>] [out_dir]
Runs UI E2E-oriented client smoke suites for:
  - Web (Playwright browser UI tests via run_web_client_smoke.sh)
  - Android native (connectedDebugAndroidTest via run_native_android_smoke.sh)
  - Apple native (UI test bundles via run_native_apple_smoke.sh)
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

SUITE_ID="CLIENT-UI-E2E-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/client-ui-e2e-smoke.log"
REPORT_FILE="${OUT_DIR}/client-ui-e2e-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

append_result() {
  local suite_name="$1"
  local suite_status="$2"
  local suite_report="$3"
  local result_entry

  result_entry="{\"suite\":\"${suite_name}\",\"status\":\"${suite_status}\",\"report_file\":\"${suite_report}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${result_entry}"
  else
    results_json="${results_json},${result_entry}"
  fi
}

run_suite() {
  local suite_name="$1"
  local suite_report_basename="$2"
  shift 2
  local suite_status="passed"
  local suite_report_path="${OUT_DIR}/${suite_report_basename}"

  echo "=== ${suite_name} :: $* ===" | tee -a "${LOG_FILE}"
  if ! "$@" --out-dir "${OUT_DIR}" 2>&1 | tee -a "${LOG_FILE}"; then
    suite_status="failed"
    status="failed"
  fi

  append_result "${suite_name}" "${suite_status}" "${suite_report_path}"
}

run_suite "web_client" "web-client-smoke-report.json" bash "${SCRIPT_DIR}/run_web_client_smoke.sh"
run_suite "android_native" "native-android-smoke-report.json" bash "${SCRIPT_DIR}/run_native_android_smoke.sh"

if command -v xcodebuild >/dev/null 2>&1; then
  run_suite "apple_native" "native-apple-smoke-report.json" bash "${SCRIPT_DIR}/run_native_apple_smoke.sh" --platform all
else
  echo "xcodebuild not available; skipping Apple native suite" | tee -a "${LOG_FILE}"
  append_result "apple_native" "skipped" "${OUT_DIR}/native-apple-smoke-report.json"
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

echo "Client UI E2E smoke status: ${status}"
echo "Client UI E2E smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
