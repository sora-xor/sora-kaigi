#!/usr/bin/env python3
"""Validate hardening gate results for performance/reliability/security across platforms."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

EXPECTED_SCHEMA = "kaigi-hardening-gates/v1"
EXPECTED_FROZEN_AT = "2026-02-15"
REQUIRED_GATE_FIELDS = ("performance", "reliability", "security")


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
        "--hardening-gates",
        type=pathlib.Path,
        default=pathlib.Path("docs/hardening-gates.json"),
    )
    args = parser.parse_args()

    platform_contract = load_json(args.platform_contract)
    hardening = load_json(args.hardening_gates)

    if platform_contract.get("schema") != "kaigi-platform-contract/v1":
        raise RuntimeError("unexpected platform contract schema")
    if hardening.get("schema") != EXPECTED_SCHEMA:
        raise RuntimeError("unexpected hardening gates schema")
    if hardening.get("frozen_at") != EXPECTED_FROZEN_AT:
        raise RuntimeError("unexpected frozen_at in hardening gates")
    if hardening.get("frozen_at") != platform_contract.get("frozen_at"):
        raise RuntimeError("frozen_at mismatch between platform contract and hardening gates")

    contracts = platform_contract.get("contracts")
    gates = hardening.get("platform_gates")
    release_train = hardening.get("release_train")
    generated_at = hardening.get("generated_at")

    if not isinstance(contracts, list):
        raise RuntimeError("platform contract contracts must be an array")
    if not isinstance(gates, list):
        raise RuntimeError("platform_gates must be an array")
    if not isinstance(release_train, str) or not release_train.strip():
        raise RuntimeError("release_train must be a non-empty string")
    if not isinstance(generated_at, str) or not generated_at.strip():
        raise RuntimeError("generated_at must be a non-empty string")

    contract_platforms = {entry.get("platform") for entry in contracts}
    gate_map: dict[str, dict] = {}
    for gate in gates:
        if not isinstance(gate, dict):
            raise RuntimeError("platform gate entries must be objects")
        platform = gate.get("platform")
        if not isinstance(platform, str) or not platform:
            raise RuntimeError("platform gate entry missing platform")
        if platform in gate_map:
            raise RuntimeError(f"duplicate platform gate entry: {platform}")

        for field in REQUIRED_GATE_FIELDS:
            value = gate.get(field)
            if value != "passed":
                raise RuntimeError(f"{platform}: {field} gate not passed")

        gate_map[platform] = gate

    gate_platforms = set(gate_map.keys())
    if gate_platforms != contract_platforms:
        missing = sorted(contract_platforms - gate_platforms)
        extra = sorted(gate_platforms - contract_platforms)
        raise RuntimeError(
            "platform_gates mismatch: missing="
            + ",".join(missing)
            + " extra="
            + ",".join(extra)
        )

    print(
        f"[OK] hardening gates passed for all platforms ({len(gate_platforms)}) in release train {release_train}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
