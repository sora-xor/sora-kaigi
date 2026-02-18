#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-target/conformance}"
mkdir -p "${OUT_DIR}"

python3 scripts/generate_conformance_evidence_index.py \
  --coverage-report "${OUT_DIR}/conformance-coverage-report.json" \
  --bundle-report "${OUT_DIR}/conformance-evidence-bundle-report.json" \
  --output-md "${OUT_DIR}/conformance-evidence-index.md" \
  --output-report "${OUT_DIR}/conformance-evidence-index-report.json" \
  --log-file "${OUT_DIR}/conformance-evidence-index.log"
