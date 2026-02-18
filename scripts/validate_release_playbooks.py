#!/usr/bin/env python3
"""Validate release/rollback playbooks for native + IPFS web coverage."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

NATIVE_PLATFORMS = ["macOS", "iOS", "iPadOS", "Windows", "Android", "Linux"]
WEB_PLATFORMS = ["Web Chromium", "Web Safari", "Web Firefox"]


def parse_mandatory_platforms(parity_matrix_path: pathlib.Path) -> list[str]:
    lines = parity_matrix_path.read_text(encoding="utf-8").splitlines()
    collecting = False
    out: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped == "## Mandatory Platforms":
            collecting = True
            continue
        if not collecting:
            continue
        if stripped.startswith("## "):
            break
        if stripped.startswith("- "):
            value = stripped[2:].strip()
            value = re.sub(r"\s*\([^)]*\)\s*$", "", value)
            out.append(value)
    return out


def ensure_markers(text: str, markers: list[str], *, name: str) -> None:
    for marker in markers:
        if marker not in text:
            raise RuntimeError(f"{name}: missing required section marker: {marker}")


def ensure_platform_mentions(text: str, platforms: list[str], *, name: str) -> None:
    for platform in platforms:
        if platform not in text:
            raise RuntimeError(f"{name}: missing platform coverage: {platform}")


def validate_release_playbook(text: str) -> None:
    ensure_markers(
        text,
        [
            "# Release Playbook",
            "## Scope",
            "## Preconditions",
            "## Artifact Build and Signing",
            "## Native Release Tracks",
            "## IPFS Web Release",
            "## Launch Checklist",
            "## Post-Release Verification",
        ],
        name="release-playbook",
    )
    ensure_platform_mentions(text, NATIVE_PLATFORMS, name="release-playbook")
    ensure_platform_mentions(text, WEB_PLATFORMS, name="release-playbook")
    if "IPFS" not in text:
        raise RuntimeError("release-playbook: missing IPFS release coverage")


def validate_rollback_playbook(text: str) -> None:
    ensure_markers(
        text,
        [
            "# Rollback Playbook",
            "## Trigger Conditions",
            "## Decision and Ownership",
            "## Native Rollback Tracks",
            "## IPFS Web Rollback",
            "## Incident Communication",
            "## Exit Criteria",
        ],
        name="rollback-playbook",
    )
    ensure_platform_mentions(text, NATIVE_PLATFORMS, name="rollback-playbook")
    ensure_platform_mentions(text, WEB_PLATFORMS, name="rollback-playbook")
    if "IPFS" not in text:
        raise RuntimeError("rollback-playbook: missing IPFS rollback coverage")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    parser.add_argument(
        "--release-playbook",
        type=pathlib.Path,
        default=pathlib.Path("docs/release-playbook.md"),
    )
    parser.add_argument(
        "--rollback-playbook",
        type=pathlib.Path,
        default=pathlib.Path("docs/rollback-playbook.md"),
    )
    parser.add_argument(
        "--mode",
        choices=["all", "release", "rollback"],
        default="all",
    )
    args = parser.parse_args()

    mandatory_platforms = parse_mandatory_platforms(args.parity_matrix)
    expected_platforms = WEB_PLATFORMS + NATIVE_PLATFORMS
    if set(mandatory_platforms) != set(expected_platforms):
        raise RuntimeError(
            "mandatory platforms mismatch in parity matrix; expected "
            + ", ".join(expected_platforms)
        )

    if args.mode in {"all", "release"}:
        release_text = args.release_playbook.read_text(encoding="utf-8")
        validate_release_playbook(release_text)

    if args.mode in {"all", "rollback"}:
        rollback_text = args.rollback_playbook.read_text(encoding="utf-8")
        validate_rollback_playbook(rollback_text)

    print(f"[OK] release playbook validation passed (mode={args.mode})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
