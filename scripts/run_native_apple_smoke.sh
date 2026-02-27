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

if [[ -n "${NATIVE_APPLE_SMOKE_APPEND_REPORT+x}" ]]; then
  NATIVE_APPLE_SMOKE_APPEND_REPORT="${NATIVE_APPLE_SMOKE_APPEND_REPORT}"
else
  NATIVE_APPLE_SMOKE_APPEND_REPORT="0"
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
Usage: bash scripts/run_native_apple_smoke.sh [--out-dir <path>] [--platform all|macos|ios|ipados|visionos|tvos|watchos] [--skip-xcodegen] [out_dir]
Env:
  NATIVE_APPLE_SMOKE_APPEND_REPORT=1   append to existing log/report instead of starting fresh
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

if [[ "${PLATFORM}" != "all" && "${PLATFORM}" != "macos" && "${PLATFORM}" != "ios" && "${PLATFORM}" != "ipados" && "${PLATFORM}" != "visionos" && "${PLATFORM}" != "tvos" && "${PLATFORM}" != "watchos" ]]; then
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
LOCK_DIR="${OUT_DIR}/.native-apple-smoke.lock"

acquired_lock=0
cleanup_lock() {
  if [[ "${acquired_lock}" == "1" ]]; then
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  fi
}
trap cleanup_lock EXIT

if mkdir "${LOCK_DIR}" 2>/dev/null; then
  acquired_lock=1
else
  echo "error: another run_native_apple_smoke.sh instance is already writing to ${OUT_DIR}; use a different --out-dir or wait for it to finish" >&2
  exit 2
fi

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

if [[ "${NATIVE_APPLE_SMOKE_APPEND_REPORT}" == "1" ]]; then
  touch "${LOG_FILE}"
  {
    echo
    echo "----- run started ${started_at} platform=${PLATFORM} -----"
  } >> "${LOG_FILE}"
else
  : >"${LOG_FILE}"
  echo "----- run started ${started_at} platform=${PLATFORM} -----" >> "${LOG_FILE}"
fi
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
tv_udid=""
watch_udid=""
vision_runtime_available=1
ios_suite_skip=0
ipados_suite_skip=0
vision_suite_skip=0
tv_suite_skip=0
watch_suite_skip=0

requires_simulator=0
if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "ios" || "${PLATFORM}" == "ipados" || "${PLATFORM}" == "tvos" || "${PLATFORM}" == "watchos" ]]; then
  requires_simulator=1
fi
if [[ "${SKIP_VISIONOS}" != "1" && ( "${PLATFORM}" == "all" || "${PLATFORM}" == "visionos" ) ]]; then
  requires_simulator=1
fi

if [[ "${requires_simulator}" == "1" ]]; then
  if ! xcrun simctl list devices available --json >/dev/null 2>&1; then
    if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
      echo "warning: CoreSimulatorService unavailable; skipping iOS/iPadOS/tvOS/watchOS/visionOS suites (ALLOW_SIMULATOR_SKIPS=1)" | tee -a "${LOG_FILE}"
      ios_suite_skip=1
      ipados_suite_skip=1
      tv_suite_skip=1
      watch_suite_skip=1
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

if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "tvos" ]]; then
  if [[ "${tv_suite_skip}" != "1" ]]; then
    tv_udid="$(find_sim_udid "tvos" "apple tv")"
    if [[ -z "${tv_udid}" ]]; then
      if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
        echo "warning: no available Apple TV simulator found; skipping tvOS suite" | tee -a "${LOG_FILE}"
        tv_suite_skip=1
      else
        echo "error: no available Apple TV simulator found" | tee -a "${LOG_FILE}"
        status="failed"
      fi
    fi
  fi
fi

if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "watchos" ]]; then
  if [[ "${watch_suite_skip}" != "1" ]]; then
    watch_udid="$(find_sim_udid "watchos" "apple watch")"
    if [[ -z "${watch_udid}" ]]; then
      if [[ "${ALLOW_SIMULATOR_SKIPS}" == "1" && "${PLATFORM}" == "all" ]]; then
        echo "warning: no available Apple Watch simulator found; skipping watchOS suite" | tee -a "${LOG_FILE}"
        watch_suite_skip=1
      else
        echo "error: no available Apple Watch simulator found" | tee -a "${LOG_FILE}"
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
  local attempt=1
  local max_attempts="${XCODEBUILD_AUTOMATION_TIMEOUT_RETRY_ATTEMPTS:-2}"
  local retry_pattern="Timed out while enabling automation mode"

  if ! [[ "${max_attempts}" =~ ^[0-9]+$ ]] || [[ "${max_attempts}" -lt 1 ]]; then
    max_attempts=1
  fi

  while true; do
    local attempt_log
    attempt_log="$(mktemp "${TMPDIR:-/tmp}/native-apple-${scenario_id}-XXXX.log")"

    echo "=== ${scenario_id} (attempt ${attempt}/${max_attempts}) :: $* ===" | tee -a "${LOG_FILE}"
    if "$@" 2>&1 | tee -a "${LOG_FILE}" | tee "${attempt_log}"; then
      rm -f "${attempt_log}"
      break
    fi

    if [[ "${attempt}" -lt "${max_attempts}" ]] && rg -Fq "${retry_pattern}" "${attempt_log}"; then
      echo "warning: ${scenario_id} hit XCTest automation timeout; retrying" | tee -a "${LOG_FILE}"
      rm -f "${attempt_log}"
      attempt=$((attempt + 1))
      continue
    fi

    scenario_status="failed"
    status="failed"
    rm -f "${attempt_log}"
    break
  done

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

  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "tvos" ]]; then
    if [[ "${tv_suite_skip}" == "1" ]]; then
      append_result "APPLE-BUILD-005" "skipped"
    else
      run_case "APPLE-BUILD-005" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiTVOS -destination "id=${tv_udid}"
    fi
  fi

  if [[ "${PLATFORM}" == "all" || "${PLATFORM}" == "watchos" ]]; then
    if [[ "${watch_suite_skip}" == "1" ]]; then
      append_result "APPLE-BUILD-006" "skipped"
    else
      run_case "APPLE-BUILD-006" xcodebuild test -workspace Kaigi.xcworkspace -scheme KaigiWatchOS -destination "id=${watch_udid}"
    fi
  fi
fi

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

python3 - "${REPORT_FILE}" "${SUITE_ID}" "${LOG_FILE}" "${started_at}" "${finished_at}" "${duration_seconds}" "${status}" "${results_json}" "${NATIVE_APPLE_SMOKE_APPEND_REPORT}" <<'PY'
import json
import os
import sys
from datetime import datetime
from typing import Optional


def parse_ts(value: object) -> Optional[datetime]:
    if not isinstance(value, str) or not value:
        return None
    normalized = value
    if value.endswith("Z"):
        normalized = value[:-1] + "+00:00"
    try:
        return datetime.fromisoformat(normalized)
    except ValueError:
        return None


def parse_results(raw: str) -> list[dict]:
    if not raw:
        return []
    try:
        parsed = json.loads(f"[{raw}]")
    except json.JSONDecodeError:
        return []
    return [item for item in parsed if isinstance(item, dict)]


(
    report_path,
    suite_id,
    log_file,
    run_started_at,
    run_finished_at,
    run_duration_seconds,
    run_status,
    run_results_raw,
    append_report,
) = sys.argv[1:]

existing_report: dict = {}
if append_report == "1" and os.path.exists(report_path):
    try:
        with open(report_path, "r", encoding="utf-8") as handle:
            loaded = json.load(handle)
            if isinstance(loaded, dict):
                existing_report = loaded
    except (OSError, json.JSONDecodeError):
        existing_report = {}

merged_results: dict[str, dict] = {}
for entry in existing_report.get("results", []):
    if isinstance(entry, dict):
        scenario_id = entry.get("scenario_id")
        if isinstance(scenario_id, str) and scenario_id:
            merged_results[scenario_id] = entry

for entry in parse_results(run_results_raw):
    scenario_id = entry.get("scenario_id")
    if isinstance(scenario_id, str) and scenario_id:
        merged_results[scenario_id] = entry

results = sorted(merged_results.values(), key=lambda item: item.get("scenario_id", ""))

existing_started_at = existing_report.get("started_at")
run_started_dt = parse_ts(run_started_at)
existing_started_dt = parse_ts(existing_started_at)
if run_started_dt and existing_started_dt:
    started_at = existing_started_at if existing_started_dt <= run_started_dt else run_started_at
elif existing_started_dt:
    started_at = existing_started_at
else:
    started_at = run_started_at

finished_at = run_finished_at if run_finished_at else existing_report.get("finished_at", run_started_at)
started_dt = parse_ts(started_at)
finished_dt = parse_ts(finished_at)
if started_dt and finished_dt:
    duration_seconds = max(0, int((finished_dt - started_dt).total_seconds()))
else:
    try:
        duration_seconds = int(run_duration_seconds)
    except ValueError:
        duration_seconds = int(existing_report.get("duration_seconds", 0) or 0)

status_values = [str(entry.get("status", "")) for entry in results]
overall_status = "passed"
if run_status == "failed" or "failed" in status_values:
    overall_status = "failed"
elif status_values and all(value == "skipped" for value in status_values):
    overall_status = "skipped"

payload = {
    "suite_id": suite_id,
    "status": overall_status,
    "started_at": started_at,
    "finished_at": finished_at,
    "duration_seconds": duration_seconds,
    "log_file": log_file,
    "results": results,
}

with open(report_path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
PY

echo "Native Apple smoke status: ${status}"
echo "Native Apple smoke report: ${REPORT_FILE}"

suite_status="$(python3 - "${REPORT_FILE}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)
print(payload.get("status", "failed"))
PY
)"

echo "Native Apple smoke aggregated status: ${suite_status}"

if [[ "${suite_status}" == "failed" ]]; then
  exit 1
fi
