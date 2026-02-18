#!/usr/bin/env python3
"""Validate that the critical-defect ledger has zero open critical defects."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

EXPECTED_SCHEMA = "kaigi-critical-defects/v1"
EXPECTED_FROZEN_AT = "2026-02-15"


def parse_mandatory_platforms(parity_matrix_path: pathlib.Path) -> list[str]:
    lines = parity_matrix_path.read_text(encoding="utf-8").splitlines()
    collecting = False
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped == "## Mandatory Platforms":
            collecting = True
            continue
        if not collecting:
            continue
        if stripped.startswith("## "):
            break
        if stripped.startswith("- "):
            value = stripped[2:].strip()
            value = re.sub(r"\s*\([^)]*\)\s*$", "", value)
            out.append(value)
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--defects-file",
        type=pathlib.Path,
        default=pathlib.Path("docs/critical-defects.json"),
    )
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    args = parser.parse_args()

    payload = json.loads(args.defects_file.read_text(encoding="utf-8"))
    if payload.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError(f"unexpected schema in {args.defects_file}")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError(f"unexpected frozen_at in {args.defects_file}")

    mandatory_platforms = parse_mandatory_platforms(args.parity_matrix)
    if len(mandatory_platforms) != 9:
        raise RuntimeError("parity matrix mandatory platform list must contain 9 entries")

    generated_at = payload.get("generated_at")
    if not isinstance(generated_at, str) or not generated_at.strip():
        raise RuntimeError("critical-defect ledger must contain generated_at")

    open_critical = payload.get("open_critical")
    if not isinstance(open_critical, list):
        raise RuntimeError("open_critical must be an array")

    if open_critical:
        ids: list[str] = []
        for entry in open_critical:
            if isinstance(entry, dict):
                issue_id = entry.get("id")
                if isinstance(issue_id, str) and issue_id.strip():
                    ids.append(issue_id)
        details = ", ".join(ids) if ids else f"{len(open_critical)} entries"
        raise RuntimeError(f"open critical defects remain: {details}")

    print("[OK] critical-defect ledger reports zero open critical defects")
    return 0


if __name__ == "__main__":
    sys.exit(main())
