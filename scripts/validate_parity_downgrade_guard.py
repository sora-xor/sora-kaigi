#!/usr/bin/env python3
"""Validate parity GA downgrade guard with explicit waiver controls."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import sys

ALLOWED_STATUSES = {"P", "A", "B", "GA"}
EXPECTED_PARITY_SCHEMA = "kaigi-parity-status-contract/v1"
EXPECTED_WAIVER_SCHEMA = "kaigi-parity-status-waivers/v1"
EXPECTED_FROZEN_AT = "2026-02-15"


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_utc_iso(value: str) -> dt.datetime:
    if not value.endswith("Z"):
        raise RuntimeError(f"invalid UTC timestamp (expected trailing Z): {value}")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--parity-status-contract",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-status-contract.json"),
    )
    parser.add_argument(
        "--waivers-file",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-status-waivers.json"),
    )
    args = parser.parse_args()

    parity = load_json(args.parity_status_contract)
    waivers_doc = load_json(args.waivers_file)

    if parity.get("schema") != EXPECTED_PARITY_SCHEMA:
        raise RuntimeError("unexpected parity status contract schema")
    if waivers_doc.get("schema") != EXPECTED_WAIVER_SCHEMA:
        raise RuntimeError("unexpected parity status waivers schema")
    if parity.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity status contract")
    if waivers_doc.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity status waivers")
    if waivers_doc.get("frozen_at") != parity.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between parity contract and waivers")

    generated_at = waivers_doc.get("generated_at")
    if not isinstance(generated_at, str) or not generated_at.strip():
        raise RuntimeError("waivers generated_at must be a non-empty string")
    parse_utc_iso(generated_at)

    platforms = parity.get("platforms")
    capabilities = parity.get("capabilities")
    waivers = waivers_doc.get("waivers")

    if not isinstance(platforms, list) or not platforms:
        raise RuntimeError("parity platforms must be non-empty array")
    if not isinstance(capabilities, list) or not capabilities:
        raise RuntimeError("parity capabilities must be non-empty array")
    if not isinstance(waivers, list):
        raise RuntimeError("waivers must be an array")

    now = dt.datetime.now(dt.timezone.utc)

    capability_status_map: dict[tuple[str, str, str], str] = {}
    capability_keys: set[tuple[str, str]] = set()
    downgraded_keys: set[tuple[str, str, str, str]] = set()

    for capability in capabilities:
        if not isinstance(capability, dict):
            raise RuntimeError("capability entries must be objects")
        area = capability.get("area")
        name = capability.get("name")
        statuses = capability.get("statuses")
        if not isinstance(area, str) or not isinstance(name, str) or not isinstance(statuses, dict):
            raise RuntimeError("invalid capability entry in parity contract")
        capability_keys.add((area, name))
        for platform in platforms:
            status = statuses.get(platform)
            if status not in ALLOWED_STATUSES:
                raise RuntimeError(f"invalid status for {area}/{name}/{platform}: {status}")
            capability_status_map[(area, name, platform)] = status
            if status != "GA":
                downgraded_keys.add((area, name, platform, status))

    waiver_keys: set[tuple[str, str, str, str]] = set()
    for waiver in waivers:
        if not isinstance(waiver, dict):
            raise RuntimeError("waiver entries must be objects")

        area = waiver.get("area")
        name = waiver.get("name")
        platform = waiver.get("platform")
        target_status = waiver.get("target_status")
        reason = waiver.get("reason")
        owner = waiver.get("owner")
        approved_by = waiver.get("approved_by")
        ticket = waiver.get("ticket")
        expires_at = waiver.get("expires_at")

        if not isinstance(area, str) or not area:
            raise RuntimeError("waiver area must be non-empty string")
        if not isinstance(name, str) or not name:
            raise RuntimeError("waiver name must be non-empty string")
        if not isinstance(platform, str) or platform not in platforms:
            raise RuntimeError(f"waiver platform is invalid: {platform}")
        if (area, name) not in capability_keys:
            raise RuntimeError(f"waiver capability does not exist in parity contract: {area}/{name}")
        if target_status not in ALLOWED_STATUSES - {"GA"}:
            raise RuntimeError(f"waiver target_status must be one of P/A/B: {target_status}")
        if not isinstance(reason, str) or not reason.strip():
            raise RuntimeError("waiver reason must be non-empty string")
        if not isinstance(owner, str) or not owner.strip():
            raise RuntimeError("waiver owner must be non-empty string")
        if not isinstance(approved_by, str) or not approved_by.strip():
            raise RuntimeError("waiver approved_by must be non-empty string")
        if not isinstance(ticket, str) or not ticket.strip():
            raise RuntimeError("waiver ticket must be non-empty string")
        if not isinstance(expires_at, str) or not expires_at.strip():
            raise RuntimeError("waiver expires_at must be non-empty string")

        expires_dt = parse_utc_iso(expires_at)
        if expires_dt <= now:
            raise RuntimeError(f"waiver has expired: {area}/{name}/{platform} {target_status}")

        current_status = capability_status_map.get((area, name, platform))
        if current_status != target_status:
            raise RuntimeError(
                f"waiver target_status mismatch for {area}/{name}/{platform}: "
                f"expected {current_status}, got {target_status}"
            )

        key = (area, name, platform, target_status)
        if key in waiver_keys:
            raise RuntimeError(
                f"duplicate waiver entry: {area}/{name}/{platform}/{target_status}"
            )
        waiver_keys.add(key)

    unwaived = sorted(downgraded_keys - waiver_keys)
    orphaned = sorted(waiver_keys - downgraded_keys)
    if unwaived:
        sample = ", ".join(f"{a}/{n}/{p}:{s}" for a, n, p, s in unwaived[:10])
        raise RuntimeError(f"unwaived parity downgrades detected: {sample}")
    if orphaned:
        sample = ", ".join(f"{a}/{n}/{p}:{s}" for a, n, p, s in orphaned[:10])
        raise RuntimeError(f"orphaned waivers detected: {sample}")

    print(
        "[OK] parity downgrade guard passed "
        f"({len(capabilities)} capabilities, {len(platforms)} platforms, {len(waiver_keys)} active waivers)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
