#!/usr/bin/env python3
"""Ensure CI workflow matrices stay in sync with frozen docs contracts."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

SCENARIO_ID_RE = re.compile(r"`([A-Z]+(?:-[A-Z]+)*-\d{3})`")


def parse_required_scenarios(test_plan_path: pathlib.Path) -> list[str]:
    text = test_plan_path.read_text(encoding="utf-8")
    seen: set[str] = set()
    ordered: list[str] = []
    for scenario in SCENARIO_ID_RE.findall(text):
        if scenario in seen:
            continue
        seen.add(scenario)
        ordered.append(scenario)
    return ordered


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
            # Allow descriptive parenthetical annotations in docs bullets.
            value = re.sub(r"\s*\([^)]*\)\s*$", "", value)
            out.append(value)
    return out


def parse_workflow_matrix_list(
    workflow_path: pathlib.Path, *, job_name: str, matrix_key: str
) -> list[str]:
    lines = workflow_path.read_text(encoding="utf-8").splitlines()

    job_line_re = re.compile(rf"^\s*{re.escape(job_name)}:\s*$")
    key_line_re = re.compile(rf"^\s*{re.escape(matrix_key)}:\s*$")

    in_job = False
    job_indent = -1
    in_matrix = False
    matrix_indent = -1
    in_key = False
    key_indent = -1
    values: list[str] = []

    for line in lines:
        stripped = line.strip()
        indent = len(line) - len(line.lstrip(" "))

        if not in_job:
            if job_line_re.match(line):
                in_job = True
                job_indent = indent
            continue

        if stripped.endswith(":") and indent <= job_indent and not job_line_re.match(line):
            break

        if not in_matrix:
            if stripped == "matrix:":
                in_matrix = True
                matrix_indent = indent
            continue

        if stripped.endswith(":") and indent <= matrix_indent and stripped != "matrix:":
            in_matrix = False
            in_key = False
            continue

        if not in_key:
            if key_line_re.match(line):
                in_key = True
                key_indent = indent
            continue

        if indent <= key_indent:
            in_key = False
            continue

        if stripped.startswith("- "):
            values.append(stripped[2:].strip())

    return values


def diff_sets(expected: list[str], actual: list[str]) -> tuple[list[str], list[str]]:
    expected_set = set(expected)
    actual_set = set(actual)
    missing = sorted(expected_set - actual_set)
    extra = sorted(actual_set - expected_set)
    return missing, extra


def print_mismatch(
    *,
    title: str,
    expected: list[str],
    actual: list[str],
    missing: list[str],
    extra: list[str],
) -> None:
    print(f"[FAIL] {title}")
    print(f"  expected_count={len(expected)} actual_count={len(actual)}")
    if missing:
        print(f"  missing_in_workflow: {', '.join(missing)}")
    if extra:
        print(f"  extra_in_workflow: {', '.join(extra)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--workflow",
        type=pathlib.Path,
        default=pathlib.Path(".github/workflows/conformance-matrix.yml"),
    )
    parser.add_argument(
        "--test-plan", type=pathlib.Path, default=pathlib.Path("docs/test-plan.md")
    )
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    args = parser.parse_args()

    expected_scenarios = parse_required_scenarios(args.test_plan)
    expected_platforms = parse_mandatory_platforms(args.parity_matrix)

    workflow_scenarios = parse_workflow_matrix_list(
        args.workflow,
        job_name="scenario-contract-matrix",
        matrix_key="scenario",
    )
    workflow_platforms = parse_workflow_matrix_list(
        args.workflow,
        job_name="platform-parity-matrix",
        matrix_key="platform",
    )

    scenario_missing, scenario_extra = diff_sets(expected_scenarios, workflow_scenarios)
    platform_missing, platform_extra = diff_sets(expected_platforms, workflow_platforms)

    failed = False
    if scenario_missing or scenario_extra:
        failed = True
        print_mismatch(
            title="scenario matrix out of sync with docs/test-plan.md",
            expected=expected_scenarios,
            actual=workflow_scenarios,
            missing=scenario_missing,
            extra=scenario_extra,
        )
    else:
        print(
            f"[OK] scenario matrix matches docs/test-plan.md ({len(expected_scenarios)} scenarios)"
        )

    if platform_missing or platform_extra:
        failed = True
        print_mismatch(
            title="platform matrix out of sync with docs/parity-matrix.md",
            expected=expected_platforms,
            actual=workflow_platforms,
            missing=platform_missing,
            extra=platform_extra,
        )
    else:
        print(
            f"[OK] platform matrix matches docs/parity-matrix.md ({len(expected_platforms)} platforms)"
        )

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
