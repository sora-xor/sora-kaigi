#!/usr/bin/env python3
"""Validate parity status contract is synchronized with docs/parity-matrix.md."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ALLOWED_STATUSES = {"P", "A", "B", "GA"}
EXPECTED_SCHEMA = "kaigi-parity-status-contract/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
SECTION_ORDER = [
    ("core", "## Core Meeting Capability Matrix"),
    ("hdr", "## HDR Matrix"),
    ("governance", "## Moderation and Governance Matrix"),
]


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


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


def parse_table_rows(lines: list[str], section_header: str) -> tuple[list[str], list[tuple[str, dict[str, str]]]]:
    start = None
    for idx, line in enumerate(lines):
        if line.strip() == section_header:
            start = idx + 1
            break
    if start is None:
        raise RuntimeError(f"missing section in parity matrix: {section_header}")

    header_idx = None
    for idx in range(start, len(lines)):
        stripped = lines[idx].strip()
        if stripped.startswith("## "):
            break
        if stripped.startswith("|"):
            header_idx = idx
            break
    if header_idx is None:
        raise RuntimeError(f"missing table header in section: {section_header}")

    header_cells = [cell.strip() for cell in lines[header_idx].strip().strip("|").split("|")]
    if len(header_cells) < 2:
        raise RuntimeError(f"invalid table header in section: {section_header}")
    platforms = header_cells[1:]

    rows: list[tuple[str, dict[str, str]]] = []
    for idx in range(header_idx + 1, len(lines)):
        stripped = lines[idx].strip()
        if stripped.startswith("## "):
            break
        if not stripped.startswith("|"):
            continue
        if set(stripped.replace("|", "").replace("-", "").replace(":", "").strip()) == set():
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if len(cells) != len(header_cells):
            continue
        capability = cells[0]
        status_values = cells[1:]
        if len(status_values) != len(platforms):
            raise RuntimeError(f"{section_header}: row platform/value count mismatch for {capability}")
        status_map = dict(zip(platforms, status_values))
        rows.append((capability, status_map))
    return platforms, rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    parser.add_argument(
        "--parity-status-contract",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-status-contract.json"),
    )
    args = parser.parse_args()

    contract = load_json(args.parity_status_contract)
    if contract.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected parity status contract schema")
    if contract.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity status contract")

    contract_platforms = contract.get("platforms")
    capabilities = contract.get("capabilities")
    if not isinstance(contract_platforms, list) or not contract_platforms:
        raise RuntimeError("contract platforms must be non-empty array")
    if not isinstance(capabilities, list) or not capabilities:
        raise RuntimeError("contract capabilities must be non-empty array")

    mandatory_platforms = parse_mandatory_platforms(args.parity_matrix)
    if mandatory_platforms != contract_platforms:
        raise RuntimeError("contract platforms do not match parity matrix mandatory platforms")

    lines = args.parity_matrix.read_text(encoding="utf-8").splitlines()
    expected: list[tuple[str, str, dict[str, str]]] = []
    for area, header in SECTION_ORDER:
        table_platforms, rows = parse_table_rows(lines, header)
        if table_platforms != mandatory_platforms:
            raise RuntimeError(f"{header}: table platforms do not match mandatory platform list")
        for capability_name, statuses in rows:
            for platform, status in statuses.items():
                if status not in ALLOWED_STATUSES:
                    raise RuntimeError(f"{capability_name} / {platform}: invalid status {status}")
            expected.append((area, capability_name, statuses))

    if len(expected) != len(capabilities):
        raise RuntimeError(
            f"capability count mismatch: matrix={len(expected)} contract={len(capabilities)}"
        )

    contract_map: dict[tuple[str, str], dict[str, str]] = {}
    for capability in capabilities:
        if not isinstance(capability, dict):
            raise RuntimeError("contract capability entries must be objects")
        area = capability.get("area")
        name = capability.get("name")
        statuses = capability.get("statuses")
        if not isinstance(area, str) or not area:
            raise RuntimeError("contract capability area must be non-empty string")
        if not isinstance(name, str) or not name:
            raise RuntimeError("contract capability name must be non-empty string")
        if (area, name) in contract_map:
            raise RuntimeError(f"duplicate contract capability entry: area={area} name={name}")
        if not isinstance(statuses, dict):
            raise RuntimeError(f"{area}/{name}: statuses must be an object")
        if set(statuses.keys()) != set(mandatory_platforms):
            raise RuntimeError(f"{area}/{name}: statuses platforms mismatch mandatory platform list")
        if any(status not in ALLOWED_STATUSES for status in statuses.values()):
            raise RuntimeError(f"{area}/{name}: invalid status value present")
        contract_map[(area, name)] = {str(k): str(v) for k, v in statuses.items()}

    for area, name, expected_statuses in expected:
        key = (area, name)
        if key not in contract_map:
            raise RuntimeError(f"missing capability in contract: area={area} name={name}")
        if contract_map[key] != expected_statuses:
            raise RuntimeError(f"status mismatch for capability: area={area} name={name}")

    print(f"[OK] parity status contract matches parity matrix ({len(expected)} capabilities)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
