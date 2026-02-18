#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
SUITE_ID="CONFORMANCE-EVIDENCE-BUNDLE"

mkdir -p "${OUT_DIR}"
LOG_FILE="${OUT_DIR}/conformance-evidence-bundle.log"
REPORT_FILE="${OUT_DIR}/conformance-evidence-bundle-report.json"

started_epoch="$(date -u +%s)"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

declare -a SUITES=(
  "scale004|scripts/run_scale004_soak.sh|scale004-soak-report.json"
  "security_protocol|scripts/run_security_protocol_smoke.sh|security-protocol-smoke-report.json"
  "media_hdr_recording|scripts/run_media_hdr_recording_smoke.sh|media-hdr-recording-smoke-report.json"
  "hdr_transport|scripts/run_hdr_transport_smoke.sh|hdr-transport-smoke-report.json"
  "m2_exit_criteria|scripts/run_m2_exit_criteria_smoke.sh|m2-exit-criteria-smoke-report.json"
  "av_baseline|scripts/run_av_baseline_smoke.sh|av-baseline-smoke-report.json"
  "screen_share_constraints|scripts/run_screen_share_constraints_smoke.sh|screen-share-constraints-smoke-report.json"
  "controlplane_reliability|scripts/run_controlplane_reliability_smoke.sh|controlplane-reliability-smoke-report.json"
  "parity_status|scripts/run_parity_status_smoke.sh|parity-status-smoke-report.json"
  "platform_contract|scripts/run_platform_contract_smoke.sh|platform-contract-smoke-report.json"
  "client_app_workspaces|scripts/run_client_app_workspaces_smoke.sh|client-app-workspaces-smoke-report.json"
  "client_release_tracks|scripts/run_client_release_tracks_smoke.sh|client-release-tracks-smoke-report.json"
  "client_release_playbook_alignment|scripts/run_client_release_playbook_alignment_smoke.sh|client-release-playbook-alignment-smoke-report.json"
  "client_fallback_drills|scripts/run_client_fallback_drills_smoke.sh|client-fallback-drills-smoke-report.json"
  "client_fallback_drill_results|scripts/run_client_fallback_drill_results_smoke.sh|client-fallback-drill-results-smoke-report.json"
  "client_release_manifest|scripts/run_client_release_manifest_smoke.sh|client-release-manifest-smoke-report.json"
  "client_release_readiness_gates|scripts/run_client_release_readiness_gates_smoke.sh|client-release-readiness-gates-smoke-report.json"
  "client_rollback_manifest|scripts/run_client_rollback_manifest_smoke.sh|client-rollback-manifest-smoke-report.json"
  "release_playbook|scripts/run_release_playbook_smoke.sh|release-playbook-smoke-report.json"
  "hardening_ga|scripts/run_hardening_ga_smoke.sh|hardening-ga-smoke-report.json"
  "release_readiness|scripts/run_release_readiness_smoke.sh|release-readiness-smoke-report.json"
  "parity_readiness|scripts/run_parity_readiness_smoke.sh|parity-readiness-smoke-report.json"
  "m3_exit_criteria|scripts/run_m3_exit_criteria_smoke.sh|m3-exit-criteria-smoke-report.json"
  "parity_ga|scripts/run_parity_ga_smoke.sh|parity-ga-smoke-report.json"
  "parity_downgrade_guard|scripts/run_parity_downgrade_guard_smoke.sh|parity-downgrade-guard-smoke-report.json"
  "parity_waiver_policy|scripts/run_parity_waiver_policy_smoke.sh|parity-waiver-policy-smoke-report.json"
  "parity_waiver_fixture_manifest|scripts/run_parity_waiver_fixture_manifest_smoke.sh|parity-waiver-fixture-manifest-smoke-report.json"
  "parity_waiver_fixture_coverage|scripts/run_parity_waiver_fixture_coverage_smoke.sh|parity-waiver-fixture-coverage-smoke-report.json"
  "parity_waiver_policy_negative|scripts/run_parity_waiver_policy_negative_smoke.sh|parity-waiver-policy-negative-smoke-report.json"
  "coverage_check|scripts/run_conformance_coverage_check.sh|conformance-coverage-report.json"
  "release_readiness_final|scripts/run_release_readiness_smoke.sh|release-readiness-smoke-report.json"
)

: >"${LOG_FILE}"
bundle_status="passed"
results_json=""

for suite in "${SUITES[@]}"; do
  IFS='|' read -r suite_name suite_script suite_report_basename <<<"${suite}"
  echo "=== ${suite_name} :: ${suite_script} ===" | tee -a "${LOG_FILE}"
  suite_status="passed"
  if ! bash "${suite_script}" "${OUT_DIR}" 2>&1 | tee -a "${LOG_FILE}"; then
    suite_status="failed"
    bundle_status="failed"
  fi

  report_path="${OUT_DIR}/${suite_report_basename}"
  entry="{\"suite\":\"${suite_name}\",\"script\":\"${suite_script}\",\"status\":\"${suite_status}\",\"report_file\":\"${report_path}\"}"
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
  "status": "${bundle_status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF

echo "=== evidence_index :: scripts/run_conformance_evidence_index.sh ===" | tee -a "${LOG_FILE}"
index_status="passed"
if ! bash scripts/run_conformance_evidence_index.sh "${OUT_DIR}" 2>&1 | tee -a "${LOG_FILE}"; then
  index_status="failed"
  bundle_status="failed"
fi

index_entry="{\"suite\":\"evidence_index\",\"script\":\"scripts/run_conformance_evidence_index.sh\",\"status\":\"${index_status}\",\"report_file\":\"${OUT_DIR}/conformance-evidence-index-report.json\"}"
results_json="${results_json},${index_entry}"

finished_epoch="$(date -u +%s)"
finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
duration_seconds=$((finished_epoch - started_epoch))

cat >"${REPORT_FILE}" <<EOF
{
  "suite_id": "${SUITE_ID}",
  "status": "${bundle_status}",
  "started_at": "${started_at}",
  "finished_at": "${finished_at}",
  "duration_seconds": ${duration_seconds},
  "log_file": "${LOG_FILE}",
  "results": [${results_json}]
}
EOF

echo "Conformance evidence bundle status: ${bundle_status}"
echo "Conformance evidence bundle report: ${REPORT_FILE}"

if [[ "${bundle_status}" != "passed" ]]; then
  exit 1
fi
