#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="target/conformance"
POSITIONAL_OUT_DIR=""
if [[ -n "${ALLOW_ANDROID_DEVICE_SKIPS+x}" ]]; then
  ALLOW_ANDROID_DEVICE_SKIPS="${ALLOW_ANDROID_DEVICE_SKIPS}"
else
  ALLOW_ANDROID_DEVICE_SKIPS="1"
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
    --help|-h)
      cat <<'EOF'
Usage: bash scripts/run_native_android_smoke.sh [--out-dir <path>] [out_dir]
Env:
  ALLOW_ANDROID_DEVICE_SKIPS=0   fail when no connected Android device/emulator is available
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

declare -a BUILD_SCENARIOS=(
  "ANDROID-BUILD-001|./gradlew --no-daemon :app:testReleaseUnitTest"
  "ANDROID-BUILD-002|./gradlew --no-daemon :app:bundleRelease"
  "ANDROID-BUILD-003|./gradlew --no-daemon :app:assembleDebug"
)

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

has_connected_android_device() {
  if ! command -v adb >/dev/null 2>&1; then
    return 1
  fi

  adb devices 2>/dev/null | awk 'NR > 1 && $2 == "device" { found = 1 } END { exit found ? 0 : 1 }'
}

for entry in "${BUILD_SCENARIOS[@]}"; do
  IFS='|' read -r scenario_id scenario_cmd <<<"${entry}"
  run_case "${scenario_id}" bash -lc "${scenario_cmd}"
done

ANDROID_UI_SCENARIO_ID="ANDROID-BUILD-004"
ANDROID_UI_SCENARIO_CMD="./gradlew --no-daemon :app:connectedDebugAndroidTest"

if has_connected_android_device; then
  run_case "${ANDROID_UI_SCENARIO_ID}" bash -lc "${ANDROID_UI_SCENARIO_CMD}"
elif [[ "${ALLOW_ANDROID_DEVICE_SKIPS}" == "1" ]]; then
  echo "warning: no connected Android device/emulator found; skipping instrumentation UI suite" | tee -a "${LOG_FILE}"
  append_result "${ANDROID_UI_SCENARIO_ID}" "skipped"
else
  echo "error: no connected Android device/emulator found; cannot run instrumentation UI suite" | tee -a "${LOG_FILE}"
  status="failed"
  append_result "${ANDROID_UI_SCENARIO_ID}" "failed"
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

echo "Native Android smoke status: ${status}"
echo "Native Android smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
