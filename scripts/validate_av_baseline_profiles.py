#!/usr/bin/env python3
"""Validate frozen A/V baseline profiles against the platform contract."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_CAPTURE_APIS = {
    "browser_getusermedia",
    "avfoundation",
    "media_foundation",
    "android_audiorecord",
    "linux_pipewire_pulseaudio",
}
ALLOWED_PLAYBACK_APIS = {
    "browser_webaudio",
    "coreaudio",
    "wasapi",
    "aaudio_opensles",
    "pulseaudio_pipewire",
}
ALLOWED_PERMISSION_FLOWS = {"runtime_prompt", "os_settings_plus_runtime_prompt"}


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
        "--av-profiles",
        type=pathlib.Path,
        default=pathlib.Path("docs/av-baseline-profiles.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    av_profiles = load_json(args.av_profiles)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if av_profiles.get("schema") != "kaigi-av-baseline-profiles/v1":
        raise RuntimeError("unexpected av baseline profile schema")
    if av_profiles.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and av baseline profiles")

    contracts = platform_contract.get("contracts")
    profiles = av_profiles.get("profiles")
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
        raise RuntimeError("duplicate platform entries in av baseline profiles")

    app_surface_by_platform: dict[str, str] = {}
    for contract in contracts:
        platform = contract.get("platform")
        app_surface = contract.get("app_surface")
        if not isinstance(platform, str) or not isinstance(app_surface, str):
            raise RuntimeError("invalid platform contract entry")
        app_surface_by_platform[platform] = app_surface

    for profile in profiles:
        platform = profile.get("platform")
        capture_api = profile.get("capture_api")
        playback_api = profile.get("playback_api")
        permission_flow = profile.get("permission_flow")
        notes = profile.get("notes")

        if not isinstance(platform, str):
            raise RuntimeError("profile platform must be a string")
        if capture_api not in ALLOWED_CAPTURE_APIS:
            raise RuntimeError(f"{platform}: unsupported capture_api={capture_api}")
        if playback_api not in ALLOWED_PLAYBACK_APIS:
            raise RuntimeError(f"{platform}: unsupported playback_api={playback_api}")
        if permission_flow not in ALLOWED_PERMISSION_FLOWS:
            raise RuntimeError(f"{platform}: unsupported permission_flow={permission_flow}")
        if not isinstance(notes, str) or not notes.strip():
            raise RuntimeError(f"{platform}: notes must be non-empty string")

        if profile.get("audio_input_required") is not True:
            raise RuntimeError(f"{platform}: audio_input_required must be true")
        if profile.get("audio_output_required") is not True:
            raise RuntimeError(f"{platform}: audio_output_required must be true")
        if profile.get("microphone_permission_required") is not True:
            raise RuntimeError(f"{platform}: microphone_permission_required must be true")
        if profile.get("echo_cancellation_default") is not True:
            raise RuntimeError(f"{platform}: echo_cancellation_default must be true")
        if profile.get("noise_suppression_default") is not True:
            raise RuntimeError(f"{platform}: noise_suppression_default must be true")
        if profile.get("automatic_gain_control_default") is not True:
            raise RuntimeError(f"{platform}: automatic_gain_control_default must be true")
        if profile.get("default_join_mic_enabled") is not False:
            raise RuntimeError(f"{platform}: default_join_mic_enabled must be false")
        if profile.get("default_join_camera_enabled") is not False:
            raise RuntimeError(f"{platform}: default_join_camera_enabled must be false")
        if profile.get("default_join_screen_share_enabled") is not False:
            raise RuntimeError(f"{platform}: default_join_screen_share_enabled must be false")
        if profile.get("speaker_route_control_supported") is not True:
            raise RuntimeError(f"{platform}: speaker_route_control_supported must be true")

        if app_surface_by_platform.get(platform) == "Web" and capture_api != "browser_getusermedia":
            raise RuntimeError(f"{platform}: web platforms must use browser_getusermedia capture_api")
        if app_surface_by_platform.get(platform) == "Native" and capture_api == "browser_getusermedia":
            raise RuntimeError(f"{platform}: native platforms must not use browser_getusermedia")

    print(f"[OK] av baseline profiles match platform contract ({len(profile_platforms)} platforms)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
