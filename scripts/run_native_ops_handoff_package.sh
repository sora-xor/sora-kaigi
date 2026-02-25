#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="target/conformance"
METADATA_FILE="docs/client-release-manifest-input.template.json"
EXISTING_MANIFEST="docs/client-release-manifest.json"
SKIP_CONFORMANCE_REFRESH=0

usage() {
  cat <<'EOF'
Usage: bash scripts/run_native_ops_handoff_package.sh [options]

Options:
  --out-dir <path>                  Output directory for logs/reports (default: target/conformance)
  --metadata <path>                 Metadata input JSON for manifest generation
                                    (default: docs/client-release-manifest-input.template.json)
  --existing-manifest <path>        Existing manifest used for defaults/backfill
                                    (default: docs/client-release-manifest.json)
  --skip-conformance-refresh        Skip full conformance refresh and package from existing evidence
  --help                            Show this help message
EOF
}

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
    --metadata)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --metadata requires a value" >&2
        exit 2
      fi
      METADATA_FILE="$2"
      shift 2
      ;;
    --existing-manifest)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --existing-manifest requires a value" >&2
        exit 2
      fi
      EXISTING_MANIFEST="$2"
      shift 2
      ;;
    --skip-conformance-refresh)
      SKIP_CONFORMANCE_REFRESH=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

SUITE_ID="NATIVE-OPS-HANDOFF-PACKAGE"
SCENARIO_PREFIX="OPS-007"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/native-ops-handoff-package.log"
REPORT_FILE="${OUT_DIR}/native-ops-handoff-package-report.json"
PACKAGE_DIR="${OUT_DIR}/native-ops-handoff-package"
GENERATED_MANIFEST_FILE="${OUT_DIR}/client-release-manifest.handoff.generated.json"
TARBALL_FILE="${OUT_DIR}/native-ops-handoff-package.tar.gz"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

append_result() {
  local scenario_id="$1"
  local scenario_status="$2"
  local entry
  entry="{\"scenario_id\":\"${scenario_id}\",\"status\":\"${scenario_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${entry}"
  else
    results_json="${results_json},${entry}"
  fi
}

run_cmd() {
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

build_handoff_package() {
  local now
  local checksum_tool
  now="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  if command -v shasum >/dev/null 2>&1; then
    checksum_tool="shasum"
  elif command -v sha256sum >/dev/null 2>&1; then
    checksum_tool="sha256sum"
  else
    echo "missing checksum tool: expected shasum or sha256sum" >&2
    return 1
  fi

  rm -rf "${PACKAGE_DIR}"
  mkdir -p "${PACKAGE_DIR}"

  local -a required_files=(
    "${OUT_DIR}/release-readiness-report.json"
    "${OUT_DIR}/release-readiness.log"
    "${OUT_DIR}/hardening-ga-smoke-report.json"
    "${GENERATED_MANIFEST_FILE}"
    "docs/client-release-manifest.json"
    "docs/client-fallback-drill-results.json"
  )
  local -a missing_files=()
  local required_file
  for required_file in "${required_files[@]}"; do
    if [[ ! -f "${required_file}" ]]; then
      missing_files+=("${required_file}")
    fi
  done

  if [[ ${#missing_files[@]} -gt 0 ]]; then
    echo "missing required handoff artifacts:" >&2
    printf '  - %s\n' "${missing_files[@]}" >&2
    echo "Hint: rerun without --skip-conformance-refresh for a full evidence refresh." >&2
    return 1
  fi

  cp "${OUT_DIR}/release-readiness-report.json" "${PACKAGE_DIR}/"
  cp "${OUT_DIR}/release-readiness.log" "${PACKAGE_DIR}/"
  cp "${OUT_DIR}/hardening-ga-smoke-report.json" "${PACKAGE_DIR}/"
  cp "${GENERATED_MANIFEST_FILE}" "${PACKAGE_DIR}/client-release-manifest.generated.json"
  cp "docs/client-release-manifest.json" "${PACKAGE_DIR}/client-release-manifest.canonical.json"
  cp "docs/client-fallback-drill-results.json" "${PACKAGE_DIR}/"
  if [[ -f "${OUT_DIR}/conformance-evidence-index.md" ]]; then
    cp "${OUT_DIR}/conformance-evidence-index.md" "${PACKAGE_DIR}/"
  fi
  if [[ -f "${OUT_DIR}/conformance-evidence-bundle-report.json" ]]; then
    cp "${OUT_DIR}/conformance-evidence-bundle-report.json" "${PACKAGE_DIR}/"
  fi

  cat >"${PACKAGE_DIR}/HANDOFF-NOTES.txt" <<EOF_NOTES
generated_at: ${now}
suite_id: ${SUITE_ID}
metadata_file: ${METADATA_FILE}
existing_manifest: ${EXISTING_MANIFEST}
generated_manifest: ${GENERATED_MANIFEST_FILE}
source_conformance_dir: ${OUT_DIR}
EOF_NOTES

  (
    cd "${PACKAGE_DIR}"
    : > "SHA256SUMS"
    while IFS= read -r -d '' file; do
      if [[ "${checksum_tool}" == "shasum" ]]; then
        shasum -a 256 "${file}" >> "SHA256SUMS"
      else
        sha256sum "${file}" >> "SHA256SUMS"
      fi
    done < <(find . -maxdepth 1 -type f ! -name "SHA256SUMS" -print0 | LC_ALL=C sort -z)
  )

  rm -f "${TARBALL_FILE}"
  tar -C "${OUT_DIR}" -czf "${TARBALL_FILE}" "$(basename "${PACKAGE_DIR}")"
}

if [[ "${SKIP_CONFORMANCE_REFRESH}" == "1" ]]; then
  echo "=== ${SCENARIO_PREFIX}-001 :: skipped conformance evidence refresh (--skip-conformance-refresh) ===" | tee -a "${LOG_FILE}"
  append_result "${SCENARIO_PREFIX}-001" "skipped"
else
  run_cmd "${SCENARIO_PREFIX}-001" bash "${SCRIPT_DIR}/run_conformance_evidence_bundle.sh" "${OUT_DIR}"
fi

run_cmd "${SCENARIO_PREFIX}-002" python3 "${SCRIPT_DIR}/generate_client_release_manifest.py" \
  --metadata "${METADATA_FILE}" \
  --existing-manifest "${EXISTING_MANIFEST}" \
  --output "${GENERATED_MANIFEST_FILE}"
run_cmd "${SCENARIO_PREFIX}-003" python3 "${SCRIPT_DIR}/validate_client_release_manifest.py" --manifest "${GENERATED_MANIFEST_FILE}"
run_cmd "${SCENARIO_PREFIX}-004" bash "${SCRIPT_DIR}/run_client_release_manifest_smoke.sh" "${OUT_DIR}"
run_cmd "${SCENARIO_PREFIX}-005" bash "${SCRIPT_DIR}/run_release_readiness_smoke.sh" "${OUT_DIR}" --assume-passed "OPS-007"
run_cmd "${SCENARIO_PREFIX}-006" bash "${SCRIPT_DIR}/run_hardening_ga_smoke.sh" "${OUT_DIR}"
run_cmd "${SCENARIO_PREFIX}-007" build_handoff_package

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
  "metadata_file": "${METADATA_FILE}",
  "existing_manifest": "${EXISTING_MANIFEST}",
  "generated_manifest_file": "${GENERATED_MANIFEST_FILE}",
  "package_dir": "${PACKAGE_DIR}",
  "package_tarball": "${TARBALL_FILE}",
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF_JSON

echo "Native ops handoff package status: ${status}"
echo "Native ops handoff package report: ${REPORT_FILE}"
echo "Native ops handoff package directory: ${PACKAGE_DIR}"
echo "Native ops handoff package tarball: ${TARBALL_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
