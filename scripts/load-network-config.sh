#!/usr/bin/env bash
set -euo pipefail

: "${STELLAR_NETWORK:=testnet}"
NETWORK_CONFIG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if ! network_config_exports="$(
    python3 "$NETWORK_CONFIG_ROOT/scripts/export-network-config.py" "$STELLAR_NETWORK"
)" || [[ -z "${network_config_exports:-}" ]]; then
    if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
        return 1
    else
        exit 1
    fi
fi

# shellcheck disable=SC1091
source /dev/stdin <<<"$network_config_exports"
export STELLAR_NETWORK
