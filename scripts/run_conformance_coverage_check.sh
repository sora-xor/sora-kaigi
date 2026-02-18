#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
shift || true
mkdir -p "${OUT_DIR}"

python3 scripts/validate_conformance_coverage.py \
  --test-plan docs/test-plan.md \
  --reports-dir "${OUT_DIR}" \
  --report-file "${OUT_DIR}/conformance-coverage-report.json" \
  --log-file "${OUT_DIR}/conformance-coverage.log" \
  "$@"
