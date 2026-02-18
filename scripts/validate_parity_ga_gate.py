#!/usr/bin/env python3
"""Validate GA parity gate: every parity status is GA with passing conformance coverage."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--parity-status-contract",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-status-contract.json"),
    )
    parser.add_argument(
        "--coverage-report",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/conformance-coverage-report.json"),
    )
    args = parser.parse_args()

    parity_contract = load_json(args.parity_status_contract)
    coverage_report = load_json(args.coverage_report)

    if parity_contract.get("schema") != "kaigi-parity-status-contract/v1":
        raise RuntimeError("unexpected parity status contract schema")

    capabilities = parity_contract.get("capabilities")
    platforms = parity_contract.get("platforms")
    if not isinstance(capabilities, list) or not capabilities:
        raise RuntimeError("parity status contract capabilities must be non-empty array")
    if not isinstance(platforms, list) or not platforms:
        raise RuntimeError("parity status contract platforms must be non-empty array")

    non_ga_rows: list[str] = []
    for capability in capabilities:
        if not isinstance(capability, dict):
            raise RuntimeError("parity capability entries must be objects")
        area = capability.get("area")
        name = capability.get("name")
        statuses = capability.get("statuses")
        if not isinstance(area, str) or not isinstance(name, str) or not isinstance(statuses, dict):
            raise RuntimeError("parity capability entry fields are invalid")

        for platform in platforms:
            status = statuses.get(platform)
            if status != "GA":
                non_ga_rows.append(f"{area}/{name}/{platform}:{status}")

    if non_ga_rows:
        raise RuntimeError(
            "parity statuses not at GA: " + ", ".join(sorted(non_ga_rows)[:10])
        )

    if coverage_report.get("status") != "passed":
        raise RuntimeError("conformance coverage report status is not passed")

    print(
        "[OK] parity GA gate passed "
        f"({len(capabilities)} capabilities x {len(platforms)} platforms at GA and coverage passing)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
