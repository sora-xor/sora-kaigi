#!/usr/bin/env python3
"""Validate frozen screen-share constraints against the platform contract."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ALLOWED_CAPTURE_TARGETS = {"display", "window", "browser_tab"}
ALLOWED_AUDIO_MODES = {"none", "tab", "system_loopback"}
MOBILE_PLATFORMS = {"IOS", "IPadOS", "VisionOS"}


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
        "--constraints",
        type=pathlib.Path,
        default=pathlib.Path("docs/screen-share-constraints.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    constraints_doc = load_json(args.constraints)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if constraints_doc.get("schema") != "kaigi-screen-share-constraints/v1":
        raise RuntimeError("unexpected screen-share constraints schema")
    if constraints_doc.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and screen-share constraints")

    contracts = platform_contract.get("contracts")
    constraints = constraints_doc.get("constraints")
    if not isinstance(contracts, list) or not isinstance(constraints, list):
        raise RuntimeError("contracts/constraints must be arrays")

    contract_platforms = {entry.get("platform") for entry in contracts}
    constraint_platforms = {entry.get("platform") for entry in constraints}
    if contract_platforms != constraint_platforms:
        missing = sorted(contract_platforms - constraint_platforms)
        extra = sorted(constraint_platforms - contract_platforms)
        raise RuntimeError(
            "platform set mismatch: missing="
            + ",".join(missing)
            + " extra="
            + ",".join(extra)
        )
    if len(constraint_platforms) != len(constraints):
        raise RuntimeError("duplicate platform entries in screen-share constraints")

    app_surface_by_platform: dict[str, str] = {}
    for contract in contracts:
        platform = contract.get("platform")
        app_surface = contract.get("app_surface")
        if not isinstance(platform, str) or not isinstance(app_surface, str):
            raise RuntimeError("invalid platform contract entry")
        app_surface_by_platform[platform] = app_surface

    for constraint in constraints:
        platform = constraint.get("platform")
        capture_targets = constraint.get("capture_targets")
        system_audio_modes = constraint.get("system_audio_modes")

        if not isinstance(platform, str):
            raise RuntimeError("constraint platform must be a string")
        if not isinstance(capture_targets, list) or not capture_targets:
            raise RuntimeError(f"{platform}: capture_targets must be a non-empty list")
        if not isinstance(system_audio_modes, list) or not system_audio_modes:
            raise RuntimeError(f"{platform}: system_audio_modes must be a non-empty list")

        if any(
            not isinstance(target, str) or target not in ALLOWED_CAPTURE_TARGETS
            for target in capture_targets
        ):
            raise RuntimeError(f"{platform}: capture_targets contains unsupported values")
        if any(
            not isinstance(mode, str) or mode not in ALLOWED_AUDIO_MODES
            for mode in system_audio_modes
        ):
            raise RuntimeError(f"{platform}: system_audio_modes contains unsupported values")

        capture_target_set = set(capture_targets)
        audio_mode_set = set(system_audio_modes)

        if constraint.get("max_concurrent_local_shares") != 1:
            raise RuntimeError(f"{platform}: max_concurrent_local_shares must be 1")
        if constraint.get("requires_explicit_user_consent") is not True:
            raise RuntimeError(f"{platform}: requires_explicit_user_consent must be true")
        notes = constraint.get("notes")
        if not isinstance(notes, str) or not notes.strip():
            raise RuntimeError(f"{platform}: notes must be a non-empty string")

        if "browser_tab" in capture_target_set and app_surface_by_platform.get(platform) != "Web":
            raise RuntimeError(f"{platform}: browser_tab capture is only valid for web platforms")
        if "tab" in audio_mode_set and "browser_tab" not in capture_target_set:
            raise RuntimeError(
                f"{platform}: tab audio mode requires browser_tab capture target"
            )
        if platform in MOBILE_PLATFORMS:
            if capture_target_set != {"display"}:
                raise RuntimeError(
                    f"{platform}: mobile capture_targets must be exactly ['display']"
                )
            if audio_mode_set != {"none"}:
                raise RuntimeError(f"{platform}: mobile system_audio_modes must be ['none']")
        if "none" in audio_mode_set and len(audio_mode_set) > 1:
            raise RuntimeError(f"{platform}: system_audio_modes cannot mix 'none' with other modes")

    print(f"[OK] screen-share constraints match platform contract ({len(constraint_platforms)} platforms)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
