#!/usr/bin/env python3
"""Generate parity readiness summary from status contract and conformance coverage."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import sys

BETA_OR_HIGHER = {"B", "GA"}


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


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
        "--output-report",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/parity-readiness-report.json"),
    )
    parser.add_argument(
        "--log-file",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/parity-readiness.log"),
    )
    args = parser.parse_args()

    contract = load_json(args.parity_status_contract)
    coverage = load_json(args.coverage_report)

    if contract.get("schema") != "kaigi-parity-status-contract/v1":
        raise RuntimeError("unexpected parity status contract schema")

    platforms = contract.get("platforms")
    capabilities = contract.get("capabilities")
    if not isinstance(platforms, list) or not isinstance(capabilities, list):
        raise RuntimeError("invalid parity status contract shape")
    if not platforms or not capabilities:
        raise RuntimeError("parity status contract cannot be empty")

    below_beta: list[dict[str, object]] = []
    status_totals: dict[str, int] = {}
    per_area_totals: dict[str, int] = {}
    per_area_below_beta: dict[str, int] = {}

    for entry in capabilities:
        if not isinstance(entry, dict):
            raise RuntimeError("capability entries must be objects")
        area = entry.get("area")
        name = entry.get("name")
        statuses = entry.get("statuses")
        if not isinstance(area, str) or not isinstance(name, str) or not isinstance(statuses, dict):
            raise RuntimeError("invalid capability entry fields")

        per_area_totals[area] = per_area_totals.get(area, 0) + 1
        platforms_below: list[str] = []
        for platform in platforms:
            status = statuses.get(platform)
            if not isinstance(status, str):
                raise RuntimeError(f"{area}/{name}: missing status for platform {platform}")
            status_totals[status] = status_totals.get(status, 0) + 1
            if status not in BETA_OR_HIGHER:
                platforms_below.append(platform)

        if platforms_below:
            per_area_below_beta[area] = per_area_below_beta.get(area, 0) + 1
            below_beta.append(
                {
                    "area": area,
                    "name": name,
                    "platforms_below_beta": platforms_below,
                }
            )

    beta_gate_ready = len(below_beta) == 0
    coverage_status = coverage.get("status") == "passed"
    required_scenarios_count = coverage.get("required_scenarios")
    if isinstance(required_scenarios_count, list):
        scenario_count = len(required_scenarios_count)
    else:
        scenario_count = 0

    generated_at = utc_now_iso()
    report = {
        "suite_id": "PARITY-READINESS-REPORT",
        "status": "passed",
        "generated_at": generated_at,
        "parity_status_contract": str(args.parity_status_contract),
        "coverage_report": str(args.coverage_report),
        "capability_count": len(capabilities),
        "platform_count": len(platforms),
        "status_totals": status_totals,
        "per_area_totals": per_area_totals,
        "per_area_below_beta": per_area_below_beta,
        "capabilities_below_beta": below_beta,
        "beta_gate_ready": beta_gate_ready,
        "coverage_status_passed": coverage_status,
        "required_scenarios_count": scenario_count,
        "m3_exit_ready": beta_gate_ready and coverage_status,
        "log_file": str(args.log_file),
    }

    log_lines = [
        f"[{generated_at}] Parity readiness report generated",
        f"Capabilities: {len(capabilities)}",
        f"Platforms: {len(platforms)}",
        f"Capabilities below beta: {len(below_beta)}",
        f"Coverage status passed: {coverage_status}",
        f"M3 exit ready: {report['m3_exit_ready']}",
    ]
    args.output_report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    args.log_file.write_text("\n".join(log_lines) + "\n", encoding="utf-8")

    print("Parity readiness report status: passed")
    print(f"Parity readiness report: {args.output_report}")
    print(f"Parity readiness log: {args.log_file}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
