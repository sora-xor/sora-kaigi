#!/usr/bin/env python3
"""Generate a markdown index mapping scenario IDs to evidence report files."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import sys
from typing import Any


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def read_json(path: pathlib.Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coverage-report", required=True, type=pathlib.Path)
    parser.add_argument("--bundle-report", required=True, type=pathlib.Path)
    parser.add_argument("--output-md", required=True, type=pathlib.Path)
    parser.add_argument("--output-report", required=True, type=pathlib.Path)
    parser.add_argument("--log-file", required=True, type=pathlib.Path)
    args = parser.parse_args()

    started_at = utc_now_iso()
    log_lines = [f"[{started_at}] Generating conformance evidence index"]

    coverage = read_json(args.coverage_report)
    bundle = read_json(args.bundle_report)

    coverage_status = str(coverage.get("status", "failed"))
    bundle_status = str(bundle.get("status", "failed"))
    status = "passed" if coverage_status == "passed" and bundle_status == "passed" else "failed"

    rows = []
    for entry in coverage.get("scenario_coverage", []):
        if not isinstance(entry, dict):
            continue
        scenario_id = str(entry.get("scenario_id", "")).strip()
        if not scenario_id:
            continue
        statuses = entry.get("statuses", [])
        pass_files: list[str] = []
        if isinstance(statuses, list):
            for status_entry in statuses:
                if not isinstance(status_entry, dict):
                    continue
                if status_entry.get("status") != "passed":
                    continue
                report_file = status_entry.get("report_file")
                if isinstance(report_file, str) and report_file not in pass_files:
                    pass_files.append(report_file)
        scenario_status = "passed" if pass_files else "missing_or_failed"
        rows.append(
            {
                "scenario_id": scenario_id,
                "status": scenario_status,
                "pass_count": len(pass_files),
                "report_files": pass_files,
            }
        )

    missing = [row["scenario_id"] for row in rows if row["status"] != "passed"]
    if missing:
        status = "failed"

    bundle_suite_rows = []
    for result in bundle.get("results", []):
        if not isinstance(result, dict):
            continue
        bundle_suite_rows.append(
            {
                "suite": str(result.get("suite", "")),
                "status": str(result.get("status", "")),
                "report_file": str(result.get("report_file", "")),
            }
        )

    md_lines = [
        "# Conformance Evidence Index",
        "",
        f"- Generated at: `{utc_now_iso()}`",
        f"- Coverage report: `{args.coverage_report}`",
        f"- Bundle report: `{args.bundle_report}`",
        f"- Overall status: `{status}`",
        "",
        "## Scenario Coverage",
        "",
        "| Scenario | Status | Pass Count | Evidence Reports |",
        "|---|---|---:|---|",
    ]
    for row in rows:
        report_files = ", ".join(f"`{path}`" for path in row["report_files"]) or "`<none>`"
        md_lines.append(
            f"| `{row['scenario_id']}` | `{row['status']}` | {row['pass_count']} | {report_files} |"
        )

    md_lines.extend(
        [
            "",
            "## Bundle Suites",
            "",
            "| Suite | Status | Report |",
            "|---|---|---|",
        ]
    )
    for row in bundle_suite_rows:
        md_lines.append(
            f"| `{row['suite']}` | `{row['status']}` | `{row['report_file']}` |"
        )

    args.output_md.write_text("\n".join(md_lines) + "\n", encoding="utf-8")

    report_payload = {
        "suite_id": "CONFORMANCE-EVIDENCE-INDEX",
        "status": status,
        "generated_at": utc_now_iso(),
        "coverage_report": str(args.coverage_report),
        "bundle_report": str(args.bundle_report),
        "output_md": str(args.output_md),
        "scenario_rows": rows,
        "bundle_suites": bundle_suite_rows,
        "missing_or_failed_scenarios": missing,
        "log_file": str(args.log_file),
    }
    args.output_report.write_text(json.dumps(report_payload, indent=2) + "\n", encoding="utf-8")

    log_lines.append(f"Scenario rows: {len(rows)}")
    log_lines.append(f"Missing/failed scenarios: {len(missing)}")
    if missing:
        log_lines.append("Missing IDs: " + ", ".join(missing))
    log_lines.append(f"Bundle suite rows: {len(bundle_suite_rows)}")
    log_lines.append(f"Index status: {status}")
    args.log_file.write_text("\n".join(log_lines) + "\n", encoding="utf-8")

    print(f"Conformance evidence index status: {status}")
    print(f"Conformance evidence index markdown: {args.output_md}")
    print(f"Conformance evidence index report: {args.output_report}")
    print(f"Conformance evidence index log: {args.log_file}")
    return 0 if status == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
