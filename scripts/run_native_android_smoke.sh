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
Usage: bash scripts/run_native_android_smoke.sh [--out-dir <path>] [out_dir]
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

if [[ -z "${GRADLE_USER_HOME:-}" ]]; then
  export GRADLE_USER_HOME="${REPO_ROOT}/.gradle"
fi
mkdir -p "${GRADLE_USER_HOME}"

bootstrap_gradle_wrapper_cache() {
  local local_dists="${GRADLE_USER_HOME}/wrapper/dists"
  if [[ -d "${local_dists}" ]]; then
    find "${local_dists}" -type f -name "*.lck" -delete 2>/dev/null || true
    return
  fi

  local global_dists="${HOME:-}/.gradle/wrapper/dists"
  if [[ -d "${global_dists}" ]]; then
    mkdir -p "${GRADLE_USER_HOME}/wrapper"
    cp -R "${global_dists}" "${local_dists}" 2>/dev/null || true
    find "${local_dists}" -type f -name "*.lck" -delete 2>/dev/null || true
  fi
}

bootstrap_gradle_wrapper_cache

SUITE_ID="NATIVE-ANDROID-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/native-android-smoke.log"
REPORT_FILE="${OUT_DIR}/native-android-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

echo "Using GRADLE_USER_HOME=${GRADLE_USER_HOME}" | tee -a "${LOG_FILE}"

declare -a SCENARIOS=(
  "ANDROID-BUILD-001|./gradlew --no-daemon :app:testReleaseUnitTest"
  "ANDROID-BUILD-002|./gradlew --no-daemon :app:bundleRelease"
  "ANDROID-BUILD-003|./gradlew --no-daemon :app:assembleDebug"
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

echo "Native Android smoke status: ${status}"
echo "Native Android smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
