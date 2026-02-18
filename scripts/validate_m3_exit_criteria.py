#!/usr/bin/env python3
"""Validate roadmap M3 exit criteria: parity >= beta and mandatory conformance coverage passing."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_PARITY_STATUSES = {"B", "GA"}


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
    parser.add_argument(
        "--parity-readiness-report",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/parity-readiness-report.json"),
    )
    args = parser.parse_args()

    parity_contract = load_json(args.parity_status_contract)
    coverage_report = load_json(args.coverage_report)
    readiness_report = load_json(args.parity_readiness_report)

    if parity_contract.get("schema") != "kaigi-parity-status-contract/v1":
        raise RuntimeError("unexpected parity status contract schema")

    capabilities = parity_contract.get("capabilities")
    platforms = parity_contract.get("platforms")
    if not isinstance(capabilities, list) or not capabilities:
        raise RuntimeError("parity status contract capabilities must be non-empty array")
    if not isinstance(platforms, list) or not platforms:
        raise RuntimeError("parity status contract platforms must be non-empty array")

    below_beta_rows: list[str] = []
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
            if status not in ALLOWED_PARITY_STATUSES:
                below_beta_rows.append(f"{area}/{name}/{platform}:{status}")

    if below_beta_rows:
        raise RuntimeError(
            "parity statuses below beta remain: " + ", ".join(sorted(below_beta_rows)[:10])
        )

    coverage_status = coverage_report.get("status")
    if coverage_status != "passed":
        raise RuntimeError("conformance coverage report status is not passed")

    if readiness_report.get("suite_id") != "PARITY-READINESS-REPORT":
        raise RuntimeError("unexpected parity readiness report suite_id")
    if readiness_report.get("beta_gate_ready") is not True:
        raise RuntimeError("parity readiness report beta_gate_ready is false")
    if readiness_report.get("coverage_status_passed") is not True:
        raise RuntimeError("parity readiness report coverage_status_passed is false")
    if readiness_report.get("m3_exit_ready") is not True:
        raise RuntimeError("parity readiness report m3_exit_ready is false")

    print(
        "[OK] M3 exit criteria passed "
        f"({len(capabilities)} capabilities x {len(platforms)} platforms at Beta+ and coverage passing)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
