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
Usage: bash scripts/run_client_release_manifest_smoke.sh [--out-dir <path>] [out_dir]
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
SUITE_ID="CLIENT-RELEASE-MANIFEST-SMOKE"
SCENARIO_ID="PLATFORM-012"
MANIFEST_TEMPLATE="docs/client-release-manifest-input.template.json"
GENERATED_MANIFEST_FILE="${OUT_DIR}/client-release-manifest.generated.json"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/client-release-manifest-smoke.log"
REPORT_FILE="${OUT_DIR}/client-release-manifest-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"

echo "=== ${SCENARIO_ID} :: generate client release manifest candidate ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/generate_client_release_manifest.py \
  --metadata "${MANIFEST_TEMPLATE}" \
  --existing-manifest docs/client-release-manifest.json \
  --output "${GENERATED_MANIFEST_FILE}" \
  2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

echo "=== ${SCENARIO_ID} :: validate generated client release manifest ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_client_release_manifest.py --manifest "${GENERATED_MANIFEST_FILE}" 2>&1 | tee -a "${LOG_FILE}"; then
  status="failed"
fi

echo "=== ${SCENARIO_ID} :: validate canonical client release manifest ===" | tee -a "${LOG_FILE}"
if ! python3 scripts/validate_client_release_manifest.py 2>&1 | tee -a "${LOG_FILE}"; then
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
  "generated_manifest_file": "${GENERATED_MANIFEST_FILE}",
  "results": [
    {
      "scenario_id": "${SCENARIO_ID}",
      "status": "${status}"
    }
  ]
}
EOF_JSON

echo "Client release manifest smoke status: ${status}"
echo "Client release manifest smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
