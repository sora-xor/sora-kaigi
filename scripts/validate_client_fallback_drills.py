#!/usr/bin/env python3
"""Validate native-to-web fallback drill contract against workspace and release-track contracts."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

EXPECTED_SCHEMA = "kaigi-client-fallback-drills/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
EXPECTED_WORKSPACE_SCHEMA = "kaigi-client-app-workspaces/v1"
EXPECTED_RELEASE_TRACK_SCHEMA = "kaigi-client-release-tracks/v1"
ALLOWED_DRILL_TRIGGER = "native-release-blocking-regression"
ALLOWED_CADENCE = "every-release-train"
OWNER_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)+$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def require_non_empty_str(entry: dict, field: str, context: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"{context}: {field} must be non-empty string")
    return value.strip()


def parse_workspace_contract(payload: dict) -> tuple[str, dict[str, dict]]:
    if payload.get("schema") != EXPECTED_WORKSPACE_SCHEMA:
        raise RuntimeError("unexpected client app workspace schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client app workspace contract")

    workspaces = payload.get("workspaces")
    if not isinstance(workspaces, list) or not workspaces:
        raise RuntimeError("client app workspace contract: workspaces must be non-empty array")

    web_workspace_id = ""
    native_workspaces: dict[str, dict] = {}

    for workspace in workspaces:
        if not isinstance(workspace, dict):
            raise RuntimeError("client app workspace contract: workspace entries must be objects")

        workspace_id = require_non_empty_str(workspace, "id", "workspace")
        implementation = workspace.get("implementation")
        if implementation not in {"web", "native"}:
            raise RuntimeError(
                f"workspace {workspace_id}: implementation must be web|native"
            )

        platforms = workspace.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(f"workspace {workspace_id}: platforms must be non-empty array")
        platform_values = []
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"workspace {workspace_id}: platforms must contain non-empty strings"
                )
            platform_values.append(platform.strip())

        if implementation == "web":
            if web_workspace_id:
                raise RuntimeError("client app workspace contract: expected exactly one web workspace")
            web_workspace_id = workspace_id
            continue

        fallback_workspace = require_non_empty_str(
            workspace, "web_fallback_workspace", f"workspace {workspace_id}"
        )
        if len(platform_values) != 1:
            raise RuntimeError(
                f"workspace {workspace_id}: native workspace must map exactly one platform"
            )

        native_workspaces[workspace_id] = {
            "platform": platform_values[0],
            "fallback_workspace": fallback_workspace,
        }

    if not web_workspace_id:
        raise RuntimeError("client app workspace contract: missing web workspace")
    return web_workspace_id, native_workspaces


def parse_release_tracks(payload: dict) -> dict[str, str]:
    if payload.get("schema") != EXPECTED_RELEASE_TRACK_SCHEMA:
        raise RuntimeError("unexpected client release track schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release track contract")

    tracks = payload.get("release_tracks")
    if not isinstance(tracks, list) or not tracks:
        raise RuntimeError("client release track contract: release_tracks must be non-empty array")

    distribution_by_workspace: dict[str, str] = {}
    for track in tracks:
        if not isinstance(track, dict):
            raise RuntimeError("client release track contract: release_track entries must be objects")

        workspace_id = require_non_empty_str(track, "workspace_id", "release track")
        distribution_channel = require_non_empty_str(
            track, "distribution_channel", f"release track {workspace_id}"
        )
        if workspace_id in distribution_by_workspace:
            raise RuntimeError(f"duplicate release track workspace_id: {workspace_id}")
        distribution_by_workspace[workspace_id] = distribution_channel

    return distribution_by_workspace


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fallback-drills",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-fallback-drills.json"),
    )
    parser.add_argument(
        "--workspaces",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-app-workspaces.json"),
    )
    parser.add_argument(
        "--release-tracks",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-tracks.json"),
    )
    args = parser.parse_args()

    fallback_contract = load_json(args.fallback_drills)
    if fallback_contract.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client fallback drill schema")
    if fallback_contract.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client fallback drill contract")

    web_workspace_id, native_workspaces = parse_workspace_contract(load_json(args.workspaces))
    distribution_by_workspace = parse_release_tracks(load_json(args.release_tracks))

    drills = fallback_contract.get("drills")
    if not isinstance(drills, list) or not drills:
        raise RuntimeError("client fallback drill contract: drills must be non-empty array")

    seen_workspace_ids: set[str] = set()
    ordered_workspace_ids: list[str] = []

    for drill in drills:
        if not isinstance(drill, dict):
            raise RuntimeError("client fallback drill contract: drill entries must be objects")

        workspace_id = require_non_empty_str(drill, "workspace_id", "drill")
        if workspace_id in seen_workspace_ids:
            raise RuntimeError(f"duplicate drill workspace_id: {workspace_id}")
        if workspace_id not in native_workspaces:
            raise RuntimeError(
                f"drill {workspace_id}: must reference native workspace in client-app-workspaces"
            )
        seen_workspace_ids.add(workspace_id)
        ordered_workspace_ids.append(workspace_id)

        workspace_meta = native_workspaces[workspace_id]
        expected_platform = workspace_meta["platform"]
        platform = require_non_empty_str(drill, "platform", f"drill {workspace_id}")
        if platform != expected_platform:
            raise RuntimeError(
                f"drill {workspace_id}: platform mismatch; expected {expected_platform}, got {platform}"
            )

        fallback_workspace = require_non_empty_str(
            drill, "fallback_workspace", f"drill {workspace_id}"
        )
        if fallback_workspace != web_workspace_id:
            raise RuntimeError(
                f"drill {workspace_id}: fallback_workspace must be {web_workspace_id}"
            )
        if fallback_workspace != workspace_meta["fallback_workspace"]:
            raise RuntimeError(
                f"drill {workspace_id}: fallback_workspace mismatch with workspace contract"
            )

        distribution_channel = require_non_empty_str(
            drill, "distribution_channel", f"drill {workspace_id}"
        )
        expected_distribution = distribution_by_workspace.get(workspace_id)
        if expected_distribution is None:
            raise RuntimeError(
                f"drill {workspace_id}: missing distribution channel in client release tracks"
            )
        if distribution_channel != expected_distribution:
            raise RuntimeError(
                f"drill {workspace_id}: distribution_channel mismatch; expected {expected_distribution}, got {distribution_channel}"
            )

        drill_trigger = require_non_empty_str(drill, "drill_trigger", f"drill {workspace_id}")
        if drill_trigger != ALLOWED_DRILL_TRIGGER:
            raise RuntimeError(
                f"drill {workspace_id}: drill_trigger must be {ALLOWED_DRILL_TRIGGER}"
            )

        max_rto_minutes = drill.get("max_rto_minutes")
        if not isinstance(max_rto_minutes, int):
            raise RuntimeError(f"drill {workspace_id}: max_rto_minutes must be integer")
        if max_rto_minutes < 1 or max_rto_minutes > 60:
            raise RuntimeError(
                f"drill {workspace_id}: max_rto_minutes must be in range [1, 60]"
            )

        validation_command = require_non_empty_str(
            drill, "validation_command", f"drill {workspace_id}"
        )
        if "web" not in validation_command.lower():
            raise RuntimeError(
                f"drill {workspace_id}: validation_command must reference web fallback validation"
            )

        cadence = require_non_empty_str(drill, "cadence", f"drill {workspace_id}")
        if cadence != ALLOWED_CADENCE:
            raise RuntimeError(
                f"drill {workspace_id}: cadence must be {ALLOWED_CADENCE}"
            )

        owner = require_non_empty_str(drill, "owner", f"drill {workspace_id}")
        if not OWNER_RE.fullmatch(owner):
            raise RuntimeError(
                f"drill {workspace_id}: owner must match {OWNER_RE.pattern}"
            )

    expected_native_workspaces = set(native_workspaces)
    missing_drills = sorted(expected_native_workspaces - seen_workspace_ids)
    if missing_drills:
        raise RuntimeError(
            "client fallback drill contract missing native workspace entries: "
            + ", ".join(missing_drills)
        )

    if ordered_workspace_ids != sorted(ordered_workspace_ids):
        raise RuntimeError("drills must be sorted by workspace_id for deterministic diffs")

    print(
        "[OK] client fallback drill contract valid "
        f"({len(seen_workspace_ids)} native workspaces, web fallback={web_workspace_id})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
