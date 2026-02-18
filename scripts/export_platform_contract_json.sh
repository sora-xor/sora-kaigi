#!/usr/bin/env bash
set -euo pipefail

OUT_FILE="${1:-docs/platform-contract.json}"
OUT_DIR="$(dirname "${OUT_FILE}")"
mkdir -p "${OUT_DIR}"

cargo run -q -p kaigi-cli -- platform-contract --pretty >"${OUT_FILE}"
echo "Wrote frozen platform contract JSON: ${OUT_FILE}"
