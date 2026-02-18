#!/usr/bin/env python3
"""Validate frozen HDR transport profiles against platform/media contracts."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_COLOR_PRIMARIES = {"bt2020", "display_p3"}
ALLOWED_TRANSFER_FUNCTIONS = {"pq", "hlg"}
ALLOWED_METADATA_FIELDS = {
    "mastering_display_primaries",
    "mastering_display_luminance",
    "max_cll",
    "max_fall",
}
ALLOWED_SDR_FALLBACK_PROFILES = {"bt709_8bit"}
ALLOWED_TONEMAP_STRATEGIES = {"hable"}
ALLOWED_NEGOTIATION_POLICIES = {
    "require_sender_capture_and_receiver_render_support",
}


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
        "--hdr-profiles",
        type=pathlib.Path,
        default=pathlib.Path("docs/hdr-transport-profiles.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    media_profiles = load_json(args.media_profiles)
    hdr_profiles = load_json(args.hdr_profiles)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if media_profiles.get("schema") != "kaigi-media-capability-profiles/v1":
        raise RuntimeError("unexpected media profile schema")
    if hdr_profiles.get("schema") != "kaigi-hdr-transport-profiles/v1":
        raise RuntimeError("unexpected hdr transport profile schema")

    frozen_at = platform_contract.get("frozen_at")
    if media_profiles.get("frozen_at") != frozen_at:
        raise RuntimeError("frozen_at mismatch between platform contract and media profiles")
    if hdr_profiles.get("frozen_at") != frozen_at:
        raise RuntimeError("frozen_at mismatch between platform contract and hdr transport profiles")

    contracts = platform_contract.get("contracts")
    media_entries = media_profiles.get("profiles")
    hdr_entries = hdr_profiles.get("profiles")
    if not isinstance(contracts, list) or not isinstance(media_entries, list) or not isinstance(hdr_entries, list):
        raise RuntimeError("contracts/profiles must be arrays")

    contract_platforms = {entry.get("platform") for entry in contracts}
    media_platforms = {entry.get("platform") for entry in media_entries}
    hdr_platforms = {entry.get("platform") for entry in hdr_entries}
    if contract_platforms != media_platforms or contract_platforms != hdr_platforms:
        raise RuntimeError("platform set mismatch between contracts, media profiles, and hdr profiles")
    if len(hdr_platforms) != len(hdr_entries):
        raise RuntimeError("duplicate platform entries in hdr transport profiles")

    media_by_platform: dict[str, dict] = {}
    for media in media_entries:
        platform = media.get("platform")
        if isinstance(platform, str):
            media_by_platform[platform] = media

    for profile in hdr_entries:
        platform = profile.get("platform")
        if not isinstance(platform, str):
            raise RuntimeError("hdr transport profile platform must be a string")

        if profile.get("hdr_enabled_on_supported_devices") is not True:
            raise RuntimeError(f"{platform}: hdr_enabled_on_supported_devices must be true")

        if profile.get("hdr_color_primaries") not in ALLOWED_COLOR_PRIMARIES:
            raise RuntimeError(f"{platform}: unsupported hdr_color_primaries")
        if profile.get("hdr_transfer_function") not in ALLOWED_TRANSFER_FUNCTIONS:
            raise RuntimeError(f"{platform}: unsupported hdr_transfer_function")

        bit_depth = profile.get("hdr_bit_depth")
        if not isinstance(bit_depth, int) or bit_depth < 10 or bit_depth > 12:
            raise RuntimeError(f"{platform}: hdr_bit_depth must be an integer in [10,12]")

        if profile.get("static_metadata_required") is not True:
            raise RuntimeError(f"{platform}: static_metadata_required must be true")
        metadata_fields = profile.get("static_metadata_fields")
        if not isinstance(metadata_fields, list) or not metadata_fields:
            raise RuntimeError(f"{platform}: static_metadata_fields must be non-empty list")
        if any(
            not isinstance(field, str) or field not in ALLOWED_METADATA_FIELDS
            for field in metadata_fields
        ):
            raise RuntimeError(f"{platform}: static_metadata_fields contains unsupported value")

        if profile.get("sdr_fallback_profile") not in ALLOWED_SDR_FALLBACK_PROFILES:
            raise RuntimeError(f"{platform}: unsupported sdr_fallback_profile")
        if profile.get("tone_mapping_strategy") not in ALLOWED_TONEMAP_STRATEGIES:
            raise RuntimeError(f"{platform}: unsupported tone_mapping_strategy")
        if profile.get("negotiation_policy") not in ALLOWED_NEGOTIATION_POLICIES:
            raise RuntimeError(f"{platform}: unsupported negotiation_policy")

        media_profile = media_by_platform.get(platform)
        if media_profile is None:
            raise RuntimeError(f"{platform}: missing media profile")
        if media_profile.get("tone_mapping_required") is not True:
            raise RuntimeError(f"{platform}: media profile must require tone mapping")
        if media_profile.get("sdr_fallback_required") is not True:
            raise RuntimeError(f"{platform}: media profile must require SDR fallback")
        if media_profile.get("hdr_capture_mode") != "supported_hardware_only":
            raise RuntimeError(f"{platform}: media profile hdr_capture_mode must be supported_hardware_only")
        if media_profile.get("hdr_render_mode") != "supported_display_only":
            raise RuntimeError(f"{platform}: media profile hdr_render_mode must be supported_display_only")

    print(f"[OK] hdr transport profiles match platform/media contracts ({len(hdr_platforms)} platforms)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
