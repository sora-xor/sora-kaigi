#!/usr/bin/env python3
"""Validate platform blocker ledger is empty for mandatory platform set."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

EXPECTED_SCHEMA = "kaigi-platform-blockers/v1"
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
        "--blockers-file",
        type=pathlib.Path,
        default=pathlib.Path("docs/platform-blockers.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    blockers = load_json(args.blockers_file)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if blockers.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected platform blockers schema")
    if blockers.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in platform blockers ledger")
    if blockers.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and platform blockers")

    contracts = platform_contract.get("contracts")
    tracked_platforms = blockers.get("tracked_platforms")
    open_blocks = blockers.get("open_blocks")
    updated_at = blockers.get("updated_at")

    if not isinstance(contracts, list):
        raise RuntimeError("platform contract contracts must be an array")
    if not isinstance(tracked_platforms, list):
        raise RuntimeError("tracked_platforms must be an array")
    if not isinstance(open_blocks, list):
        raise RuntimeError("open_blocks must be an array")
    if not isinstance(updated_at, str) or not updated_at.strip():
        raise RuntimeError("updated_at must be a non-empty string")

    contract_platforms = {entry.get("platform") for entry in contracts}
    tracked_set = set(tracked_platforms)
    if contract_platforms != tracked_set:
        missing = sorted(contract_platforms - tracked_set)
        extra = sorted(tracked_set - contract_platforms)
        raise RuntimeError(
            "tracked_platforms mismatch: missing=" + ",".join(missing) + " extra=" + ",".join(extra)
        )
    if len(tracked_set) != len(tracked_platforms):
        raise RuntimeError("tracked_platforms contains duplicates")

    if open_blocks:
        block_ids: list[str] = []
        for block in open_blocks:
            if isinstance(block, dict):
                block_id = block.get("id")
                if isinstance(block_id, str) and block_id.strip():
                    block_ids.append(block_id)
        details = ", ".join(block_ids) if block_ids else f"{len(open_blocks)} entries"
        raise RuntimeError(f"platform blockers remain open: {details}")

    print(f"[OK] platform blocker ledger reports zero open blocks ({len(tracked_set)} platforms)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
