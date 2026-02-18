#!/usr/bin/env python3
"""Validate frozen client release-track contract against workspace/platform contract."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

EXPECTED_SCHEMA = "kaigi-client-release-tracks/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
EXPECTED_WORKSPACE_SCHEMA = "kaigi-client-app-workspaces/v1"
EXPECTED_WORKSPACE_FROZEN_AT = "2026-02-15"
EXPECTED_RELEASE_CHANNELS = ["alpha", "beta", "ga"]
WORKSPACE_ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def require_non_empty_str(entry: dict, field: str, workspace_id: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(
            f"release track {workspace_id}: {field} must be a non-empty string"
        )
    return value.strip()


def require_string_list(entry: dict, field: str, workspace_id: str) -> list[str]:
    value = entry.get(field)
    if not isinstance(value, list) or not value:
        raise RuntimeError(
            f"release track {workspace_id}: {field} must be a non-empty array"
        )
    out: list[str] = []
    for item in value:
        if not isinstance(item, str) or not item.strip():
            raise RuntimeError(
                f"release track {workspace_id}: {field} values must be non-empty strings"
            )
        out.append(item.strip())
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-tracks",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-tracks.json"),
    )
    parser.add_argument(
        "--workspaces",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-app-workspaces.json"),
    )
    args = parser.parse_args()

    release_contract = load_json(args.release_tracks)
    if release_contract.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client release track contract schema")
    if release_contract.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release track contract")

    workspace_contract = load_json(args.workspaces)
    if workspace_contract.get("schema") != EXPECTED_WORKSPACE_SCHEMA:
        raise RuntimeError("unexpected client workspace contract schema")
    if workspace_contract.get("frozen_at") != EXPECTED_WORKSPACE_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client workspace contract")

    workspace_entries = workspace_contract.get("workspaces")
    if not isinstance(workspace_entries, list) or not workspace_entries:
        raise RuntimeError("workspace contract must contain non-empty workspaces array")

    workspace_by_id: dict[str, dict] = {}
    for workspace in workspace_entries:
        if not isinstance(workspace, dict):
            raise RuntimeError("workspace entries must be objects")
        workspace_id = workspace.get("id")
        if not isinstance(workspace_id, str) or not workspace_id:
            raise RuntimeError("workspace id must be a non-empty string")
        if workspace_id in workspace_by_id:
            raise RuntimeError(f"duplicate workspace id in workspace contract: {workspace_id}")

        implementation = workspace.get("implementation")
        if implementation not in {"web", "native"}:
            raise RuntimeError(
                f"workspace {workspace_id}: implementation must be web|native"
            )

        platforms = workspace.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(
                f"workspace {workspace_id}: platforms must be a non-empty array"
            )
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"workspace {workspace_id}: platforms must contain non-empty strings"
                )

        workspace_by_id[workspace_id] = workspace

    tracks = release_contract.get("release_tracks")
    if not isinstance(tracks, list) or not tracks:
        raise RuntimeError("release_tracks must be a non-empty array")

    seen_workspace_ids: set[str] = set()
    ordered_workspace_ids: list[str] = []

    for track in tracks:
        if not isinstance(track, dict):
            raise RuntimeError("release_track entries must be objects")

        workspace_id = track.get("workspace_id")
        if not isinstance(workspace_id, str) or not workspace_id:
            raise RuntimeError("release_track workspace_id must be non-empty string")
        if not WORKSPACE_ID_RE.fullmatch(workspace_id):
            raise RuntimeError(
                f"release_track workspace_id must match {WORKSPACE_ID_RE.pattern}: {workspace_id}"
            )
        if workspace_id in seen_workspace_ids:
            raise RuntimeError(f"duplicate release_track workspace_id: {workspace_id}")
        if workspace_id not in workspace_by_id:
            raise RuntimeError(
                f"release_track references unknown workspace_id: {workspace_id}"
            )

        seen_workspace_ids.add(workspace_id)
        ordered_workspace_ids.append(workspace_id)

        workspace = workspace_by_id[workspace_id]
        implementation = workspace["implementation"]
        workspace_platforms = workspace["platforms"]

        platforms = require_string_list(track, "platforms", workspace_id)
        if platforms != workspace_platforms:
            raise RuntimeError(
                f"release track {workspace_id}: platforms must match workspace contract"
            )

        for field in (
            "ci_build_command",
            "ci_smoke_command",
            "artifact_kind",
            "distribution_channel",
        ):
            require_non_empty_str(track, field, workspace_id)

        release_channels = require_string_list(track, "release_channels", workspace_id)
        if release_channels != EXPECTED_RELEASE_CHANNELS:
            raise RuntimeError(
                "release track "
                f"{workspace_id}: release_channels must be {EXPECTED_RELEASE_CHANNELS}"
            )

        hdr_validation_required = track.get("hdr_validation_required")
        if not isinstance(hdr_validation_required, bool):
            raise RuntimeError(
                f"release track {workspace_id}: hdr_validation_required must be boolean"
            )
        if not hdr_validation_required:
            raise RuntimeError(
                f"release track {workspace_id}: hdr_validation_required must be true"
            )

        signing_required = track.get("signing_required")
        if not isinstance(signing_required, bool):
            raise RuntimeError(
                f"release track {workspace_id}: signing_required must be boolean"
            )

        artifact_kind = track["artifact_kind"]
        distribution_channel = track["distribution_channel"]
        fallback_workspace = track.get("fallback_workspace")

        if implementation == "web":
            if fallback_workspace is not None:
                raise RuntimeError(
                    f"release track {workspace_id}: web workspace must not set fallback_workspace"
                )
            if artifact_kind != "ipfs-web-bundle":
                raise RuntimeError(
                    f"release track {workspace_id}: web artifact_kind must be ipfs-web-bundle"
                )
            if distribution_channel != "ipfs":
                raise RuntimeError(
                    f"release track {workspace_id}: web distribution_channel must be ipfs"
                )
            if signing_required:
                raise RuntimeError(
                    f"release track {workspace_id}: web signing_required must be false"
                )
        else:
            expected_fallback_workspace = workspace.get("web_fallback_workspace")
            if (
                not isinstance(fallback_workspace, str)
                or not fallback_workspace.strip()
                or fallback_workspace.strip() != expected_fallback_workspace
            ):
                raise RuntimeError(
                    "release track "
                    f"{workspace_id}: fallback_workspace must match workspace contract"
                )
            if not signing_required:
                raise RuntimeError(
                    f"release track {workspace_id}: native signing_required must be true"
                )
            if artifact_kind == "ipfs-web-bundle":
                raise RuntimeError(
                    f"release track {workspace_id}: native artifact_kind cannot be ipfs-web-bundle"
                )
            if distribution_channel == "ipfs":
                raise RuntimeError(
                    f"release track {workspace_id}: native distribution_channel cannot be ipfs"
                )

    expected_workspace_ids = set(workspace_by_id)
    missing_workspace_ids = sorted(expected_workspace_ids - seen_workspace_ids)
    if missing_workspace_ids:
        raise RuntimeError(
            "release track contract missing workspace IDs: "
            + ", ".join(missing_workspace_ids)
        )

    if ordered_workspace_ids != sorted(ordered_workspace_ids):
        raise RuntimeError(
            "release_tracks must be sorted by workspace_id for deterministic diffs"
        )

    print(
        "[OK] client release track contract valid "
        f"({len(seen_workspace_ids)} workspaces, schema={EXPECTED_SCHEMA})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
