#!/usr/bin/env python3
"""Validate waiver fixture manifest covers all waiver-policy controls."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any

EXPECTED_MANIFEST_SCHEMA = "kaigi-parity-waiver-fixture-manifest/v1"
EXPECTED_POLICY_SCHEMA = "kaigi-parity-waiver-policy/v1"
EXPECTED_FROZEN_AT = "2026-02-15"


def load_json(path: pathlib.Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/fixtures/waivers/manifest.json"),
    )
    parser.add_argument(
        "--waiver-policy",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-waiver-policy.json"),
    )
    args = parser.parse_args()

    manifest = load_json(args.manifest)
    policy = load_json(args.waiver_policy)

    if manifest.get("schema") != EXPECTED_MANIFEST_SCHEMA:
        raise RuntimeError("unexpected parity waiver fixture manifest schema")
    if policy.get("schema") != EXPECTED_POLICY_SCHEMA:
        raise RuntimeError("unexpected parity waiver policy schema")
    if manifest.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity waiver fixture manifest")
    if policy.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity waiver policy")

    fixtures = manifest.get("fixtures")
    if not isinstance(fixtures, list) or not fixtures:
        raise RuntimeError("fixture manifest must include non-empty fixtures array")

    failing_fragments: list[str] = []
    pass_fixture_count = 0
    for fixture in fixtures:
        if not isinstance(fixture, dict):
            raise RuntimeError("fixture entries must be objects")
        expected = fixture.get("expected")
        if expected == "pass":
            pass_fixture_count += 1
            continue
        if expected == "fail":
            fragment = fixture.get("expect_error_contains")
            if not isinstance(fragment, str) or not fragment.strip():
                raise RuntimeError("failing fixture must set expect_error_contains")
            failing_fragments.append(fragment)
            continue
        raise RuntimeError("fixture expected must be pass|fail")

    if pass_fixture_count == 0:
        raise RuntimeError("fixture manifest must include at least one passing control fixture")

    required_fragment_by_control: list[tuple[str, str]] = []

    if isinstance(policy.get("min_reason_chars"), int):
        required_fragment_by_control.append(("min_reason_chars", "reason too short"))
    if isinstance(policy.get("max_reason_chars"), int):
        required_fragment_by_control.append(("max_reason_chars", "reason too long"))
    if isinstance(policy.get("max_waiver_ttl_days"), int):
        required_fragment_by_control.append(("max_waiver_ttl_days", "ttl exceeds policy max"))

    allowed_statuses = policy.get("allowed_target_statuses")
    if isinstance(allowed_statuses, list):
        required_fragment_by_control.append(
            ("allowed_target_statuses", "target_status must be one of")
        )

    if isinstance(policy.get("owner_pattern"), str):
        required_fragment_by_control.append(("owner_pattern", "owner format invalid"))
    if isinstance(policy.get("approved_by_pattern"), str):
        required_fragment_by_control.append(("approved_by_pattern", "approved_by format invalid"))
    if isinstance(policy.get("ticket_pattern"), str):
        required_fragment_by_control.append(("ticket_pattern", "ticket format invalid"))
    if policy.get("require_distinct_owner_and_approver") is True:
        required_fragment_by_control.append(
            (
                "require_distinct_owner_and_approver",
                "owner and approver must differ",
            )
        )

    missing_controls: list[str] = []
    matched_controls: list[str] = []
    for control, fragment in required_fragment_by_control:
        matched = any(fragment in candidate for candidate in failing_fragments)
        if matched:
            matched_controls.append(control)
        else:
            missing_controls.append(f"{control} ({fragment})")

    if missing_controls:
        raise RuntimeError(
            "fixture manifest missing negative coverage for controls: "
            + ", ".join(missing_controls)
        )

    print(
        "[OK] parity waiver fixture coverage valid "
        f"({len(matched_controls)} controls mapped, "
        f"{len(failing_fragments)} failing fixtures, {pass_fixture_count} passing fixtures)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
