#!/usr/bin/env python3
"""Validate mandatory conformance scenario coverage from evidence reports."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import sys
import tempfile
import time
from typing import Dict, List, Tuple

SCENARIO_ID_RE = re.compile(r"`([A-Z]+(?:-[A-Z]+)*-\d{3})`")
IGNORED_REPORT_BASENAMES = {
    "conformance-evidence-bundle-report.json",
    "conformance-coverage-report.json",
}
REPORT_PARSE_RETRY_ATTEMPTS = 3
REPORT_PARSE_RETRY_DELAY_SECONDS = 0.05


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


def extract_scenario_results_with_retry(report_path: pathlib.Path) -> List[Tuple[str, str]]:
    for attempt in range(REPORT_PARSE_RETRY_ATTEMPTS + 1):
        try:
            return extract_scenario_results(report_path)
        except json.JSONDecodeError:
            if attempt >= REPORT_PARSE_RETRY_ATTEMPTS:
                raise
            time.sleep(REPORT_PARSE_RETRY_DELAY_SECONDS)
    raise RuntimeError(f"failed to parse report after retries: {report_path}")


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


def write_log(path: pathlib.Path, lines: List[str]) -> None:
    atomic_write_text(path, "\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-plan", required=True, type=pathlib.Path)
    parser.add_argument("--reports-dir", required=True, type=pathlib.Path)
    parser.add_argument("--report-file", required=True, type=pathlib.Path)
    parser.add_argument("--log-file", required=True, type=pathlib.Path)
    parser.add_argument(
        "--allow-missing-scenario",
        action="append",
        default=[],
        help="Scenario ID allowed to be missing without failing this coverage run.",
    )
    parser.add_argument(
        "--allow-failed-scenario",
        action="append",
        default=[],
        help="Scenario ID allowed to have only failing evidence without failing this coverage run.",
    )
    args = parser.parse_args()

    started_at = utc_now_iso()
    log_lines: List[str] = [f"[{started_at}] Conformance coverage check started"]
    parse_errors: List[Dict[str, str]] = []

    required_scenarios = parse_required_scenarios(args.test_plan)
    if not required_scenarios:
        raise RuntimeError(f"no scenario IDs found in {args.test_plan}")
    log_lines.append(f"Required scenario count: {len(required_scenarios)}")

    scenario_statuses: Dict[str, List[Dict[str, str]]] = {}
    scanned_reports: List[str] = []
    for report_path in sorted(args.reports_dir.glob("*-report.json")):
        if report_path.name in IGNORED_REPORT_BASENAMES:
            continue
        try:
            entries = extract_scenario_results_with_retry(report_path)
        except Exception as exc:  # noqa: BLE001
            parse_errors.append(
                {
                    "report_file": str(report_path),
                    "error": str(exc),
                }
            )
            log_lines.append(f"PARSE ERROR: {report_path}: {exc}")
            continue

        scanned_reports.append(str(report_path))
        for scenario_id, status in entries:
            scenario_statuses.setdefault(scenario_id, []).append(
                {"status": status, "report_file": str(report_path)}
            )

    allowed_missing = set(args.allow_missing_scenario)
    allowed_failed = set(args.allow_failed_scenario)
    missing_scenarios = [
        sid
        for sid in required_scenarios
        if sid not in scenario_statuses and sid not in allowed_missing
    ]
    waived_missing_scenarios = [
        sid
        for sid in required_scenarios
        if sid not in scenario_statuses and sid in allowed_missing
    ]
    failing_scenarios: List[Dict[str, object]] = []
    waived_failing_scenarios: List[str] = []
    for scenario_id in required_scenarios:
        statuses = scenario_statuses.get(scenario_id, [])
        if not statuses:
            continue
        if not any(entry["status"] == "passed" for entry in statuses):
            if scenario_id in allowed_failed:
                waived_failing_scenarios.append(scenario_id)
                continue
            failing_scenarios.append(
                {
                    "scenario_id": scenario_id,
                    "statuses": statuses,
                }
            )

    scenario_coverage = []
    for scenario_id in required_scenarios:
        statuses = scenario_statuses.get(scenario_id, [])
        scenario_coverage.append(
            {
                "scenario_id": scenario_id,
                "seen": bool(statuses),
                "pass_count": sum(1 for entry in statuses if entry["status"] == "passed"),
                "statuses": statuses,
            }
        )

    finished_at = utc_now_iso()
    report = {
        "suite_id": "CONFORMANCE-COVERAGE-CHECK",
        "status": "passed",
        "started_at": started_at,
        "finished_at": finished_at,
        "test_plan": str(args.test_plan),
        "reports_dir": str(args.reports_dir),
        "report_files_scanned": scanned_reports,
        "required_scenarios": required_scenarios,
        "scenario_coverage": scenario_coverage,
        "missing_scenarios": missing_scenarios,
        "waived_missing_scenarios": waived_missing_scenarios,
        "waived_failing_scenarios": sorted(waived_failing_scenarios),
        "failing_scenarios": failing_scenarios,
        "parse_errors": parse_errors,
        "allow_missing_scenarios": sorted(allowed_missing),
        "allow_failed_scenarios": sorted(allowed_failed),
        "log_file": str(args.log_file),
    }
    if missing_scenarios or failing_scenarios or parse_errors:
        report["status"] = "failed"

    log_lines.append(f"Scanned report files: {len(scanned_reports)}")
    log_lines.append(f"Missing scenarios: {len(missing_scenarios)}")
    if missing_scenarios:
        log_lines.append("Missing IDs: " + ", ".join(missing_scenarios))
    log_lines.append(f"Waived missing scenarios: {len(waived_missing_scenarios)}")
    if waived_missing_scenarios:
        log_lines.append("Waived missing IDs: " + ", ".join(waived_missing_scenarios))
    log_lines.append(f"Waived failing scenarios: {len(waived_failing_scenarios)}")
    if waived_failing_scenarios:
        log_lines.append("Waived failing IDs: " + ", ".join(sorted(waived_failing_scenarios)))
    log_lines.append(f"Failing scenarios: {len(failing_scenarios)}")
    if failing_scenarios:
        log_lines.append(
            "Failing IDs: "
            + ", ".join(entry["scenario_id"] for entry in failing_scenarios)
        )
    log_lines.append(f"Report parse errors: {len(parse_errors)}")
    log_lines.append(f"Coverage status: {report['status']}")

    atomic_write_text(args.report_file, json.dumps(report, indent=2) + "\n")
    write_log(args.log_file, log_lines)
    print(f"Conformance coverage status: {report['status']}")
    print(f"Conformance coverage report: {args.report_file}")
    print(f"Conformance coverage log: {args.log_file}")

    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
