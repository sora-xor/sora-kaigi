#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
TEST_NAME="tests::long_duration_control_plane_soak_preserves_core_invariants"
SCENARIO_ID="SCALE-004"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/scale004-soak.log"
REPORT_FILE="${OUT_DIR}/scale004-soak-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

status="passed"
if ! cargo test -p kaigi-hub-echo "${TEST_NAME}" -- --exact --nocapture 2>&1 | tee "${LOG_FILE}"; then
  status="failed"
fi

if [[ "${status}" == "passed" ]]; then
  if ! grep -Eq "^running 1 test$" "${LOG_FILE}"; then
    echo "expected exactly one soak test to run" >&2
    status="failed"
  elif ! grep -Fq "test ${TEST_NAME} ... ok" "${LOG_FILE}"; then
    echo "expected soak test ${TEST_NAME} to pass" >&2
    status="failed"
  fi
fi

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF
{
  "scenario_id": "${SCENARIO_ID}",
  "test_name": "${TEST_NAME}",
  "status": "${status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}"
}
EOF

echo "SCALE-004 soak status: ${status}"
echo "SCALE-004 soak report: ${REPORT_FILE}"

if [[ "${status}" != "passed" ]]; then
  exit 1
fi
