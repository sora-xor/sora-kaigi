#!/usr/bin/env python3
"""Validate parity waiver fixture manifest and fixture corpus integrity."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import sys

EXPECTED_MANIFEST_SCHEMA = "kaigi-parity-waiver-fixture-manifest/v1"
EXPECTED_WAIVER_SCHEMA = "kaigi-parity-status-waivers/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
TOKEN_RE = re.compile(r"__EXPIRES_PLUS_(\d+)_DAYS__")
FIXTURE_ID_RE = re.compile(r"^[a-z0-9]+(?:_[a-z0-9]+)*$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_utc_iso(value: str) -> dt.datetime:
    if not value.endswith("Z"):
        raise RuntimeError(f"invalid UTC timestamp (expected trailing Z): {value}")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def is_valid_expires_value(value: str) -> bool:
    if TOKEN_RE.fullmatch(value):
        return True
    try:
        parse_utc_iso(value)
    except RuntimeError:
        return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        default=pathlib.Path("docs/fixtures/waivers/manifest.json"),
    )
    args = parser.parse_args()

    manifest = load_json(args.manifest)
    if manifest.get("schema") != EXPECTED_MANIFEST_SCHEMA:
        raise RuntimeError("unexpected fixture manifest schema")
    if manifest.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in fixture manifest")

    fixtures = manifest.get("fixtures")
    if not isinstance(fixtures, list) or not fixtures:
        raise RuntimeError("fixture manifest must include non-empty fixtures array")

    seen_ids: set[str] = set()
    seen_files: set[str] = set()
    fixture_ids_in_order: list[str] = []
    now = dt.datetime.now(dt.timezone.utc)

    for fixture in fixtures:
        if not isinstance(fixture, dict):
            raise RuntimeError("fixture entries must be objects")

        fixture_id = fixture.get("id")
        fixture_file = fixture.get("fixture_file")
        expected = fixture.get("expected")
        expect_error_contains = fixture.get("expect_error_contains")

        if not isinstance(fixture_id, str) or not fixture_id:
            raise RuntimeError("fixture id must be non-empty string")
        if not FIXTURE_ID_RE.fullmatch(fixture_id):
            raise RuntimeError(
                f"fixture id must match {FIXTURE_ID_RE.pattern}: {fixture_id}"
            )
        if fixture_id in seen_ids:
            raise RuntimeError(f"duplicate fixture id: {fixture_id}")
        seen_ids.add(fixture_id)
        fixture_ids_in_order.append(fixture_id)

        if not isinstance(fixture_file, str) or not fixture_file:
            raise RuntimeError(f"{fixture_id}: fixture_file must be non-empty string")
        if not fixture_file.startswith("docs/fixtures/waivers/"):
            raise RuntimeError(f"{fixture_id}: fixture_file must be under docs/fixtures/waivers/")
        if fixture_file in seen_files:
            raise RuntimeError(f"{fixture_id}: duplicate fixture_file reference: {fixture_file}")
        seen_files.add(fixture_file)
        expected_filename = fixture_id.replace("_", "-") + ".json"
        if pathlib.Path(fixture_file).name != expected_filename:
            raise RuntimeError(
                f"{fixture_id}: fixture_file basename must be {expected_filename}"
            )

        if expected not in {"pass", "fail"}:
            raise RuntimeError(f"{fixture_id}: expected must be pass|fail")
        if expected == "fail":
            if not isinstance(expect_error_contains, str) or not expect_error_contains.strip():
                raise RuntimeError(f"{fixture_id}: failing fixture must set expect_error_contains")
        elif expect_error_contains is not None:
            raise RuntimeError(
                f"{fixture_id}: passing fixture must not set expect_error_contains"
            )

        fixture_path = pathlib.Path(fixture_file)
        if not fixture_path.is_file():
            raise RuntimeError(f"{fixture_id}: fixture file not found: {fixture_file}")

        payload = load_json(fixture_path)
        if payload.get("schema") != EXPECTED_WAIVER_SCHEMA:
            raise RuntimeError(f"{fixture_id}: unexpected waiver schema")
        if payload.get("frozen_at") != EXPECTED_FROZEN_AT:
            raise RuntimeError(f"{fixture_id}: unexpected waiver frozen_at")

        generated_at = payload.get("generated_at")
        if not isinstance(generated_at, str) or not generated_at.strip():
            raise RuntimeError(f"{fixture_id}: generated_at must be non-empty string")
        parse_utc_iso(generated_at)

        waivers = payload.get("waivers")
        if not isinstance(waivers, list) or not waivers:
            raise RuntimeError(f"{fixture_id}: waivers must be non-empty array")
        if len(waivers) != 1:
            raise RuntimeError(f"{fixture_id}: waivers must contain exactly one entry")

        for waiver in waivers:
            if not isinstance(waiver, dict):
                raise RuntimeError(f"{fixture_id}: waiver entries must be objects")
            expires_at = waiver.get("expires_at")
            if not isinstance(expires_at, str) or not expires_at.strip():
                raise RuntimeError(f"{fixture_id}: waiver expires_at must be non-empty string")
            if not is_valid_expires_value(expires_at):
                raise RuntimeError(f"{fixture_id}: invalid expires_at value: {expires_at}")
            token_match = TOKEN_RE.fullmatch(expires_at)
            if token_match:
                days = int(token_match.group(1))
                if days <= 0:
                    raise RuntimeError(f"{fixture_id}: expires token days must be > 0")
            else:
                expires_dt = parse_utc_iso(expires_at)
                if expires_dt <= now:
                    raise RuntimeError(f"{fixture_id}: fixed expires_at is already expired")

    sorted_fixture_ids = sorted(fixture_ids_in_order)
    if fixture_ids_in_order != sorted_fixture_ids:
        raise RuntimeError("fixture manifest fixtures must be sorted by id")

    fixture_dir = args.manifest.parent
    actual_fixture_files = {
        str(path).replace("\\", "/")
        for path in sorted(fixture_dir.glob("*.json"))
        if path.name != args.manifest.name
    }
    if actual_fixture_files != seen_files:
        missing_from_manifest = sorted(actual_fixture_files - seen_files)
        unknown_in_manifest = sorted(seen_files - actual_fixture_files)
        details: list[str] = []
        if missing_from_manifest:
            details.append(
                "unreferenced fixture files: " + ", ".join(missing_from_manifest)
            )
        if unknown_in_manifest:
            details.append(
                "manifest references missing fixture files: " + ", ".join(unknown_in_manifest)
            )
        raise RuntimeError("; ".join(details))

    print(f"[OK] parity waiver fixture manifest valid ({len(fixtures)} fixtures)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
