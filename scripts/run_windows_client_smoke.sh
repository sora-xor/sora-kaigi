#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="target/conformance"
BUILD_ONLY=0
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
    --build-only)
      BUILD_ONLY=1
      shift
      ;;
    --help|-h)
      cat <<'EOF'
Usage: bash scripts/run_windows_client_smoke.sh [--out-dir <path>] [--build-only] [out_dir]
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

SUITE_ID="WINDOWS-CLIENT-SMOKE"
mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/windows-client-smoke.log"
REPORT_FILE="${OUT_DIR}/windows-client-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

append_result() {
  local scenario_id="$1"
  local scenario_status="$2"
  local result_entry
  result_entry="{\"scenario_id\":\"${scenario_id}\",\"status\":\"${scenario_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${result_entry}"
  else
    results_json="${results_json},${result_entry}"
  fi
}

run_case() {
  local scenario_id="$1"
  shift
  local scenario_status="passed"
  echo "=== ${scenario_id} :: $* ===" | tee -a "${LOG_FILE}"
  if ! "$@" 2>&1 | tee -a "${LOG_FILE}"; then
    scenario_status="failed"
    status="failed"
  fi
  append_result "${scenario_id}" "${scenario_status}"
}

host_os="$(uname -s)"
if [[ "${host_os}" != "MINGW"* && "${host_os}" != "MSYS"* && "${host_os}" != "CYGWIN"* ]]; then
  echo "info: non-Windows host (${host_os}); skipping Windows build/test scenarios" | tee -a "${LOG_FILE}"
  append_result "WINDOWS-BUILD-001" "skipped"
  append_result "WINDOWS-BUILD-002" "skipped"
  append_result "WINDOWS-BUILD-003" "skipped"
else
  if ! command -v dotnet >/dev/null 2>&1; then
    echo "error: dotnet CLI is required on Windows hosts" | tee -a "${LOG_FILE}"
    append_result "WINDOWS-BUILD-001" "failed"
    append_result "WINDOWS-BUILD-002" "failed"
    append_result "WINDOWS-BUILD-003" "failed"
    status="failed"
  else
    run_case "WINDOWS-BUILD-001" dotnet restore clients/windows/Kaigi.Windows.sln
    run_case "WINDOWS-BUILD-002" dotnet build clients/windows/Kaigi.Windows.sln -c Release
    if [[ "${BUILD_ONLY}" == "1" ]]; then
      append_result "WINDOWS-BUILD-003" "skipped"
    else
      run_case "WINDOWS-BUILD-003" dotnet test clients/windows/Kaigi.Windows.sln -c Release --no-build
    fi
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

echo "Windows client smoke status: ${status}"
echo "Windows client smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
