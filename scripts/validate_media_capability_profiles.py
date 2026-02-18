#!/usr/bin/env python3
"""Validate frozen media capability profiles against the platform contract."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_CAPTURE_PIPELINES = {
    "browser_getusermedia",
    "native_avfoundation",
    "native_media_foundation",
    "native_camera2",
    "native_v4l2_pipewire",
}
ALLOWED_HDR_MODES = {"supported_hardware_only", "unsupported"}
ALLOWED_RENDER_MODES = {"supported_display_only", "unsupported"}


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
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    media_profiles = load_json(args.media_profiles)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if media_profiles.get("schema") != "kaigi-media-capability-profiles/v1":
        raise RuntimeError("unexpected media profile schema")
    if media_profiles.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and media profiles")

    contracts = platform_contract.get("contracts")
    profiles = media_profiles.get("profiles")
    if not isinstance(contracts, list) or not isinstance(profiles, list):
        raise RuntimeError("contracts/profiles must be arrays")

    contract_platforms = {entry.get("platform") for entry in contracts}
    profile_platforms = {entry.get("platform") for entry in profiles}
    if contract_platforms != profile_platforms:
        missing = sorted(contract_platforms - profile_platforms)
        extra = sorted(profile_platforms - contract_platforms)
        raise RuntimeError(
            "platform set mismatch: missing="
            + ",".join(missing)
            + " extra="
            + ",".join(extra)
        )

    if len(profile_platforms) != len(profiles):
        raise RuntimeError("duplicate platform entries in media profiles")

    for profile in profiles:
        platform = profile.get("platform")
        capture_pipeline = profile.get("capture_pipeline")
        hdr_capture_mode = profile.get("hdr_capture_mode")
        hdr_render_mode = profile.get("hdr_render_mode")
        codecs = profile.get("preferred_video_codecs")

        if capture_pipeline not in ALLOWED_CAPTURE_PIPELINES:
            raise RuntimeError(f"{platform}: unsupported capture_pipeline={capture_pipeline}")
        if hdr_capture_mode not in ALLOWED_HDR_MODES:
            raise RuntimeError(f"{platform}: unsupported hdr_capture_mode={hdr_capture_mode}")
        if hdr_render_mode not in ALLOWED_RENDER_MODES:
            raise RuntimeError(f"{platform}: unsupported hdr_render_mode={hdr_render_mode}")
        if profile.get("sdr_fallback_required") is not True:
            raise RuntimeError(f"{platform}: sdr_fallback_required must be true")
        if profile.get("tone_mapping_required") is not True:
            raise RuntimeError(f"{platform}: tone_mapping_required must be true")
        if not isinstance(codecs, list) or not codecs:
            raise RuntimeError(f"{platform}: preferred_video_codecs must be non-empty list")
        if not all(isinstance(codec, str) and codec for codec in codecs):
            raise RuntimeError(f"{platform}: preferred_video_codecs contains invalid value")

    print(
        f"[OK] media capability profiles match platform contract ({len(profile_platforms)} platforms)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
