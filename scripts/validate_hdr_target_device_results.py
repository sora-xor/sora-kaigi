#!/usr/bin/env python3
"""Validate HDR target-device results coverage across all mandatory platforms."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_CASE_TYPES = {"hdr_path", "sdr_fallback_path"}
ALLOWED_STATUS = {"passed", "failed"}


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
        "--media-profiles",
        type=pathlib.Path,
        default=pathlib.Path("docs/media-capability-profiles.json"),
    )
    parser.add_argument(
        "--target-results",
        type=pathlib.Path,
        default=pathlib.Path("docs/hdr-target-device-results.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    media_profiles = load_json(args.media_profiles)
    target_results = load_json(args.target_results)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if media_profiles.get("schema") != "kaigi-media-capability-profiles/v1":
        raise RuntimeError("unexpected media profile schema")
    if target_results.get("schema") != "kaigi-hdr-target-device-results/v1":
        raise RuntimeError("unexpected hdr target-device results schema")

    frozen_at = platform_contract.get("frozen_at")
    if media_profiles.get("frozen_at") != frozen_at:
        raise RuntimeError("frozen_at mismatch between platform contract and media profiles")
    if target_results.get("frozen_at") != frozen_at:
        raise RuntimeError("frozen_at mismatch between platform contract and hdr target-device results")

    contracts = platform_contract.get("contracts")
    media_entries = media_profiles.get("profiles")
    results = target_results.get("results")
    if not isinstance(contracts, list) or not isinstance(media_entries, list) or not isinstance(results, list):
        raise RuntimeError("contracts/profiles/results must be arrays")

    contract_platforms = {entry.get("platform") for entry in contracts}
    media_platforms = {entry.get("platform") for entry in media_entries}
    if contract_platforms != media_platforms:
        raise RuntimeError("platform set mismatch between platform contract and media profiles")

    generated_at = target_results.get("generated_at")
    if not isinstance(generated_at, str) or not generated_at.strip():
        raise RuntimeError("hdr target-device results must include generated_at")

    seen_case_ids: set[str] = set()
    case_status_by_platform: dict[str, dict[str, list[str]]] = {
        platform: {case_type: [] for case_type in ALLOWED_CASE_TYPES}
        for platform in contract_platforms
    }

    for row in results:
        if not isinstance(row, dict):
            raise RuntimeError("target-device results entries must be objects")
        platform = row.get("platform")
        case_id = row.get("case_id")
        case_type = row.get("case_type")
        status = row.get("status")
        test_environment = row.get("test_environment")
        executed_at = row.get("executed_at")
        notes = row.get("notes")

        if not isinstance(platform, str) or platform not in contract_platforms:
            raise RuntimeError(f"invalid platform in target-device result: {platform}")
        if not isinstance(case_id, str) or not case_id.strip():
            raise RuntimeError(f"{platform}: case_id must be non-empty string")
        if case_id in seen_case_ids:
            raise RuntimeError(f"duplicate case_id: {case_id}")
        seen_case_ids.add(case_id)

        if case_type not in ALLOWED_CASE_TYPES:
            raise RuntimeError(f"{platform}: invalid case_type={case_type}")
        if status not in ALLOWED_STATUS:
            raise RuntimeError(f"{platform}: invalid status={status}")
        if not isinstance(test_environment, str) or not test_environment.strip():
            raise RuntimeError(f"{platform}: test_environment must be non-empty string")
        if not isinstance(executed_at, str) or not executed_at.strip():
            raise RuntimeError(f"{platform}: executed_at must be non-empty string")
        if not isinstance(notes, str) or not notes.strip():
            raise RuntimeError(f"{platform}: notes must be non-empty string")

        case_status_by_platform[platform][case_type].append(status)

    for platform in sorted(contract_platforms):
        for case_type in sorted(ALLOWED_CASE_TYPES):
            statuses = case_status_by_platform[platform][case_type]
            if not statuses:
                raise RuntimeError(f"{platform}: missing target-device evidence for {case_type}")
            if not any(status == "passed" for status in statuses):
                raise RuntimeError(f"{platform}: no passing target-device evidence for {case_type}")

    print(f"[OK] hdr target-device results cover all platforms ({len(contract_platforms)} platforms)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
