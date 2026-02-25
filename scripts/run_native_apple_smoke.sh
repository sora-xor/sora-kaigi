#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="target/conformance"
POSITIONAL_OUT_DIR=""
PLATFORM="all"
GENERATE_PROJECT=1
SKIP_VISIONOS_REASON=""
if [[ -n "${SKIP_VISIONOS+x}" ]]; then
  SKIP_VISIONOS="${SKIP_VISIONOS}"
elif [[ "${CI:-}" == "true" ]]; then
  SKIP_VISIONOS="1"
  SKIP_VISIONOS_REASON="ci-default"
else
  SKIP_VISIONOS="0"
fi

if [[ -n "${ALLOW_SIMULATOR_SKIPS+x}" ]]; then
  ALLOW_SIMULATOR_SKIPS="${ALLOW_SIMULATOR_SKIPS}"
elif [[ "${CI:-}" == "true" ]]; then
  ALLOW_SIMULATOR_SKIPS="0"
else
  ALLOW_SIMULATOR_SKIPS="1"
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
Usage: bash scripts/run_native_apple_smoke.sh [--out-dir <path>] [--platform all|macos|ios|ipados|visionos] [--skip-xcodegen] [out_dir]
EOF
      exit 0
      ;;
    --platform)
      if [[ $# -lt 2 || -z "${2:-}" || "${2}" == -* ]]; then
        echo "error: --platform requires a value" >&2
        exit 2
      fi
      PLATFORM="$2"
      shift 2
      ;;
    --skip-xcodegen)
      GENERATE_PROJECT=0
      shift
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

if [[ "${PLATFORM}" != "all" && "${PLATFORM}" != "macos" && "${PLATFORM}" != "ios" && "${PLATFORM}" != "ipados" && "${PLATFORM}" != "visionos" ]]; then
  echo "Unsupported --platform value: ${PLATFORM}" >&2
  exit 2
fi

if [[ "${SKIP_VISIONOS}" == "1" && "${PLATFORM}" == "visionos" ]]; then
  echo "error: SKIP_VISIONOS=1 cannot be used with --platform visionos" >&2
  exit 2
fi

if ! command -v xcodebuild >/dev/null 2>&1; then
  echo "error: xcodebuild is required for native Apple smoke" >&2
  exit 1
fi

if [[ "${GENERATE_PROJECT}" == "1" ]]; then
  if command -v xcodegen >/dev/null 2>&1; then
    xcodegen generate --spec Kaigi.yml >/dev/null
  elif [[ ! -f Kaigi.xcodeproj/project.pbxproj ]]; then
    echo "error: xcodegen is unavailable and Kaigi.xcodeproj is missing" >&2
    exit 2
  fi
fi

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/native-apple-smoke.log"
REPORT_FILE="${OUT_DIR}/native-apple-smoke-report.json"
SUITE_ID="NATIVE-APPLE-SMOKE"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

: >"${LOG_FILE}"
status="passed"
results_json=""

find_sim_udid() {
  local runtime_pattern="$1"
  local name_pattern="$2"
  python3 - "$runtime_pattern" "$name_pattern" <<'PY'
import json
import subprocess
import sys

runtime_pattern = sys.argv[1].lower()
name_pattern = sys.argv[2].lower()
try:
    payload = subprocess.check_output(
        ["xcrun", "simctl", "list", "devices", "available", "--json"],
        text=True,
        stderr=subprocess.DEVNULL,
    )
except Exception:
    print("")
    raise SystemExit(0)
devices = json.loads(payload).get("devices", {})

for runtime, entries in devices.items():
    if runtime_pattern not in runtime.lower():
        continue
    for entry in entries:
        if not entry.get("isAvailable", True):
            continue
        name = entry.get("name", "")
        if name_pattern in name.lower():
            udid = entry.get("udid")
            if udid:
                print(udid)
                raise SystemExit(0)
PY
}

has_available_vision_runtime() {
  python3 <<'PY'
import json
import subprocess
import sys

try:
    payload = subprocess.check_output(
        ["xcrun", "simctl", "list", "runtimes", "available", "--json"],
        text=True,
        stderr=subprocess.DEVNULL,
    )
except Exception:
    raise SystemExit(1)
runtimes = json.loads(payload).get("runtimes", [])

for runtime in runtimes:
    if not runtime.get("isAvailable", True):
        continue
    identifier = str(runtime.get("identifier", "")).lower()
    name = str(runtime.get("name", "")).lower()
    if "visionos" in identifier or "visionos" in name:
        raise SystemExit(0)

raise SystemExit(1)
PY
}

ios_udid=""
ipad_udid=""
vision_udid=""
vision_runtime_available=1
ios_suite_skip=0
ipados_suite_skip=0
vision_suite_skip=0

requires_simulator=0
if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ios" || "${PLATFORM}" == "ipados" ]]; then
  requires_simulator=1
fi
if [[ "${SKIP_VISIONOS}" != "1" && ( "${PLATFORM}" == "all" || "${PLATFORM}" == "visionos" ) ]]; then
  requires_simulator=1
fi

if [[ "${requires_simulator}" == "1" ]]; then
  if ! xcrun simctl list devices available --json >/dev/null 2>&1; then
    if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
      echo "warning: CoreSimulatorService unavailable; skipping iOS/iPadOS/visionOS suites (ALLOW_SIMULATOR_SKIPS=1)" | tee -a "${LOG_FILE}"
      ios_suite_skip=1
      ipados_suite_skip=1
      vision_suite_skip=1
      vision_runtime_available=0
    else
      echo "error: CoreSimulatorService unavailable; cannot run requested Apple simulator suite(s)" | tee -a "${LOG_FILE}"
      status="failed"
    fi
  fi
fi

if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ios" ]]; then
  if [[ "${ios_suite_skip}" != "1" ]]; then
    ios_udid="$(find_sim_udid "ios" "iphone")"
    if [[ -z "${ios_udid}" ]]; then
      if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
        echo "warning: no available iPhone simulator found; skipping iOS suite" | tee -a "${LOG_FILE}"
        ios_suite_skip=1
      else
        echo "error: no available iPhone simulator found" | tee -a "${LOG_FILE}"
        status="failed"
      fi
    fi
  fi
fi

if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ipados" ]]; then
  if [[ "${ipados_suite_skip}" != "1" ]]; then
    ipad_udid="$(find_sim_udid "ios" "ipad")"
    if [[ -z "${ipad_udid}" ]]; then
      if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
        echo "warning: no available iPad simulator found; skipping iPadOS suite" | tee -a "${LOG_FILE}"
        ipados_suite_skip=1
      else
        echo "error: no available iPad simulator found" | tee -a "${LOG_FILE}"
        status="failed"
      fi
    fi
  fi
fi

if [[ "${SKIP_VISIONOS}" != "1" && ( "${PLATFORM}" == "all" || "${PLATFORM}" == "visionos" ) ]]; then
  if [[ "${vision_suite_skip}" != "1" ]]; then
    if ! has_available_vision_runtime; then
      vision_runtime_available=0
      if [[ "${PLATFORM}" == "visionos" ]]; then
        echo "error: visionOS simulator runtime is not installed; install it from Xcode > Settings > Components" | tee -a "${LOG_FILE}"
        status="failed"
      else
        echo "warning: visionOS simulator runtime is not installed; skipping visionOS suite" | tee -a "${LOG_FILE}"
      fi
      vision_suite_skip=1
    fi

    if [[ "${vision_runtime_available}" == "1" ]]; then
      vision_udid="$(find_sim_udid "vision" "apple vision")"
      if [[ -z "${vision_udid}" ]]; then
        if [[ "${PLATFORM}" == "visionos" ]]; then
          echo "error: no available Apple Vision simulator found" | tee -a "${LOG_FILE}"
          status="failed"
        else
          echo "warning: no available Apple Vision simulator found; skipping visionOS suite" | tee -a "${LOG_FILE}"
        fi
        vision_suite_skip=1
      fi
    fi
  fi
fi

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

if [[ "${status}" == "passed" ]]; then
  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "macos" ]]; then
    run_case "APPLE-BUILD-001" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiMacOS -destination "platform=macOS"
  fi

  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ios" ]]; then
    if [[ "${ios_suite_skip}" == "1" ]]; then
      append_result "APPLE-BUILD-002" "skipped"
    else
      run_case "APPLE-BUILD-002" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiIOS -destination "id=${ios_udid}"
    fi
  fi

  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ipados" ]]; then
    if [[ "${ipados_suite_skip}" == "1" ]]; then
      append_result "APPLE-BUILD-003" "skipped"
    else
      run_case "APPLE-BUILD-003" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiIPadOS -destination "id=${ipad_udid}"
    fi
  fi

  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "visionos" ]]; then
    if [[ "${SKIP_VISIONOS}" == "1" ]]; then
      if [[ "${SKIP_VISIONOS_REASON}" == "ci-default" ]]; then
        echo "info: visionOS suite disabled in CI by default (set SKIP_VISIONOS=0 to enable)" | tee -a "${LOG_FILE}"
      else
        echo "info: visionOS suite disabled by SKIP_VISIONOS=1" | tee -a "${LOG_FILE}"
      fi
      append_result "APPLE-BUILD-004" "skipped"
    elif [[ "${vision_suite_skip}" == "1" || "${vision_runtime_available}" != "1" ]]; then
      append_result "APPLE-BUILD-004" "skipped"
    elif [[ -n "${vision_udid}" ]]; then
      run_case "APPLE-BUILD-004" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiVisionOS -destination "id=${vision_udid}"
    else
      echo "warning: no available Apple Vision simulator found; skipping visionOS suite" | tee -a "${LOG_FILE}"
      append_result "APPLE-BUILD-004" "skipped"
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

echo "Native Apple smoke status: ${status}"
echo "Native Apple smoke report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
