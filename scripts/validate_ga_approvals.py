#!/usr/bin/env python3
"""Validate single-train GA approvals cover every mandatory platform."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

EXPECTED_SCHEMA = "kaigi-ga-approvals/v1"
EXPECTED_FROZEN_AT = "2026-02-15"


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--platform-contract",
        type=pathlib.Path,
        default=pathlib.Path("docs/platform-contract.json"),
    )
    parser.add_argument(
        "--ga-approvals",
        type=pathlib.Path,
        default=pathlib.Path("docs/ga-approvals.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    approvals = load_json(args.ga_approvals)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if approvals.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected ga approvals schema")
    if approvals.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in ga approvals")
    if approvals.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and ga approvals")

    release_train = approvals.get("release_train")
    overall_status = approvals.get("overall_status")
    generated_at = approvals.get("generated_at")
    platform_approvals = approvals.get("platform_approvals")

    if not isinstance(release_train, str) or not release_train.strip():
        raise RuntimeError("release_train must be a non-empty string")
    if overall_status != "approved":
        raise RuntimeError("overall_status must be approved")
    if not isinstance(generated_at, str) or not generated_at.strip():
        raise RuntimeError("generated_at must be a non-empty string")
    if not isinstance(platform_approvals, list):
        raise RuntimeError("platform_approvals must be an array")

    contracts = platform_contract.get("contracts")
    if not isinstance(contracts, list):
        raise RuntimeError("platform contract contracts must be an array")
    contract_platforms = {entry.get("platform") for entry in contracts}

    approval_map: dict[str, dict] = {}
    for entry in platform_approvals:
        if not isinstance(entry, dict):
            raise RuntimeError("platform approval entries must be objects")
        platform = entry.get("platform")
        entry_train = entry.get("release_train")
        approval_status = entry.get("approval_status")
        approver = entry.get("approver")
        approved_at = entry.get("approved_at")

        if not isinstance(platform, str) or not platform:
            raise RuntimeError("platform approval entry missing platform")
        if platform in approval_map:
            raise RuntimeError(f"duplicate platform approval entry: {platform}")
        if entry_train != release_train:
            raise RuntimeError(f"{platform}: release_train mismatch")
        if approval_status != "approved":
            raise RuntimeError(f"{platform}: approval_status must be approved")
        if not isinstance(approver, str) or not approver.strip():
            raise RuntimeError(f"{platform}: approver must be non-empty")
        if not isinstance(approved_at, str) or not approved_at.strip():
            raise RuntimeError(f"{platform}: approved_at must be non-empty")

        approval_map[platform] = entry

    approved_platforms = set(approval_map.keys())
    if approved_platforms != contract_platforms:
        missing = sorted(contract_platforms - approved_platforms)
        extra = sorted(approved_platforms - contract_platforms)
        raise RuntimeError(
            "platform approvals mismatch: missing="
            + ",".join(missing)
            + " extra="
            + ",".join(extra)
        )

    print(
        "[OK] ga approvals cover all platforms "
        f"({len(approved_platforms)}) in release train {release_train}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
