#!/usr/bin/env python3
"""Validate client release readiness gates against manifest and fallback drill evidence."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

EXPECTED_SCHEMA = "kaigi-client-release-readiness-gates/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
EXPECTED_RELEASE_TRAIN = "2027-01-31-bigbang-ga"

EXPECTED_RELEASE_TRACK_SCHEMA = "kaigi-client-release-tracks/v1"
EXPECTED_RELEASE_MANIFEST_SCHEMA = "kaigi-client-release-manifest/v1"
EXPECTED_FALLBACK_DRILLS_SCHEMA = "kaigi-client-fallback-drills/v1"
EXPECTED_FALLBACK_RESULTS_SCHEMA = "kaigi-client-fallback-drill-results/v1"


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
        raise RuntimeError("release tracks must be non-empty array")

    out: dict[str, dict] = {}
    for track in tracks:
        if not isinstance(track, dict):
            raise RuntimeError("release track entries must be objects")

        workspace_id = require_non_empty_str(track, "workspace_id", "release track")
        if workspace_id in out:
            raise RuntimeError(f"duplicate release track workspace_id: {workspace_id}")

        platforms = track.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(
                f"release track {workspace_id}: platforms must be non-empty array"
            )
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"release track {workspace_id}: platforms must contain non-empty strings"
                )

        implementation = "web" if workspace_id == "web" else "native"
        out[workspace_id] = {"implementation": implementation}

    return out


def parse_release_manifest(payload: dict) -> dict[str, str]:
    if payload.get("schema") != EXPECTED_RELEASE_MANIFEST_SCHEMA:
        raise RuntimeError("unexpected client release manifest schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release manifest")
    if payload.get("release_train") != EXPECTED_RELEASE_TRAIN:
        raise RuntimeError("unexpected release_train in client release manifest")

    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise RuntimeError("client release manifest artifacts must be non-empty array")

    out: dict[str, str] = {}
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise RuntimeError("client release manifest artifacts must be objects")
        workspace_id = require_non_empty_str(artifact, "workspace_id", "artifact")
        status = require_non_empty_str(artifact, "status", f"artifact {workspace_id}")
        if workspace_id in out:
            raise RuntimeError(f"duplicate client release manifest workspace_id: {workspace_id}")
        out[workspace_id] = status

    return out


def parse_fallback_drills(payload: dict) -> dict[str, int]:
    if payload.get("schema") != EXPECTED_FALLBACK_DRILLS_SCHEMA:
        raise RuntimeError("unexpected client fallback drills schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client fallback drills")

    drills = payload.get("drills")
    if not isinstance(drills, list) or not drills:
        raise RuntimeError("client fallback drills must be non-empty array")

    out: dict[str, int] = {}
    for drill in drills:
        if not isinstance(drill, dict):
            raise RuntimeError("client fallback drill entries must be objects")

        workspace_id = require_non_empty_str(drill, "workspace_id", "fallback drill")
        max_rto_minutes = drill.get("max_rto_minutes")
        if not isinstance(max_rto_minutes, int):
            raise RuntimeError(
                f"fallback drill {workspace_id}: max_rto_minutes must be integer"
            )
        if workspace_id in out:
            raise RuntimeError(f"duplicate fallback drill workspace_id: {workspace_id}")
        out[workspace_id] = max_rto_minutes

    return out


def parse_fallback_results(payload: dict) -> dict[str, dict]:
    if payload.get("schema") != EXPECTED_FALLBACK_RESULTS_SCHEMA:
        raise RuntimeError("unexpected client fallback drill results schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client fallback drill results")
    if payload.get("release_train") != EXPECTED_RELEASE_TRAIN:
        raise RuntimeError("unexpected release_train in client fallback drill results")

    results = payload.get("results")
    if not isinstance(results, list) or not results:
        raise RuntimeError("client fallback drill results must be non-empty array")

    out: dict[str, dict] = {}
    for result in results:
        if not isinstance(result, dict):
            raise RuntimeError("client fallback drill result entries must be objects")

        workspace_id = require_non_empty_str(result, "workspace_id", "fallback result")
        status = require_non_empty_str(result, "status", f"fallback result {workspace_id}")
        observed_rto = result.get("observed_rto_minutes")
        if not isinstance(observed_rto, int):
            raise RuntimeError(
                f"fallback result {workspace_id}: observed_rto_minutes must be integer"
            )
        if workspace_id in out:
            raise RuntimeError(f"duplicate fallback result workspace_id: {workspace_id}")

        out[workspace_id] = {"status": status, "observed_rto_minutes": observed_rto}

    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--readiness-gates",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-readiness-gates.json"),
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
    parser.add_argument(
        "--fallback-drills",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-fallback-drills.json"),
    )
    parser.add_argument(
        "--fallback-results",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-fallback-drill-results.json"),
    )
    args = parser.parse_args()

    release_tracks = parse_release_tracks(load_json(args.release_tracks))
    manifest_status_by_workspace = parse_release_manifest(load_json(args.release_manifest))
    fallback_max_rto_by_workspace = parse_fallback_drills(load_json(args.fallback_drills))
    fallback_result_by_workspace = parse_fallback_results(load_json(args.fallback_results))

    payload = load_json(args.readiness_gates)
    if payload.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client release readiness gates schema")
    if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release readiness gates")
    if payload.get("release_train") != EXPECTED_RELEASE_TRAIN:
        raise RuntimeError("unexpected release_train in client release readiness gates")

    gates = payload.get("gates")
    if not isinstance(gates, list) or not gates:
        raise RuntimeError("client release readiness gates must be non-empty array")

    seen_workspace_ids: set[str] = set()
    ordered_workspace_ids: list[str] = []

    for gate in gates:
        if not isinstance(gate, dict):
            raise RuntimeError("client release readiness gate entries must be objects")

        workspace_id = require_non_empty_str(gate, "workspace_id", "release readiness gate")
        if workspace_id in seen_workspace_ids:
            raise RuntimeError(f"duplicate release readiness gate workspace_id: {workspace_id}")
        if workspace_id not in release_tracks:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: missing from release tracks"
            )
        if workspace_id not in manifest_status_by_workspace:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: missing from release manifest"
            )

        seen_workspace_ids.add(workspace_id)
        ordered_workspace_ids.append(workspace_id)

        release_ready = gate.get("release_ready")
        if not isinstance(release_ready, bool):
            raise RuntimeError(
                f"release readiness gate {workspace_id}: release_ready must be boolean"
            )
        if not release_ready:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: release_ready must be true for this release train"
            )

        manifest_status = require_non_empty_str(
            gate, "manifest_status", f"release readiness gate {workspace_id}"
        )
        expected_manifest_status = manifest_status_by_workspace[workspace_id]
        if manifest_status != expected_manifest_status:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: manifest_status mismatch; "
                f"expected {expected_manifest_status}, got {manifest_status}"
            )

        fallback_drill_required = gate.get("fallback_drill_required")
        if not isinstance(fallback_drill_required, bool):
            raise RuntimeError(
                f"release readiness gate {workspace_id}: fallback_drill_required must be boolean"
            )

        fallback_drill_status = require_non_empty_str(
            gate, "fallback_drill_status", f"release readiness gate {workspace_id}"
        )

        max_rto = gate.get("max_rto_minutes")
        observed_rto = gate.get("observed_rto_minutes")

        evidence_refs = gate.get("evidence_refs")
        if not isinstance(evidence_refs, list) or not evidence_refs:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: evidence_refs must be non-empty array"
            )
        evidence_ref_set: set[str] = set()
        for item in evidence_refs:
            if not isinstance(item, str) or not item.strip():
                raise RuntimeError(
                    f"release readiness gate {workspace_id}: evidence_refs must contain non-empty strings"
                )
            evidence_ref_set.add(item.strip())

        if "docs/client-release-manifest.json" not in evidence_ref_set:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: evidence_refs must include docs/client-release-manifest.json"
            )

        implementation = release_tracks[workspace_id]["implementation"]
        if implementation == "web":
            if fallback_drill_required:
                raise RuntimeError(
                    f"release readiness gate {workspace_id}: web workspace must not require fallback drill"
                )
            if fallback_drill_status != "n/a":
                raise RuntimeError(
                    f"release readiness gate {workspace_id}: web fallback_drill_status must be n/a"
                )
            if max_rto is not None or observed_rto is not None:
                raise RuntimeError(
                    f"release readiness gate {workspace_id}: web max_rto_minutes/observed_rto_minutes must be null"
                )
            if "docs/client-fallback-drill-results.json" in evidence_ref_set:
                raise RuntimeError(
                    f"release readiness gate {workspace_id}: web evidence_refs must not include fallback drill results"
                )
            continue

        if not fallback_drill_required:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: native workspace must require fallback drill"
            )
        if workspace_id not in fallback_max_rto_by_workspace:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: missing fallback drill definition"
            )
        if workspace_id not in fallback_result_by_workspace:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: missing fallback drill result"
            )

        if not isinstance(max_rto, int):
            raise RuntimeError(
                f"release readiness gate {workspace_id}: max_rto_minutes must be integer"
            )
        if not isinstance(observed_rto, int):
            raise RuntimeError(
                f"release readiness gate {workspace_id}: observed_rto_minutes must be integer"
            )

        expected_max_rto = fallback_max_rto_by_workspace[workspace_id]
        if max_rto != expected_max_rto:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: max_rto_minutes mismatch; "
                f"expected {expected_max_rto}, got {max_rto}"
            )

        fallback_result = fallback_result_by_workspace[workspace_id]
        if fallback_drill_status != fallback_result["status"]:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: fallback_drill_status mismatch; "
                f"expected {fallback_result['status']}, got {fallback_drill_status}"
            )
        if fallback_drill_status != "passed":
            raise RuntimeError(
                f"release readiness gate {workspace_id}: fallback_drill_status must be passed"
            )

        expected_observed_rto = fallback_result["observed_rto_minutes"]
        if observed_rto != expected_observed_rto:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: observed_rto_minutes mismatch; "
                f"expected {expected_observed_rto}, got {observed_rto}"
            )
        if observed_rto > max_rto:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: observed_rto_minutes exceeds max_rto_minutes "
                f"({observed_rto} > {max_rto})"
            )

        if "docs/client-fallback-drill-results.json" not in evidence_ref_set:
            raise RuntimeError(
                f"release readiness gate {workspace_id}: evidence_refs must include docs/client-fallback-drill-results.json"
            )

    expected_workspace_ids = set(release_tracks)
    missing_workspace_ids = sorted(expected_workspace_ids - seen_workspace_ids)
    if missing_workspace_ids:
        raise RuntimeError(
            "client release readiness gates missing workspace entries: "
            + ", ".join(missing_workspace_ids)
        )

    if ordered_workspace_ids != sorted(ordered_workspace_ids):
        raise RuntimeError("client release readiness gates must be sorted by workspace_id")

    print(
        "[OK] client release readiness gates valid "
        f"({len(seen_workspace_ids)} workspaces, release_train={EXPECTED_RELEASE_TRAIN})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
