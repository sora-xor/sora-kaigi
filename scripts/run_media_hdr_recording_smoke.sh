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
Usage: bash scripts/run_media_hdr_recording_smoke.sh [--out-dir <path>] [out_dir]
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
SUITE_ID="MEDIA-HDR-REC-SMOKE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/media-hdr-recording-smoke.log"
REPORT_FILE="${OUT_DIR}/media-hdr-recording-smoke-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a CASES=(
  "MOD-002|kaigi-hub-echo|tests::moderation_handle_frame_disables_screen_share_and_broadcasts_state"
  "MOD-002|kaigi-hub-echo|tests::moderation_handle_frame_kick_sends_error_and_close_to_target"
  "MEDIA-002|kaigi-hub-echo|tests::participant_state_screen_share_respects_max_screen_shares_limit"
  "HDR-001|kaigi-hub-echo|tests::media_profile_handle_frame_preserves_hdr_with_sender_and_remote_support"
  "HDR-002|kaigi-hub-echo|tests::media_profile_handle_frame_falls_back_to_sdr_without_hdr_capabilities"
  "REC-001|kaigi-hub-echo|tests::recording_notice_handle_frame_rejects_start_when_policy_disallows"
  "REC-002|kaigi-hub-echo|tests::recording_notice_handle_frame_broadcasts_when_policy_allows"
  "REC-003|kaigi-hub-echo|tests::recording_notice_handle_frame_rejects_start_when_policy_disallows"
)

: >"${LOG_FILE}"
suite_status="passed"
results_json=""

for case in "${CASES[@]}"; do
  IFS='|' read -r scenario_id package test_name <<<"${case}"
  echo "=== ${scenario_id} :: ${package} :: ${test_name} ===" | tee -a "${LOG_FILE}"
  case_status="passed"
  if ! cargo test -p "${package}" "${test_name}" -- --exact --nocapture 2>&1 | tee -a "${LOG_FILE}"; then
    case_status="failed"
    suite_status="failed"
  fi

  entry="{\"scenario_id\":\"${scenario_id}\",\"package\":\"${package}\",\"test_name\":\"${test_name}\",\"status\":\"${case_status}\"}"
  if [[ -z "${results_json}" ]]; then
    results_json="${entry}"
  else
    results_json="${results_json},${entry}"
  fi
done

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF
{
  "suite_id": "${SUITE_ID}",
  "status": "${suite_status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF

echo "Media/HDR/recording smoke status: ${suite_status}"
echo "Media/HDR/recording smoke report: ${REPORT_FILE}"

if [[ "${suite_status}" != "passed" ]]; then
  exit 1
fi
