#!/usr/bin/env python3
"""Validate frozen client rollback manifest against release contracts."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import sys

EXPECTED_SCHEMA = "kaigi-client-rollback-manifest/v1"
EXPECTED_RELEASE_TRACK_SCHEMA = "kaigi-client-release-tracks/v1"
EXPECTED_RELEASE_MANIFEST_SCHEMA = "kaigi-client-release-manifest/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
EXPECTED_RELEASE_TRAIN = "2027-01-31-bigbang-ga"
ALLOWED_STATUS = {"ready"}
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
ISO_Z_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def require_non_empty_str(entry: dict, field: str, context: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"{context}: {field} must be non-empty string")
    return value.strip()


def parse_release_tracks(payload: dict) -> dict[str, dict]:
    if payload.get("schema") != EXPECTED_RELEASE_TRACK_SCHEMA:
        raise RuntimeError("unexpected client release track schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release tracks")

    tracks = payload.get("release_tracks")
    if not isinstance(tracks, list) or not tracks:
        raise RuntimeError("client release track contract: release_tracks must be non-empty array")

    out: dict[str, dict] = {}
    for track in tracks:
        if not isinstance(track, dict):
            raise RuntimeError("client release track contract: entries must be objects")

        workspace_id = require_non_empty_str(track, "workspace_id", "release track")
        if workspace_id in out:
            raise RuntimeError(f"client release track contract: duplicate workspace_id {workspace_id}")

        platforms = track.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(f"release track {workspace_id}: platforms must be non-empty array")

        platform_values: list[str] = []
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"release track {workspace_id}: platforms must contain non-empty strings"
                )
            platform_values.append(platform.strip())

        distribution_channel = require_non_empty_str(
            track, "distribution_channel", f"release track {workspace_id}"
        )
        signing_required = track.get("signing_required")
        if not isinstance(signing_required, bool):
            raise RuntimeError(
                f"release track {workspace_id}: signing_required must be boolean"
            )

        out[workspace_id] = {
            "platforms": platform_values,
            "distribution_channel": distribution_channel,
            "signing_required": signing_required,
        }

    return out


def parse_release_manifest(payload: dict) -> dict[str, dict]:
    if payload.get("schema") != EXPECTED_RELEASE_MANIFEST_SCHEMA:
        raise RuntimeError("unexpected client release manifest schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release manifest")
    if payload.get("release_train") != EXPECTED_RELEASE_TRAIN:
        raise RuntimeError("unexpected release_train in client release manifest")

    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise RuntimeError("client release manifest artifacts must be non-empty array")

    out: dict[str, dict] = {}
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise RuntimeError("client release manifest artifact entries must be objects")

        workspace_id = require_non_empty_str(artifact, "workspace_id", "artifact")
        if workspace_id in out:
            raise RuntimeError(f"duplicate client release manifest workspace_id: {workspace_id}")

        artifact_uri = require_non_empty_str(
            artifact, "artifact_uri", f"artifact {workspace_id}"
        )
        status = require_non_empty_str(artifact, "status", f"artifact {workspace_id}")

        out[workspace_id] = {
            "artifact_uri": artifact_uri,
            "status": status,
        }

    return out


def parse_iso_z(value: str, *, context: str) -> dt.datetime:
    if not ISO_Z_RE.fullmatch(value):
        raise RuntimeError(f"{context}: published_at must be RFC3339 second precision Z timestamp")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rollback-manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-rollback-manifest.json"),
    )
    parser.add_argument(
        "--release-tracks",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-tracks.json"),
    )
    parser.add_argument(
        "--release-manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-manifest.json"),
    )
    args = parser.parse_args()

    track_by_workspace = parse_release_tracks(load_json(args.release_tracks))
    release_manifest_by_workspace = parse_release_manifest(load_json(args.release_manifest))

    payload = load_json(args.rollback_manifest)
    if payload.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client rollback manifest schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client rollback manifest")

    release_train = require_non_empty_str(payload, "release_train", "client rollback manifest")
    if release_train != EXPECTED_RELEASE_TRAIN:
        raise RuntimeError(
            f"client rollback manifest: release_train must be {EXPECTED_RELEASE_TRAIN}"
        )

    rollback_target_release_train = require_non_empty_str(
        payload,
        "rollback_target_release_train",
        "client rollback manifest",
    )
    if rollback_target_release_train == release_train:
        raise RuntimeError(
            "client rollback manifest: rollback_target_release_train must differ from release_train"
        )

    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise RuntimeError("client rollback manifest: artifacts must be non-empty array")

    seen_workspace_ids: set[str] = set()
    ordered_workspace_ids: list[str] = []
    web_workspace_count = 0
    native_workspace_count = 0

    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise RuntimeError("client rollback manifest: artifact entries must be objects")

        workspace_id = require_non_empty_str(artifact, "workspace_id", "artifact")
        if workspace_id in seen_workspace_ids:
            raise RuntimeError(f"client rollback manifest: duplicate workspace_id {workspace_id}")
        if workspace_id not in track_by_workspace:
            raise RuntimeError(f"client rollback manifest: unknown workspace_id {workspace_id}")
        if workspace_id not in release_manifest_by_workspace:
            raise RuntimeError(
                f"client rollback manifest: workspace_id {workspace_id} missing from release manifest"
            )

        seen_workspace_ids.add(workspace_id)
        ordered_workspace_ids.append(workspace_id)

        track = track_by_workspace[workspace_id]
        release_manifest_entry = release_manifest_by_workspace[workspace_id]

        platforms = artifact.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(f"artifact {workspace_id}: platforms must be non-empty array")

        platform_values: list[str] = []
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"artifact {workspace_id}: platforms must contain non-empty strings"
                )
            platform_values.append(platform.strip())

        if platform_values != track["platforms"]:
            raise RuntimeError(
                f"artifact {workspace_id}: platforms mismatch release tracks contract"
            )

        distribution_channel = require_non_empty_str(
            artifact, "distribution_channel", f"artifact {workspace_id}"
        )
        if distribution_channel != track["distribution_channel"]:
            raise RuntimeError(
                f"artifact {workspace_id}: distribution_channel mismatch; "
                f"expected {track['distribution_channel']}, got {distribution_channel}"
            )

        rollback_artifact_uri = require_non_empty_str(
            artifact, "rollback_artifact_uri", f"artifact {workspace_id}"
        )
        if rollback_artifact_uri == release_manifest_entry["artifact_uri"]:
            raise RuntimeError(
                f"artifact {workspace_id}: rollback_artifact_uri must differ from release artifact_uri"
            )

        if workspace_id == "web":
            web_workspace_count += 1
            if distribution_channel != "ipfs":
                raise RuntimeError("artifact web: distribution_channel must be ipfs")
            if not rollback_artifact_uri.startswith("ipfs://"):
                raise RuntimeError("artifact web: rollback_artifact_uri must start with ipfs://")
        else:
            native_workspace_count += 1
            if rollback_artifact_uri.startswith("ipfs://"):
                raise RuntimeError(
                    f"artifact {workspace_id}: native rollback_artifact_uri must not be ipfs://"
                )

        rollback_artifact_sha256 = require_non_empty_str(
            artifact, "rollback_artifact_sha256", f"artifact {workspace_id}"
        )
        if not SHA256_RE.fullmatch(rollback_artifact_sha256):
            raise RuntimeError(
                f"artifact {workspace_id}: rollback_artifact_sha256 must match {SHA256_RE.pattern}"
            )

        for field in ("rollback_signature_ref", "rollback_sbom_ref", "rollback_provenance_ref"):
            require_non_empty_str(artifact, field, f"artifact {workspace_id}")

        signing_verified = artifact.get("signing_verified")
        if not isinstance(signing_verified, bool):
            raise RuntimeError(f"artifact {workspace_id}: signing_verified must be boolean")
        if track["signing_required"] and not signing_verified:
            raise RuntimeError(
                f"artifact {workspace_id}: signing_verified must be true for signed release tracks"
            )

        status = require_non_empty_str(artifact, "status", f"artifact {workspace_id}")
        if status not in ALLOWED_STATUS:
            raise RuntimeError(
                f"artifact {workspace_id}: status must be one of {sorted(ALLOWED_STATUS)}"
            )
        if release_manifest_entry["status"] != "ready":
            raise RuntimeError(
                f"artifact {workspace_id}: release manifest status must be ready to allow rollback"
            )

        published_at = require_non_empty_str(artifact, "published_at", f"artifact {workspace_id}")
        parse_iso_z(published_at, context=f"artifact {workspace_id}")

    missing_workspace_ids = sorted(set(track_by_workspace) - seen_workspace_ids)
    if missing_workspace_ids:
        raise RuntimeError(
            "client rollback manifest missing workspace entries: "
            + ", ".join(missing_workspace_ids)
        )

    if ordered_workspace_ids != sorted(ordered_workspace_ids):
        raise RuntimeError("client rollback manifest artifacts must be sorted by workspace_id")

    if web_workspace_count != 1:
        raise RuntimeError("client rollback manifest must include exactly one web artifact entry")
    if native_workspace_count == 0:
        raise RuntimeError("client rollback manifest must include native artifact entries")

    print(
        "[OK] client rollback manifest valid "
        f"({len(seen_workspace_ids)} workspaces, release_train={release_train}, "
        f"rollback_target={rollback_target_release_train})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
