#!/usr/bin/env python3
"""Export network settings from environments.toml for shell scripts or Terraform."""

import argparse
import json
import shlex
import sys
import tomllib
from pathlib import Path


def load_config(path: Path) -> dict:
    with path.open("rb") as config_file:
        config = tomllib.load(config_file)
    if not isinstance(config, dict):
        raise ValueError("environment configuration must be a TOML table")
    return config


def network_config(config: dict, network: str) -> dict:
    selected = config.get(network)
    if not isinstance(selected, dict):
        raise ValueError(f"network {network!r} is not defined in environments.toml")
    for key in ("rpc_url", "network_passphrase"):
        if not isinstance(selected.get(key), str) or not selected[key]:
            raise ValueError(f"network {network!r} must define {key}")
    return selected


def emit_shell(selected: dict) -> None:
    for name, value in (
        ("STELLAR_RPC_URL", selected["rpc_url"]),
        ("STELLAR_NETWORK_PASSPHRASE", selected["network_passphrase"]),
    ):
        print(f'if [[ -z "${{{name}:-}}" ]]; then')
        print(f"  export {name}={shlex.quote(value)}")
        print("fi")


def emit_terraform(config: dict, selected: dict) -> None:
    cidrs = config.get("aws", {}).get("vpc_cidrs", {})
    if not isinstance(cidrs, dict) or not cidrs:
        raise ValueError("environments.toml must define [aws.vpc_cidrs]")
    endpoints = selected.get("rpc_urls")
    if endpoints is None:
        endpoints = {region: selected["rpc_url"] for region in cidrs}
    if not isinstance(endpoints, dict) or set(endpoints) != set(cidrs):
        raise ValueError("network rpc_urls must provide an endpoint for each configured AWS region")
    if any(not isinstance(url, str) or not url for url in endpoints.values()):
        raise ValueError("all regional RPC endpoints must be non-empty strings")
    print(
        json.dumps(
            {
                "network_passphrase": selected["network_passphrase"],
                "rpc_urls": endpoints,
                "vpc_cidrs": cidrs,
            },
            indent=2,
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("shell", "terraform"), default="shell")
    parser.add_argument(
        "--config",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "environments.toml",
    )
    parser.add_argument("network", help="Network name from environments.toml")
    args = parser.parse_args()

    try:
        config = load_config(args.config)
        selected = network_config(config, args.network)
        if args.format == "shell":
            emit_shell(selected)
        else:
            emit_terraform(config, selected)
    except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(f"network config error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
