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
Usage: bash scripts/run_linux_client_smoke.sh [--out-dir <path>] [out_dir]
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

SUITE_ID="LINUX-CLIENT-SMOKE"
mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/linux-client-smoke.log"
REPORT_FILE="${OUT_DIR}/linux-client-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

declare -a SCENARIOS=(
  "LINUX-BUILD-001|cargo test -p kaigi-linux-client"
  "LINUX-BUILD-002|cargo build --release -p kaigi-linux-client"
  "LINUX-BUILD-003|cargo run -p kaigi-linux-client --quiet"
)

for entry in "${SCENARIOS[@]}"; do
  IFS='|' read -r scenario_id scenario_cmd <<<"${entry}"
  scenario_status="passed"

  echo "=== ${scenario_id} :: ${scenario_cmd} ===" | tee -a "${LOG_FILE}"
  if ! eval "${scenario_cmd}" 2>&1 | tee -a "${LOG_FILE}"; then
    scenario_status="failed"
    status="failed"
  fi

  result_entry="{\"scenario_id\":\"${scenario_id}\",\"status\":\"${scenario_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${result_entry}"
  else
    results_json="${results_json},${result_entry}"
  fi
done

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

echo "Linux client smoke status: ${status}"
echo "Linux client smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
