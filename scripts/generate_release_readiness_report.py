#!/usr/bin/env python3
"""Generate a release-readiness summary from conformance evidence and defect ledger."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import sys
import tempfile
from typing import Dict, List, Tuple

SCENARIO_ID_RE = re.compile(r"`([A-Z]+(?:-[A-Z]+)*-\d{3})`")
IGNORED_REPORT_BASENAMES = {
    "conformance-evidence-bundle-report.json",
    "conformance-coverage-report.json",
    "conformance-evidence-index-report.json",
    "release-readiness-report.json",
}

ALPHA_SCENARIOS = {
    "P-CONF-001",
    "P-CONF-002",
    "P-CONF-003",
    "P-CONF-004",
    "SEC-001",
    "SEC-002",
    "SEC-003",
    "SEC-004",
}
RELIABILITY_SCENARIOS = {
    "SCALE-001",
    "SCALE-002",
    "SCALE-003",
    "SCALE-004",
}
BETA_SCENARIOS = {
    "MOD-001",
    "MOD-002",
    "MOD-003",
    "MOD-004",
    "MOD-005",
    "MOD-006",
    "MEDIA-001",
    "MEDIA-002",
    "MEDIA-003",
    "HDR-001",
    "HDR-002",
    "HDR-003",
    "HDR-004",
    "HDR-005",
    "REC-001",
    "REC-002",
    "REC-003",
}
PLATFORM_SCENARIOS = {
    "PLATFORM-001",
    "PLATFORM-002",
    "PLATFORM-003",
    "PLATFORM-004",
    "PLATFORM-005",
    "PLATFORM-006",
}
OPS_SCENARIOS = {
    "OPS-001",
    "OPS-002",
    "OPS-003",
    "OPS-004",
    "OPS-005",
    "OPS-006",
    "OPS-007",
}


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def parse_required_scenarios(test_plan_path: pathlib.Path) -> List[str]:
    text = test_plan_path.read_text(encoding="utf-8")
    scenarios: List[str] = []
    seen = set()
    for scenario_id in SCENARIO_ID_RE.findall(text):
        if scenario_id in seen:
            continue
        seen.add(scenario_id)
        scenarios.append(scenario_id)
    return scenarios


def extract_scenario_results(report_path: pathlib.Path) -> List[Tuple[str, str]]:
    data = json.loads(report_path.read_text(encoding="utf-8"))
    rows: List[Tuple[str, str]] = []
    if isinstance(data, dict):
        scenario_id = data.get("scenario_id")
        status = data.get("status")
        if isinstance(scenario_id, str) and isinstance(status, str):
            rows.append((scenario_id, status))

        results = data.get("results")
        if isinstance(results, list):
            for item in results:
                if not isinstance(item, dict):
                    continue
                sid = item.get("scenario_id")
                state = item.get("status")
                if isinstance(sid, str) and isinstance(state, str):
                    rows.append((sid, state))
    return rows


def scenario_passes(scenario_statuses: Dict[str, List[Dict[str, str]]], scenario_id: str) -> bool:
    return any(entry["status"] == "passed" for entry in scenario_statuses.get(scenario_id, []))


def gate_result(
    scenario_statuses: Dict[str, List[Dict[str, str]]], required: set[str]
) -> Dict[str, object]:
    missing_or_failed = sorted(sid for sid in required if not scenario_passes(scenario_statuses, sid))
    return {
        "status": "passed" if not missing_or_failed else "failed",
        "required_scenarios": sorted(required),
        "missing_or_failed_scenarios": missing_or_failed,
    }


def atomic_write_text(path: pathlib.Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
        delete=False,
    ) as tmp:
        tmp.write(content)
        tmp.flush()
        os.fsync(tmp.fileno())
        temp_path = pathlib.Path(tmp.name)
    temp_path.replace(path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--test-plan",
        type=pathlib.Path,
        default=pathlib.Path("docs/test-plan.md"),
    )
    parser.add_argument(
        "--reports-dir",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance"),
    )
    parser.add_argument(
        "--critical-defects",
        type=pathlib.Path,
        default=pathlib.Path("docs/critical-defects.json"),
    )
    parser.add_argument(
        "--output-report",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/release-readiness-report.json"),
    )
    parser.add_argument(
        "--log-file",
        type=pathlib.Path,
        default=pathlib.Path("target/conformance/release-readiness.log"),
    )
    parser.add_argument(
        "--assume-passed",
        action="append",
        default=[],
        help="Scenario ID to treat as passed for this report generation run.",
    )
    args = parser.parse_args()

    started_at = utc_now_iso()
    log_lines: List[str] = [f"[{started_at}] Release readiness report started"]
    parse_errors: List[Dict[str, str]] = []

    required_scenarios = parse_required_scenarios(args.test_plan)
    if not required_scenarios:
        raise RuntimeError(f"no scenario IDs found in {args.test_plan}")
    required_set = set(required_scenarios)

    scenario_statuses: Dict[str, List[Dict[str, str]]] = {}
    scanned_reports: List[str] = []
    for report_path in sorted(args.reports_dir.glob("*-report.json")):
        if report_path.name in IGNORED_REPORT_BASENAMES:
            continue
        try:
            entries = extract_scenario_results(report_path)
        except Exception as exc:  # noqa: BLE001
            parse_errors.append({"report_file": str(report_path), "error": str(exc)})
            continue

        scanned_reports.append(str(report_path))
        for scenario_id, status in entries:
            scenario_statuses.setdefault(scenario_id, []).append(
                {"status": status, "report_file": str(report_path)}
            )

    for scenario_id in args.assume_passed:
        if scenario_id not in required_set:
            continue
        scenario_statuses.setdefault(scenario_id, []).append(
            {"status": "passed", "report_file": "<assumed-local-run>"}
        )

    missing_scenarios = sorted(sid for sid in required_scenarios if sid not in scenario_statuses)
    failing_scenarios = sorted(
        sid
        for sid in required_scenarios
        if sid in scenario_statuses and not scenario_passes(scenario_statuses, sid)
    )

    defects_payload = json.loads(args.critical_defects.read_text(encoding="utf-8"))
    open_critical = defects_payload.get("open_critical")
    if not isinstance(open_critical, list):
        raise RuntimeError("critical-defects file has invalid open_critical field")
    open_critical_ids = []
    for entry in open_critical:
        if isinstance(entry, dict):
            issue_id = entry.get("id")
            if isinstance(issue_id, str) and issue_id.strip():
                open_critical_ids.append(issue_id)
    zero_open_critical = len(open_critical) == 0

    alpha_gate = gate_result(scenario_statuses, ALPHA_SCENARIOS)
    reliability_gate = gate_result(scenario_statuses, RELIABILITY_SCENARIOS)
    beta_gate = gate_result(scenario_statuses, BETA_SCENARIOS)
    platform_gate = gate_result(scenario_statuses, PLATFORM_SCENARIOS)
    ops_gate = gate_result(scenario_statuses, OPS_SCENARIOS)

    all_mandatory_scenarios_pass = not missing_scenarios and not failing_scenarios
    ga_gate_passed = (
        all_mandatory_scenarios_pass
        and zero_open_critical
        and alpha_gate["status"] == "passed"
        and reliability_gate["status"] == "passed"
        and beta_gate["status"] == "passed"
        and platform_gate["status"] == "passed"
        and ops_gate["status"] == "passed"
    )

    finished_at = utc_now_iso()
    report = {
        "suite_id": "RELEASE-READINESS-REPORT",
        "status": "failed" if parse_errors else "passed",
        "generated_at": finished_at,
        "test_plan": str(args.test_plan),
        "reports_dir": str(args.reports_dir),
        "critical_defects_file": str(args.critical_defects),
        "report_files_scanned": scanned_reports,
        "required_scenarios_count": len(required_scenarios),
        "missing_scenarios": missing_scenarios,
        "failing_scenarios": failing_scenarios,
        "gates": {
            "alpha": alpha_gate,
            "reliability": reliability_gate,
            "beta": beta_gate,
            "platform": platform_gate,
            "ops": ops_gate,
            "all_mandatory_scenarios_pass": all_mandatory_scenarios_pass,
            "zero_open_critical_defects": zero_open_critical,
            "ga_gate_passed": ga_gate_passed,
        },
        "open_critical_defect_count": len(open_critical),
        "open_critical_defect_ids": open_critical_ids,
        "assumed_passed_scenarios": args.assume_passed,
        "parse_errors": parse_errors,
        "log_file": str(args.log_file),
    }

    log_lines.append(f"Required scenario count: {len(required_scenarios)}")
    log_lines.append(f"Scanned report files: {len(scanned_reports)}")
    log_lines.append(f"Missing scenarios: {len(missing_scenarios)}")
    log_lines.append(f"Failing scenarios: {len(failing_scenarios)}")
    log_lines.append(f"Open critical defects: {len(open_critical)}")
    log_lines.append(f"GA gate passed: {ga_gate_passed}")
    log_lines.append(f"Report status: {report['status']}")

    atomic_write_text(args.output_report, json.dumps(report, indent=2) + "\n")
    atomic_write_text(args.log_file, "\n".join(log_lines) + "\n")
    print(f"Release readiness report status: {report['status']}")
    print(f"Release readiness report: {args.output_report}")
    print(f"Release readiness log: {args.log_file}")

    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
