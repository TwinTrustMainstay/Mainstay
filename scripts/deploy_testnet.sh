#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true
source "$ROOT/scripts/load-network-config.sh"
if [[ "${STELLAR_NETWORK}" != "testnet" ]]; then
  echo "Refusing to deploy: STELLAR_NETWORK must be 'testnet' (got '${STELLAR_NETWORK}')." >&2
  exit 1
fi

echo "Deploying to testnet..."

stellar keys generate deployer --network testnet 2>/dev/null || true

for contract in asset-registry engineer-registry lifecycle; do
    echo "Deploying $contract..."
    stellar contract deploy \
        --wasm target/wasm32-unknown-unknown/release/${contract//-/_}.wasm \
        --source deployer \
        --network testnet
done

echo "Deployment complete."
