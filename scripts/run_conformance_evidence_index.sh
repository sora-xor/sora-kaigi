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
Usage: bash scripts/run_conformance_evidence_index.sh [--out-dir <path>] [out_dir]
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

mkdir -p "${OUT_DIR}"

python3 scripts/generate_conformance_evidence_index.py \
  --coverage-report "${OUT_DIR}/conformance-coverage-report.json" \
  --bundle-report "${OUT_DIR}/conformance-evidence-bundle-report.json" \
  --output-md "${OUT_DIR}/conformance-evidence-index.md" \
  --output-report "${OUT_DIR}/conformance-evidence-index-report.json" \
  --log-file "${OUT_DIR}/conformance-evidence-index.log"
