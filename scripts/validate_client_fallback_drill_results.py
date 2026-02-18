#!/usr/bin/env python3
"""Validate fallback drill execution results against fallback drill contract."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import sys

EXPECTED_RESULTS_SCHEMA = "kaigi-client-fallback-drill-results/v1"
EXPECTED_DRILLS_SCHEMA = "kaigi-client-fallback-drills/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
STATUS_ALLOWED = {"passed"}
ISO_Z_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def require_non_empty_str(entry: dict, field: str, context: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"{context}: {field} must be non-empty string")
    return value.strip()


def parse_mandatory_web_platforms(parity_matrix_path: pathlib.Path) -> set[str]:
    lines = parity_matrix_path.read_text(encoding="utf-8").splitlines()
    collecting = False
    platforms: set[str] = set()
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
            if value.startswith("Web "):
                platforms.add(value)
    return platforms


def parse_drills(payload: dict) -> dict[str, dict]:
    if payload.get("schema") != EXPECTED_DRILLS_SCHEMA:
        raise RuntimeError("unexpected fallback drill contract schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in fallback drill contract")

    drills = payload.get("drills")
    if not isinstance(drills, list) or not drills:
        raise RuntimeError("fallback drill contract: drills must be non-empty array")

    out: dict[str, dict] = {}
    for drill in drills:
        if not isinstance(drill, dict):
            raise RuntimeError("fallback drill contract: drill entries must be objects")

        workspace_id = require_non_empty_str(drill, "workspace_id", "drill")
        if workspace_id in out:
            raise RuntimeError(f"fallback drill contract: duplicate workspace_id {workspace_id}")

        platform = require_non_empty_str(drill, "platform", f"drill {workspace_id}")
        fallback_workspace = require_non_empty_str(
            drill, "fallback_workspace", f"drill {workspace_id}"
        )
        distribution_channel = require_non_empty_str(
            drill, "distribution_channel", f"drill {workspace_id}"
        )

        max_rto_minutes = drill.get("max_rto_minutes")
        if not isinstance(max_rto_minutes, int):
            raise RuntimeError(f"drill {workspace_id}: max_rto_minutes must be integer")

        out[workspace_id] = {
            "platform": platform,
            "fallback_workspace": fallback_workspace,
            "distribution_channel": distribution_channel,
            "max_rto_minutes": max_rto_minutes,
        }

    return out


def parse_iso_z(value: str, *, context: str) -> dt.datetime:
    if not ISO_Z_RE.fullmatch(value):
        raise RuntimeError(f"{context}: executed_at must be RFC3339 second precision Z timestamp")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--results",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-fallback-drill-results.json"),
    )
    parser.add_argument(
        "--drills",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-fallback-drills.json"),
    )
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    args = parser.parse_args()

    drill_by_workspace = parse_drills(load_json(args.drills))
    web_platforms = parse_mandatory_web_platforms(args.parity_matrix)
    if not web_platforms:
        raise RuntimeError("failed to parse mandatory web platforms from parity matrix")

    payload = load_json(args.results)
    if payload.get("schema") != EXPECTED_RESULTS_SCHEMA:
        raise RuntimeError("unexpected fallback drill results schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in fallback drill results contract")

    release_train = require_non_empty_str(payload, "release_train", "fallback drill results")

    results = payload.get("results")
    if not isinstance(results, list) or not results:
        raise RuntimeError("fallback drill results: results must be non-empty array")

    seen_workspace_ids: set[str] = set()
    ordered_workspace_ids: list[str] = []
    covered_web_platforms: set[str] = set()
    executed_at_by_workspace: dict[str, dt.datetime] = {}

    for result in results:
        if not isinstance(result, dict):
            raise RuntimeError("fallback drill results: result entries must be objects")

        workspace_id = require_non_empty_str(result, "workspace_id", "result")
        if workspace_id in seen_workspace_ids:
            raise RuntimeError(f"fallback drill results: duplicate workspace_id {workspace_id}")
        if workspace_id not in drill_by_workspace:
            raise RuntimeError(
                f"fallback drill results: unknown workspace_id {workspace_id} (missing drill contract entry)"
            )
        seen_workspace_ids.add(workspace_id)
        ordered_workspace_ids.append(workspace_id)

        drill = drill_by_workspace[workspace_id]

        platform = require_non_empty_str(result, "platform", f"result {workspace_id}")
        if platform != drill["platform"]:
            raise RuntimeError(
                f"result {workspace_id}: platform mismatch; expected {drill['platform']}, got {platform}"
            )

        fallback_workspace = require_non_empty_str(
            result, "fallback_workspace", f"result {workspace_id}"
        )
        if fallback_workspace != drill["fallback_workspace"]:
            raise RuntimeError(
                f"result {workspace_id}: fallback_workspace mismatch; expected {drill['fallback_workspace']}, got {fallback_workspace}"
            )

        distribution_channel = require_non_empty_str(
            result, "distribution_channel", f"result {workspace_id}"
        )
        if distribution_channel != drill["distribution_channel"]:
            raise RuntimeError(
                f"result {workspace_id}: distribution_channel mismatch; expected {drill['distribution_channel']}, got {distribution_channel}"
            )

        browser_profile = require_non_empty_str(
            result, "browser_profile", f"result {workspace_id}"
        )
        if browser_profile not in web_platforms:
            raise RuntimeError(
                f"result {workspace_id}: browser_profile must be one of {sorted(web_platforms)}"
            )
        covered_web_platforms.add(browser_profile)

        observed_rto_minutes = result.get("observed_rto_minutes")
        if not isinstance(observed_rto_minutes, int):
            raise RuntimeError(
                f"result {workspace_id}: observed_rto_minutes must be integer"
            )
        if observed_rto_minutes < 0:
            raise RuntimeError(
                f"result {workspace_id}: observed_rto_minutes must be >= 0"
            )
        if observed_rto_minutes > drill["max_rto_minutes"]:
            raise RuntimeError(
                f"result {workspace_id}: observed_rto_minutes exceeds drill max_rto_minutes "
                f"({observed_rto_minutes} > {drill['max_rto_minutes']})"
            )

        status = require_non_empty_str(result, "status", f"result {workspace_id}")
        if status not in STATUS_ALLOWED:
            raise RuntimeError(
                f"result {workspace_id}: status must be one of {sorted(STATUS_ALLOWED)}"
            )

        executed_at_raw = require_non_empty_str(
            result, "executed_at", f"result {workspace_id}"
        )
        executed_at_by_workspace[workspace_id] = parse_iso_z(
            executed_at_raw, context=f"result {workspace_id}"
        )

    missing_workspace_ids = sorted(set(drill_by_workspace) - seen_workspace_ids)
    if missing_workspace_ids:
        raise RuntimeError(
            "fallback drill results missing workspace entries: "
            + ", ".join(missing_workspace_ids)
        )

    if ordered_workspace_ids != sorted(ordered_workspace_ids):
        raise RuntimeError("fallback drill results must be sorted by workspace_id")

    missing_browser_coverage = sorted(web_platforms - covered_web_platforms)
    if missing_browser_coverage:
        raise RuntimeError(
            "fallback drill results missing web browser coverage: "
            + ", ".join(missing_browser_coverage)
        )

    if release_train != "2027-01-31-bigbang-ga":
        raise RuntimeError("fallback drill results: release_train must be 2027-01-31-bigbang-ga")

    print(
        "[OK] fallback drill results contract valid "
        f"({len(seen_workspace_ids)} workspaces, {len(covered_web_platforms)} web browsers covered)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
