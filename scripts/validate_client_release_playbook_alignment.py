#!/usr/bin/env python3
"""Validate release/rollback playbooks stay aligned with client release-track contract."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

EXPECTED_SCHEMA = "kaigi-client-release-tracks/v1"
EXPECTED_FROZEN_AT = "2026-02-15"

RELEASE_TABLE_HEADING = "## Client Release Track Contract"
ROLLBACK_TABLE_HEADING = "## Client Rollback Track Contract"

RELEASE_HEADERS = [
    "Workspace ID",
    "Platforms",
    "Artifact Kind",
    "Distribution Channel",
    "Signing Required",
    "HDR Validation Required",
]

ROLLBACK_HEADERS = [
    "Workspace ID",
    "Platforms",
    "Distribution Channel",
    "Signing Required",
    "Fallback Workspace",
]


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def split_table_row(line: str) -> list[str]:
    stripped = line.strip()
    if not (stripped.startswith("|") and stripped.endswith("|")):
        raise RuntimeError(f"invalid markdown table row: {line}")
    parts = [segment.strip() for segment in stripped[1:-1].split("|")]
    return parts


def parse_table(text: str, *, heading: str, headers: list[str], name: str) -> list[dict[str, str]]:
    lines = text.splitlines()
    try:
        heading_index = next(i for i, line in enumerate(lines) if line.strip() == heading)
    except StopIteration as exc:
        raise RuntimeError(f"{name}: missing heading: {heading}") from exc

    cursor = heading_index + 1
    while cursor < len(lines) and not lines[cursor].strip().startswith("|"):
        cursor += 1
    if cursor >= len(lines):
        raise RuntimeError(f"{name}: missing markdown table after heading: {heading}")

    parsed_headers = split_table_row(lines[cursor])
    if parsed_headers != headers:
        raise RuntimeError(
            f"{name}: header mismatch for {heading}; expected {headers}, got {parsed_headers}"
        )

    cursor += 1
    if cursor >= len(lines) or not lines[cursor].strip().startswith("|"):
        raise RuntimeError(f"{name}: missing separator row for {heading}")

    cursor += 1
    rows: list[dict[str, str]] = []
    while cursor < len(lines):
        line = lines[cursor].strip()
        if not line.startswith("|"):
            break
        values = split_table_row(line)
        if len(values) != len(headers):
            raise RuntimeError(
                f"{name}: row column count mismatch in {heading}; expected {len(headers)} got {len(values)}"
            )
        rows.append(dict(zip(headers, values)))
        cursor += 1

    if not rows:
        raise RuntimeError(f"{name}: table under {heading} must contain at least one data row")
    return rows


def bool_token(value: object, *, field: str, workspace_id: str) -> str:
    if not isinstance(value, bool):
        raise RuntimeError(
            f"release track {workspace_id}: {field} must be boolean in contract"
        )
    return "true" if value else "false"


def build_expected_rows(release_tracks: list[dict]) -> tuple[list[dict[str, str]], list[dict[str, str]]]:
    ordered_tracks = sorted(release_tracks, key=lambda entry: str(entry.get("workspace_id", "")))
    release_rows: list[dict[str, str]] = []
    rollback_rows: list[dict[str, str]] = []

    for track in ordered_tracks:
        workspace_id = track.get("workspace_id")
        if not isinstance(workspace_id, str) or not workspace_id:
            raise RuntimeError("release track contract has invalid workspace_id")

        platforms = track.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(
                f"release track {workspace_id}: platforms must be non-empty array"
            )
        platform_tokens = []
        for platform in platforms:
            if not isinstance(platform, str) or not platform.strip():
                raise RuntimeError(
                    f"release track {workspace_id}: platforms must contain non-empty strings"
                )
            platform_tokens.append(platform.strip())

        artifact_kind = track.get("artifact_kind")
        distribution_channel = track.get("distribution_channel")
        if not isinstance(artifact_kind, str) or not artifact_kind.strip():
            raise RuntimeError(
                f"release track {workspace_id}: artifact_kind must be non-empty string"
            )
        if not isinstance(distribution_channel, str) or not distribution_channel.strip():
            raise RuntimeError(
                f"release track {workspace_id}: distribution_channel must be non-empty string"
            )

        release_rows.append(
            {
                "Workspace ID": workspace_id,
                "Platforms": ", ".join(platform_tokens),
                "Artifact Kind": artifact_kind.strip(),
                "Distribution Channel": distribution_channel.strip(),
                "Signing Required": bool_token(
                    track.get("signing_required"),
                    field="signing_required",
                    workspace_id=workspace_id,
                ),
                "HDR Validation Required": bool_token(
                    track.get("hdr_validation_required"),
                    field="hdr_validation_required",
                    workspace_id=workspace_id,
                ),
            }
        )

        fallback_workspace = track.get("fallback_workspace")
        fallback_token = "-"
        if fallback_workspace is not None:
            if not isinstance(fallback_workspace, str) or not fallback_workspace.strip():
                raise RuntimeError(
                    f"release track {workspace_id}: fallback_workspace must be non-empty string when present"
                )
            fallback_token = fallback_workspace.strip()

        rollback_rows.append(
            {
                "Workspace ID": workspace_id,
                "Platforms": ", ".join(platform_tokens),
                "Distribution Channel": distribution_channel.strip(),
                "Signing Required": bool_token(
                    track.get("signing_required"),
                    field="signing_required",
                    workspace_id=workspace_id,
                ),
                "Fallback Workspace": fallback_token,
            }
        )

    return release_rows, rollback_rows


def compare_rows(
    *,
    expected_rows: list[dict[str, str]],
    actual_rows: list[dict[str, str]],
    headers: list[str],
    table_name: str,
) -> None:
    if len(actual_rows) != len(expected_rows):
        raise RuntimeError(
            f"{table_name}: expected {len(expected_rows)} rows, got {len(actual_rows)}"
        )

    actual_by_workspace: dict[str, dict[str, str]] = {}
    for row in actual_rows:
        workspace_id = row.get("Workspace ID", "")
        if workspace_id in actual_by_workspace:
            raise RuntimeError(f"{table_name}: duplicate Workspace ID row: {workspace_id}")
        actual_by_workspace[workspace_id] = row

    expected_ids = [row["Workspace ID"] for row in expected_rows]
    actual_ids = sorted(actual_by_workspace)
    if actual_ids != expected_ids:
        raise RuntimeError(
            f"{table_name}: workspace row set mismatch; expected {expected_ids}, got {actual_ids}"
        )

    for expected in expected_rows:
        workspace_id = expected["Workspace ID"]
        actual = actual_by_workspace[workspace_id]
        for header in headers:
            expected_value = expected[header]
            actual_value = actual.get(header, "")
            if actual_value != expected_value:
                raise RuntimeError(
                    f"{table_name}: workspace {workspace_id} column {header!r} mismatch; "
                    f"expected {expected_value!r}, got {actual_value!r}"
                )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-tracks",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-release-tracks.json"),
    )
    parser.add_argument(
        "--release-playbook",
        type=pathlib.Path,
        default=pathlib.Path("docs/release-playbook.md"),
    )
    parser.add_argument(
        "--rollback-playbook",
        type=pathlib.Path,
        default=pathlib.Path("docs/rollback-playbook.md"),
    )
    args = parser.parse_args()

    contract = load_json(args.release_tracks)
    if contract.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client release track schema")
    if contract.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client release track contract")

    release_tracks = contract.get("release_tracks")
    if not isinstance(release_tracks, list) or not release_tracks:
        raise RuntimeError("release_tracks must be a non-empty array")

    expected_release_rows, expected_rollback_rows = build_expected_rows(release_tracks)

    release_text = args.release_playbook.read_text(encoding="utf-8")
    rollback_text = args.rollback_playbook.read_text(encoding="utf-8")

    release_rows = parse_table(
        release_text,
        heading=RELEASE_TABLE_HEADING,
        headers=RELEASE_HEADERS,
        name="release-playbook",
    )
    rollback_rows = parse_table(
        rollback_text,
        heading=ROLLBACK_TABLE_HEADING,
        headers=ROLLBACK_HEADERS,
        name="rollback-playbook",
    )

    compare_rows(
        expected_rows=expected_release_rows,
        actual_rows=release_rows,
        headers=RELEASE_HEADERS,
        table_name="release-playbook contract table",
    )
    compare_rows(
        expected_rows=expected_rollback_rows,
        actual_rows=rollback_rows,
        headers=ROLLBACK_HEADERS,
        table_name="rollback-playbook contract table",
    )

    print(
        "[OK] client release playbook alignment valid "
        f"({len(expected_release_rows)} workspaces, schema={EXPECTED_SCHEMA})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
