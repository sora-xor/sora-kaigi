#!/usr/bin/env python3
"""Generate docs/client-release-manifest.json from release tracks and artifact metadata.

This tool is intended for release-engineering handoff preparation. It keeps
platform/workspace topology in sync with docs/client-release-tracks.json and
fills mutable artifact fields from a metadata input file (URI, checksums,
signature/SBOM/provenance refs, publish timestamp).
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import sys
from typing import Any

EXPECTED_RELEASE_TRACK_SCHEMA = "kaigi-client-release-tracks/v1"
DEFAULT_MANIFEST_SCHEMA = "kaigi-client-release-manifest/v1"
DEFAULT_RELEASE_TRAIN = "2027-01-31-bigbang-ga"
ISO_Z_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def load_json(path: pathlib.Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def require_non_empty_str(value: Any, *, field: str, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"{context}: {field} must be a non-empty string")
    return value.strip()


def parse_release_tracks(path: pathlib.Path) -> dict[str, dict[str, Any]]:
    payload = load_json(path)
    if payload.get("schema") != EXPECTED_RELEASE_TRACK_SCHEMA:
        raise RuntimeError(f"unexpected release track schema in {path}")

    tracks = payload.get("release_tracks")
    if not isinstance(tracks, list) or not tracks:
        raise RuntimeError("release_tracks must be a non-empty array")

    out: dict[str, dict[str, Any]] = {}
    for track in tracks:
        if not isinstance(track, dict):
            raise RuntimeError("release_tracks entries must be objects")
        workspace_id = require_non_empty_str(track.get("workspace_id"), field="workspace_id", context="release track")
        if workspace_id in out:
            raise RuntimeError(f"duplicate workspace_id in release tracks: {workspace_id}")

        platforms = track.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(f"release track {workspace_id}: platforms must be non-empty array")
        normalized_platforms: list[str] = []
        for platform in platforms:
            normalized_platforms.append(
                require_non_empty_str(
                    platform,
                    field="platforms[]",
                    context=f"release track {workspace_id}",
                )
            )

        distribution_channel = require_non_empty_str(
            track.get("distribution_channel"),
            field="distribution_channel",
            context=f"release track {workspace_id}",
        )
        signing_required = track.get("signing_required")
        if not isinstance(signing_required, bool):
            raise RuntimeError(f"release track {workspace_id}: signing_required must be boolean")

        out[workspace_id] = {
            "platforms": normalized_platforms,
            "distribution_channel": distribution_channel,
            "signing_required": signing_required,
        }
    return out


def parse_existing_manifest(path: pathlib.Path) -> tuple[str, str | None, str | None, dict[str, dict[str, Any]]]:
    if not path.exists():
        return DEFAULT_MANIFEST_SCHEMA, None, None, {}

    payload = load_json(path)
    schema = payload.get("schema")
    if not isinstance(schema, str) or not schema.strip():
        schema = DEFAULT_MANIFEST_SCHEMA
    else:
        schema = schema.strip()

    frozen_at = payload.get("frozen_at")
    if isinstance(frozen_at, str):
        frozen_at = frozen_at.strip() or None
    else:
        frozen_at = None

    release_train = payload.get("release_train")
    if isinstance(release_train, str):
        release_train = release_train.strip() or None
    else:
        release_train = None

    artifacts = payload.get("artifacts")
    out: dict[str, dict[str, Any]] = {}
    if isinstance(artifacts, list):
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                continue
            workspace_id = artifact.get("workspace_id")
            if isinstance(workspace_id, str) and workspace_id.strip():
                out[workspace_id.strip()] = artifact

    return schema, frozen_at, release_train, out


def parse_metadata(path: pathlib.Path) -> dict[str, dict[str, Any]]:
    payload = load_json(path)
    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise RuntimeError(f"{path}: artifacts must be a non-empty array")

    out: dict[str, dict[str, Any]] = {}
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise RuntimeError(f"{path}: artifact entries must be objects")
        workspace_id = require_non_empty_str(
            artifact.get("workspace_id"),
            field="workspace_id",
            context=f"metadata artifact ({path})",
        )
        if workspace_id in out:
            raise RuntimeError(f"{path}: duplicate workspace_id {workspace_id}")
        out[workspace_id] = artifact
    return out


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def choose_str(
    workspace_id: str,
    field: str,
    *,
    metadata: dict[str, Any] | None,
    existing: dict[str, Any] | None,
    default: str | None = None,
) -> str:
    if metadata is not None and field in metadata:
        return require_non_empty_str(
            metadata[field],
            field=field,
            context=f"metadata artifact {workspace_id}",
        )
    if existing is not None and field in existing:
        return require_non_empty_str(
            existing[field],
            field=field,
            context=f"existing artifact {workspace_id}",
        )
    if default is not None:
        return default
    raise RuntimeError(f"workspace {workspace_id}: missing required field {field}")


def choose_bool(
    workspace_id: str,
    field: str,
    *,
    metadata: dict[str, Any] | None,
    existing: dict[str, Any] | None,
    default: bool,
) -> bool:
    if metadata is not None and field in metadata:
        value = metadata[field]
        if not isinstance(value, bool):
            raise RuntimeError(f"metadata artifact {workspace_id}: {field} must be boolean")
        return value
    if existing is not None and field in existing:
        value = existing[field]
        if not isinstance(value, bool):
            raise RuntimeError(f"existing artifact {workspace_id}: {field} must be boolean")
        return value
    return default


def choose_status(
    workspace_id: str,
    *,
    metadata: dict[str, Any] | None,
    existing: dict[str, Any] | None,
) -> str:
    status = choose_str(
        workspace_id,
        "status",
        metadata=metadata,
        existing=existing,
        default="ready",
    )
    if status != "ready":
        raise RuntimeError(f"workspace {workspace_id}: status must be ready")
    return status


def choose_published_at(
    workspace_id: str,
    *,
    metadata: dict[str, Any] | None,
    existing: dict[str, Any] | None,
    default_published_at: str,
) -> str:
    value = choose_str(
        workspace_id,
        "published_at",
        metadata=metadata,
        existing=existing,
        default=default_published_at,
    )
    if not ISO_Z_RE.fullmatch(value):
        raise RuntimeError(
            f"workspace {workspace_id}: published_at must match RFC3339 second precision Z"
        )
    return value


def choose_artifact_sha(
    workspace_id: str,
    *,
    metadata: dict[str, Any] | None,
    existing: dict[str, Any] | None,
) -> str:
    if metadata is not None and "artifact_path" in metadata:
        artifact_path = pathlib.Path(
            require_non_empty_str(
                metadata["artifact_path"],
                field="artifact_path",
                context=f"metadata artifact {workspace_id}",
            )
        )
        if not artifact_path.exists() or not artifact_path.is_file():
            raise RuntimeError(
                f"metadata artifact {workspace_id}: artifact_path does not exist: {artifact_path}"
            )
        checksum = sha256_file(artifact_path)
    elif metadata is not None and "artifact_sha256" in metadata:
        checksum = require_non_empty_str(
            metadata["artifact_sha256"],
            field="artifact_sha256",
            context=f"metadata artifact {workspace_id}",
        ).lower()
    elif existing is not None and "artifact_sha256" in existing:
        checksum = require_non_empty_str(
            existing["artifact_sha256"],
            field="artifact_sha256",
            context=f"existing artifact {workspace_id}",
        ).lower()
    else:
        raise RuntimeError(
            f"workspace {workspace_id}: provide artifact_path or artifact_sha256 in metadata"
        )

    if not SHA256_RE.fullmatch(checksum):
        raise RuntimeError(
            f"workspace {workspace_id}: artifact_sha256 must match {SHA256_RE.pattern}"
        )
    return checksum


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-tracks",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-tracks.json"),
        help="Path to client release tracks contract JSON.",
    )
    parser.add_argument(
        "--metadata",
        type=pathlib.Path,
        required=True,
        help="Path to manifest metadata input JSON.",
    )
    parser.add_argument(
        "--existing-manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-manifest.json"),
        help="Existing manifest used for defaults/backfill.",
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-manifest.json"),
        help="Output manifest path.",
    )
    parser.add_argument(
        "--schema",
        default=None,
        help="Override schema string for output manifest.",
    )
    parser.add_argument(
        "--frozen-at",
        dest="frozen_at",
        default=None,
        help="Override frozen_at date (YYYY-MM-DD).",
    )
    parser.add_argument(
        "--release-train",
        default=None,
        help="Override release_train value.",
    )
    parser.add_argument(
        "--published-at",
        dest="published_at",
        default=None,
        help="Default published_at timestamp for entries lacking explicit value.",
    )
    args = parser.parse_args()

    track_by_workspace = parse_release_tracks(args.release_tracks)
    metadata_by_workspace = parse_metadata(args.metadata)
    (
        existing_schema,
        existing_frozen_at,
        existing_release_train,
        existing_by_workspace,
    ) = parse_existing_manifest(args.existing_manifest)

    unknown_metadata_workspaces = sorted(
        set(metadata_by_workspace.keys()) - set(track_by_workspace.keys())
    )
    if unknown_metadata_workspaces:
        raise RuntimeError(
            "metadata includes workspace(s) missing from release tracks: "
            + ", ".join(unknown_metadata_workspaces)
        )

    schema = args.schema or existing_schema or DEFAULT_MANIFEST_SCHEMA
    frozen_at = args.frozen_at or existing_frozen_at
    if not frozen_at:
        tracks_payload = load_json(args.release_tracks)
        candidate = tracks_payload.get("frozen_at")
        if isinstance(candidate, str) and candidate.strip():
            frozen_at = candidate.strip()
    if not frozen_at:
        raise RuntimeError("frozen_at is required (pass --frozen-at or provide existing manifest)")

    release_train = args.release_train or existing_release_train or DEFAULT_RELEASE_TRAIN
    published_at_default = args.published_at or utc_now_iso()
    if not ISO_Z_RE.fullmatch(published_at_default):
        raise RuntimeError("--published-at must be RFC3339 second precision Z timestamp")

    artifacts: list[dict[str, Any]] = []
    for workspace_id in sorted(track_by_workspace):
        track = track_by_workspace[workspace_id]
        metadata = metadata_by_workspace.get(workspace_id)
        existing = existing_by_workspace.get(workspace_id)
        if metadata is None and existing is None:
            raise RuntimeError(
                f"workspace {workspace_id}: missing metadata and no existing manifest entry to backfill"
            )

        artifact_uri = choose_str(
            workspace_id,
            "artifact_uri",
            metadata=metadata,
            existing=existing,
        )
        signature_ref = choose_str(
            workspace_id,
            "signature_ref",
            metadata=metadata,
            existing=existing,
        )
        sbom_ref = choose_str(
            workspace_id,
            "sbom_ref",
            metadata=metadata,
            existing=existing,
        )
        provenance_ref = choose_str(
            workspace_id,
            "provenance_ref",
            metadata=metadata,
            existing=existing,
        )
        artifact_sha256 = choose_artifact_sha(
            workspace_id,
            metadata=metadata,
            existing=existing,
        )
        signing_verified = choose_bool(
            workspace_id,
            "signing_verified",
            metadata=metadata,
            existing=existing,
            default=bool(track["signing_required"]),
        )
        status = choose_status(
            workspace_id,
            metadata=metadata,
            existing=existing,
        )
        published_at = choose_published_at(
            workspace_id,
            metadata=metadata,
            existing=existing,
            default_published_at=published_at_default,
        )

        artifacts.append(
            {
                "workspace_id": workspace_id,
                "platforms": track["platforms"],
                "distribution_channel": track["distribution_channel"],
                "artifact_uri": artifact_uri,
                "artifact_sha256": artifact_sha256,
                "signature_ref": signature_ref,
                "sbom_ref": sbom_ref,
                "provenance_ref": provenance_ref,
                "signing_verified": signing_verified,
                "status": status,
                "published_at": published_at,
            }
        )

    output_payload = {
        "schema": schema,
        "frozen_at": frozen_at,
        "release_train": release_train,
        "artifacts": artifacts,
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output_payload, indent=2) + "\n", encoding="utf-8")

    updated_workspaces = sorted(metadata_by_workspace.keys())
    print(f"[OK] wrote {args.output}")
    print(f"updated workspaces from metadata: {', '.join(updated_workspaces)}")
    print(f"release_train: {release_train}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
