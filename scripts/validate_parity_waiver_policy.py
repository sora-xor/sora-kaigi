#!/usr/bin/env python3
"""Validate parity waiver entries satisfy policy constraints."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import sys

EXPECTED_WAIVER_SCHEMA = "kaigi-parity-status-waivers/v1"
EXPECTED_POLICY_SCHEMA = "kaigi-parity-waiver-policy/v1"
EXPECTED_FROZEN_AT = "2026-02-15"


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_utc_iso(value: str) -> dt.datetime:
    if not value.endswith("Z"):
        raise RuntimeError(f"invalid UTC timestamp (expected trailing Z): {value}")
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def require_int(payload: dict, field: str, *, minimum: int = 0) -> int:
    value = payload.get(field)
    if not isinstance(value, int) or value < minimum:
        raise RuntimeError(f"{field} must be an integer >= {minimum}")
    return value


def require_bool(payload: dict, field: str) -> bool:
    value = payload.get(field)
    if not isinstance(value, bool):
        raise RuntimeError(f"{field} must be a boolean")
    return value


def require_list(payload: dict, field: str) -> list:
    value = payload.get(field)
    if not isinstance(value, list):
        raise RuntimeError(f"{field} must be an array")
    return value


def require_regex(payload: dict, field: str) -> re.Pattern[str]:
    value = payload.get(field)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"{field} must be a non-empty regex string")
    try:
        return re.compile(value)
    except re.error as exc:  # noqa: BLE001
        raise RuntimeError(f"{field} regex is invalid: {exc}") from exc


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--waivers-file",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-status-waivers.json"),
    )
    parser.add_argument(
        "--waiver-policy",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-waiver-policy.json"),
    )
    args = parser.parse_args()

    waivers_doc = load_json(args.waivers_file)
    policy_doc = load_json(args.waiver_policy)

    if waivers_doc.get("schema") != EXPECTED_WAIVER_SCHEMA:
        raise RuntimeError("unexpected parity status waivers schema")
    if policy_doc.get("schema") != EXPECTED_POLICY_SCHEMA:
        raise RuntimeError("unexpected parity waiver policy schema")

    waivers_frozen_at = waivers_doc.get("frozen_at")
    policy_frozen_at = policy_doc.get("frozen_at")
    if waivers_frozen_at != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity status waivers")
    if policy_frozen_at != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in parity waiver policy")
    if waivers_frozen_at != policy_frozen_at:
        raise RuntimeError("frozen_at mismatch between parity waivers and waiver policy")

    min_reason_chars = require_int(policy_doc, "min_reason_chars", minimum=1)
    max_reason_chars = require_int(policy_doc, "max_reason_chars", minimum=min_reason_chars)
    max_ttl_days = require_int(policy_doc, "max_waiver_ttl_days", minimum=1)
    require_distinct = require_bool(policy_doc, "require_distinct_owner_and_approver")

    allowed_target_statuses_raw = require_list(policy_doc, "allowed_target_statuses")
    allowed_target_statuses: set[str] = set()
    for entry in allowed_target_statuses_raw:
        if not isinstance(entry, str) or not entry:
            raise RuntimeError("allowed_target_statuses entries must be non-empty strings")
        allowed_target_statuses.add(entry)
    if not allowed_target_statuses:
        raise RuntimeError("allowed_target_statuses cannot be empty")

    owner_re = require_regex(policy_doc, "owner_pattern")
    approver_re = require_regex(policy_doc, "approved_by_pattern")
    ticket_re = require_regex(policy_doc, "ticket_pattern")

    waivers = waivers_doc.get("waivers")
    if not isinstance(waivers, list):
        raise RuntimeError("waivers must be an array")

    now = dt.datetime.now(dt.timezone.utc)
    max_ttl = dt.timedelta(days=max_ttl_days)

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
        if not isinstance(platform, str) or not platform:
            raise RuntimeError("waiver platform must be non-empty string")

        if not isinstance(target_status, str) or target_status not in allowed_target_statuses:
            raise RuntimeError(
                f"waiver target_status must be one of {sorted(allowed_target_statuses)}"
            )

        if not isinstance(reason, str):
            raise RuntimeError("waiver reason must be a string")
        reason_trimmed = reason.strip()
        if len(reason_trimmed) < min_reason_chars:
            raise RuntimeError(
                f"waiver reason too short for {area}/{name}/{platform}: "
                f"{len(reason_trimmed)} < {min_reason_chars}"
            )
        if len(reason_trimmed) > max_reason_chars:
            raise RuntimeError(
                f"waiver reason too long for {area}/{name}/{platform}: "
                f"{len(reason_trimmed)} > {max_reason_chars}"
            )

        if not isinstance(owner, str):
            raise RuntimeError("waiver owner must be a string")
        if not isinstance(approved_by, str):
            raise RuntimeError("waiver approved_by must be a string")
        if require_distinct and owner == approved_by:
            raise RuntimeError(
                f"waiver owner and approver must differ for {area}/{name}/{platform}"
            )

        if not owner_re.fullmatch(owner):
            raise RuntimeError(
                f"waiver owner format invalid for {area}/{name}/{platform}: {owner}"
            )
        if not approver_re.fullmatch(approved_by):
            raise RuntimeError(
                f"waiver approved_by format invalid for {area}/{name}/{platform}: {approved_by}"
            )

        if not isinstance(ticket, str) or not ticket_re.fullmatch(ticket):
            raise RuntimeError(
                f"waiver ticket format invalid for {area}/{name}/{platform}: {ticket}"
            )

        if not isinstance(expires_at, str) or not expires_at.strip():
            raise RuntimeError("waiver expires_at must be non-empty string")
        expires_dt = parse_utc_iso(expires_at)
        if expires_dt <= now:
            raise RuntimeError(f"waiver has expired for {area}/{name}/{platform}")
        if expires_dt - now > max_ttl:
            raise RuntimeError(
                f"waiver ttl exceeds policy max ({max_ttl_days}d) for {area}/{name}/{platform}"
            )

    print(
        "[OK] parity waiver policy validation passed "
        f"({len(waivers)} waivers, max_ttl_days={max_ttl_days}, min_reason_chars={min_reason_chars})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
