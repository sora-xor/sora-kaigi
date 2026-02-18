#!/usr/bin/env python3
"""Execute parity waiver fixture cases and compare outcomes against manifest expectations."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import subprocess
import sys
import tempfile
from typing import Any

TOKEN_RE = re.compile(r"__EXPIRES_PLUS_(\d+)_DAYS__")


def load_json(path: pathlib.Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def to_utc_iso(value: dt.datetime) -> str:
    return value.replace(microsecond=0).isoformat().replace("+00:00", "Z")


def materialize_tokens(value: Any, now: dt.datetime) -> Any:
    if isinstance(value, str):
        match = TOKEN_RE.fullmatch(value)
        if match:
            delta_days = int(match.group(1))
            return to_utc_iso(now + dt.timedelta(days=delta_days))
        return value
    if isinstance(value, list):
        return [materialize_tokens(item, now) for item in value]
    if isinstance(value, dict):
        return {key: materialize_tokens(item, now) for key, item in value.items()}
    return value


def run_case(
    *,
    fixture_id: str,
    fixture_path: pathlib.Path,
    expected: str,
    expect_error_contains: str | None,
    validator: pathlib.Path,
    now: dt.datetime,
    tmp_dir: pathlib.Path,
) -> dict[str, Any]:
    payload = load_json(fixture_path)
    materialized = materialize_tokens(payload, now)
    materialized_path = tmp_dir / f"{fixture_id}.json"
    materialized_path.write_text(json.dumps(materialized, indent=2) + "\n", encoding="utf-8")

    cmd = [
        sys.executable,
        str(validator),
        "--waivers-file",
        str(materialized_path),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True)  # noqa: S603
    output = (proc.stdout + proc.stderr).strip()

    passed = False
    if expected == "pass":
        passed = proc.returncode == 0
    elif expected == "fail":
        passed = proc.returncode != 0
        if passed and expect_error_contains:
            passed = expect_error_contains in output

    return {
        "fixture_id": fixture_id,
        "fixture_file": str(fixture_path),
        "materialized_file": str(materialized_path),
        "expected": expected,
        "expect_error_contains": expect_error_contains,
        "return_code": proc.returncode,
        "passed": passed,
        "validator_output": output,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/fixtures/waivers/manifest.json"),
    )
    parser.add_argument(
        "--validator",
        type=pathlib.Path,
        default=pathlib.Path("scripts/validate_parity_waiver_policy.py"),
    )
    parser.add_argument("--checks-json", type=pathlib.Path, required=True)
    parser.add_argument("--log-file", type=pathlib.Path, required=True)
    args = parser.parse_args()

    manifest = load_json(args.manifest)
    fixtures = manifest.get("fixtures")
    if not isinstance(fixtures, list) or not fixtures:
        raise RuntimeError("manifest fixtures must be non-empty array")

    now = dt.datetime.now(dt.timezone.utc)
    checks: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = pathlib.Path(tmp)
        for entry in fixtures:
            if not isinstance(entry, dict):
                raise RuntimeError("fixture entries must be objects")
            fixture_id = entry.get("id")
            fixture_file = entry.get("fixture_file")
            expected = entry.get("expected")
            expect_error_contains = entry.get("expect_error_contains")

            if not isinstance(fixture_id, str) or not isinstance(fixture_file, str):
                raise RuntimeError("fixture id and fixture_file must be strings")
            if expected not in {"pass", "fail"}:
                raise RuntimeError(f"fixture {fixture_id}: expected must be pass|fail")
            if expect_error_contains is not None and not isinstance(expect_error_contains, str):
                raise RuntimeError(
                    f"fixture {fixture_id}: expect_error_contains must be string when present"
                )

            result = run_case(
                fixture_id=fixture_id,
                fixture_path=pathlib.Path(fixture_file),
                expected=expected,
                expect_error_contains=expect_error_contains,
                validator=args.validator,
                now=now,
                tmp_dir=tmp_dir,
            )
            checks.append(result)

    status = "passed" if all(check["passed"] for check in checks) else "failed"

    log_lines = [
        f"[{to_utc_iso(dt.datetime.now(dt.timezone.utc))}] Executed parity waiver fixtures",
        f"Manifest: {args.manifest}",
        f"Validator: {args.validator}",
        f"Fixture count: {len(checks)}",
        f"Status: {status}",
    ]
    for check in checks:
        log_lines.append(
            f"- {check['fixture_id']}: expected={check['expected']} "
            f"rc={check['return_code']} passed={check['passed']}"
        )
        if check["expected"] == "fail" and check.get("expect_error_contains"):
            log_lines.append(f"  expect_error_contains={check['expect_error_contains']}")

    args.checks_json.write_text(json.dumps(checks, indent=2) + "\n", encoding="utf-8")
    args.log_file.write_text("\n".join(log_lines) + "\n", encoding="utf-8")

    for check in checks:
        print(
            f"fixture={check['fixture_id']} expected={check['expected']} "
            f"rc={check['return_code']} passed={check['passed']}"
        )

    print(f"fixture execution status: {status}")
    print(f"checks json: {args.checks_json}")
    print(f"log file: {args.log_file}")

    return 0 if status == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
