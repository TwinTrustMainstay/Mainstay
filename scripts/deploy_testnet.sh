#!/usr/bin/env bash
set -euo pipefail

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true

: "${STELLAR_NETWORK:=testnet}"
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
