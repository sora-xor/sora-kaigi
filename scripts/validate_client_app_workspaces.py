#!/usr/bin/env python3
"""Validate frozen client app workspace contract and on-disk workspace paths."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

EXPECTED_SCHEMA = "kaigi-client-app-workspaces/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
WORKSPACE_ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def load_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


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


def require_non_empty_str(entry: dict, field: str, workspace_id: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"workspace {workspace_id}: {field} must be non-empty string")
    return value.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--contract",
        type=pathlib.Path,
        default=pathlib.Path("docs/client-app-workspaces.json"),
    )
    parser.add_argument(
        "--parity-matrix",
        type=pathlib.Path,
        default=pathlib.Path("docs/parity-matrix.md"),
    )
    args = parser.parse_args()

    contract = load_json(args.contract)
    if contract.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected client app workspace contract schema")
    if contract.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in client app workspace contract")

    required_platforms = parse_mandatory_platforms(args.parity_matrix)
    if not required_platforms:
        raise RuntimeError("failed to parse mandatory platforms from parity matrix")
    required_platform_set = set(required_platforms)
    required_web_platforms = {platform for platform in required_platforms if platform.startswith("Web ")}

    workspaces = contract.get("workspaces")
    if not isinstance(workspaces, list) or not workspaces:
        raise RuntimeError("workspaces must be a non-empty array")

    workspace_ids: set[str] = set()
    workspace_impl: dict[str, str] = {}
    web_workspace_ids: list[str] = []
    platform_owner: dict[str, str] = {}
    native_fallbacks: list[tuple[str, str]] = []

    for workspace in workspaces:
        if not isinstance(workspace, dict):
            raise RuntimeError("workspace entries must be objects")

        workspace_id = workspace.get("id")
        if not isinstance(workspace_id, str) or not workspace_id:
            raise RuntimeError("workspace id must be non-empty string")
        if not WORKSPACE_ID_RE.fullmatch(workspace_id):
            raise RuntimeError(
                f"workspace id must match {WORKSPACE_ID_RE.pattern}: {workspace_id}"
            )
        if workspace_id in workspace_ids:
            raise RuntimeError(f"duplicate workspace id: {workspace_id}")
        workspace_ids.add(workspace_id)

        implementation = workspace.get("implementation")
        if implementation not in {"web", "native"}:
            raise RuntimeError(
                f"workspace {workspace_id}: implementation must be web|native"
            )
        workspace_impl[workspace_id] = implementation

        for field in ("build_system", "artifact", "hdr_strategy"):
            require_non_empty_str(workspace, field, workspace_id)

        workspace_path_str = require_non_empty_str(workspace, "path", workspace_id)
        workspace_path = pathlib.Path(workspace_path_str)
        if not workspace_path.is_dir():
            raise RuntimeError(
                f"workspace {workspace_id}: path directory does not exist: {workspace_path}"
            )
        readme_path = workspace_path / "README.md"
        if not readme_path.is_file():
            raise RuntimeError(
                f"workspace {workspace_id}: missing workspace README: {readme_path}"
            )

        platforms = workspace.get("platforms")
        if not isinstance(platforms, list) or not platforms:
            raise RuntimeError(f"workspace {workspace_id}: platforms must be non-empty array")

        for platform in platforms:
            if not isinstance(platform, str) or not platform:
                raise RuntimeError(
                    f"workspace {workspace_id}: platform entries must be non-empty strings"
                )
            if platform not in required_platform_set:
                raise RuntimeError(
                    f"workspace {workspace_id}: unknown platform in contract: {platform}"
                )
            if platform in platform_owner:
                raise RuntimeError(
                    "platform mapped by multiple workspaces: "
                    f"{platform} ({platform_owner[platform]}, {workspace_id})"
                )
            platform_owner[platform] = workspace_id

        fallback_workspace = workspace.get("web_fallback_workspace")
        if implementation == "web":
            if fallback_workspace is not None:
                raise RuntimeError(
                    f"workspace {workspace_id}: web workspace must not set web_fallback_workspace"
                )
            web_workspace_ids.append(workspace_id)
            if not set(platforms).issubset(required_web_platforms):
                raise RuntimeError(
                    f"workspace {workspace_id}: web workspace can only cover web platforms"
                )
        else:
            if not isinstance(fallback_workspace, str) or not fallback_workspace.strip():
                raise RuntimeError(
                    f"workspace {workspace_id}: native workspace must set web_fallback_workspace"
                )
            if any(platform.startswith("Web ") for platform in platforms):
                raise RuntimeError(
                    f"workspace {workspace_id}: native workspace cannot claim web platforms"
                )
            native_fallbacks.append((workspace_id, fallback_workspace.strip()))

    if len(web_workspace_ids) != 1:
        raise RuntimeError(
            f"expected exactly one web workspace, found {len(web_workspace_ids)}"
        )
    web_workspace_id = web_workspace_ids[0]

    for workspace_id, fallback_workspace in native_fallbacks:
        if fallback_workspace != web_workspace_id:
            raise RuntimeError(
                f"workspace {workspace_id}: web_fallback_workspace must be {web_workspace_id}"
            )
        if fallback_workspace not in workspace_impl:
            raise RuntimeError(
                f"workspace {workspace_id}: unknown web_fallback_workspace {fallback_workspace}"
            )
        if workspace_impl[fallback_workspace] != "web":
            raise RuntimeError(
                f"workspace {workspace_id}: web_fallback_workspace must reference web workspace"
            )

    missing_platforms = sorted(required_platform_set - set(platform_owner))
    if missing_platforms:
        raise RuntimeError(
            "client app workspace contract missing platforms: " + ", ".join(missing_platforms)
        )

    covered_web_platforms = {
        platform for platform, owner in platform_owner.items() if owner == web_workspace_id
    }
    if covered_web_platforms != required_web_platforms:
        raise RuntimeError(
            "web workspace coverage mismatch; expected "
            + ", ".join(sorted(required_web_platforms))
            + " got "
            + ", ".join(sorted(covered_web_platforms))
        )

    print(
        "[OK] client app workspace contract valid "
        f"({len(workspaces)} workspaces, {len(platform_owner)} platforms, "
        f"web workspace={web_workspace_id})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
