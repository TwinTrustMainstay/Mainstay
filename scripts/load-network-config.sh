#!/usr/bin/env bash

: "${STELLAR_NETWORK:=testnet}"
NETWORK_CONFIG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
network_config_exports="$(
    python3 "$NETWORK_CONFIG_ROOT/scripts/export-network-config.py" "$STELLAR_NETWORK"
)" || return 1
source /dev/stdin <<<"$network_config_exports"
export STELLAR_NETWORK
