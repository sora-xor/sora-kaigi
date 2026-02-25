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
Usage: bash scripts/run_native_ops_handoff_package_smoke.sh [--out-dir <path>] [out_dir]
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

SUITE_ID="NATIVE-OPS-HANDOFF-PACKAGE-SMOKE"
SCENARIO_ID="OPS-007"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/native-ops-handoff-package-smoke.log"
REPORT_FILE="${OUT_DIR}/native-ops-handoff-package-smoke-report.json"
HANDOFF_REPORT_FILE="${OUT_DIR}/native-ops-handoff-package-report.json"
HANDOFF_TARBALL_FILE="${OUT_DIR}/native-ops-handoff-package.tar.gz"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== ${SCENARIO_ID} :: build native ops handoff package ===" | tee -a "${LOG_FILE}"
if ! bash "${SCRIPT_DIR}/run_native_ops_handoff_package.sh" \
  --out-dir "${OUT_DIR}" \
  --skip-conformance-refresh \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
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
  "handoff_report_file": "${HANDOFF_REPORT_FILE}",
  "handoff_tarball_file": "${HANDOFF_TARBALL_FILE}",
  "results": [
    {
      "scenario_id": "${SCENARIO_ID}",
      "status": "${status}"
    }
  ]
}
EOF_JSON

echo "Native ops handoff package smoke status: ${status}"
echo "Native ops handoff package smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
