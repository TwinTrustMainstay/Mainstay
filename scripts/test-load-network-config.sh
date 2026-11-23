#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOAD_NETWORK_CONFIG="$ROOT/scripts/load-network-config.sh"

failures=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

for network in dev staging testnet mainnet futurenet standalone; do
    export STELLAR_NETWORK="$network"
    # shellcheck disable=SC1090
    if source "$LOAD_NETWORK_CONFIG"; then
        if [[ -n "${STELLAR_RPC_URL:-}" ]] && [[ -n "${STELLAR_NETWORK_PASSPHRASE:-}" ]]; then
            pass "valid network '$network' loads correctly (RPC: $STELLAR_RPC_URL)"
        else
            fail "valid network '$network' loaded but variables are empty"
        fi
    else
        fail "valid network '$network' failed to load"
    fi
    unset STELLAR_RPC_URL STELLAR_NETWORK_PASSPHRASE STELLAR_NETWORK
done

export STELLAR_NETWORK="nonexistent"
# shellcheck disable=SC1090
if source "$LOAD_NETWORK_CONFIG" >/dev/null 2>&1; then
    fail "missing network 'nonexistent' should have failed when sourced"
else
    pass "missing network 'nonexistent' fails when sourced (exit code: $?)"
fi
if bash "$LOAD_NETWORK_CONFIG" >/dev/null 2>&1; then
    fail "missing network 'nonexistent' should have failed when executed"
else
    pass "missing network 'nonexistent' fails when executed (exit code: $?)"
fi
unset STELLAR_NETWORK

export STELLAR_NETWORK=""
# shellcheck disable=SC1090
if source "$LOAD_NETWORK_CONFIG"; then
    if [[ -n "${STELLAR_RPC_URL:-}" ]] && [[ -n "${STELLAR_NETWORK_PASSPHRASE:-}" ]] && [[ "${STELLAR_NETWORK:-}" == "testnet" ]]; then
        pass "empty network name falls back to default (testnet)"
    else
        fail "empty network name fell back but variables incorrect: STELLAR_NETWORK=${STELLAR_NETWORK:-<unset>}"
    fi
else
    fail "empty network name should fall back to default but failed"
fi
unset STELLAR_RPC_URL STELLAR_NETWORK_PASSPHRASE STELLAR_NETWORK

unset STELLAR_NETWORK
# shellcheck disable=SC1090
if source "$LOAD_NETWORK_CONFIG"; then
    if [[ -n "${STELLAR_RPC_URL:-}" ]] && [[ -n "${STELLAR_NETWORK_PASSPHRASE:-}" ]] && [[ "${STELLAR_NETWORK:-}" == "testnet" ]]; then
        pass "default network (testnet) loads correctly when STELLAR_NETWORK unset"
    else
        fail "default network loaded but variables incorrect: STELLAR_NETWORK=${STELLAR_NETWORK:-<unset>}"
    fi
else
    fail "default network (testnet) failed to load"
fi
unset STELLAR_RPC_URL STELLAR_NETWORK_PASSPHRASE STELLAR_NETWORK

if [[ "$failures" -gt 0 ]]; then
    echo "$failures test(s) failed" >&2
    exit 1
fi
echo "All load-network-config tests passed."
