#!/usr/bin/env bash
set -euo pipefail

source ~/.cargo/env 2>/dev/null || true

# Strict .env loader: only accepts KEY=value lines matching a strict regex.
# Never executes .env contents as shell code.
load_env() {
  local file="$1"
  [[ -f "$file" ]] || return 0
  local line k v
  while IFS= read -r line || [[ -n "$line" ]]; do
    # Skip blank lines and comments
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    if [[ "$line" =~ ^[A-Za-z_][A-Za-z0-9_]*=.*$ ]]; then
      k="${line%%=*}"
      v="${line#*=}"
      export "$k=$v"
    else
      echo "Warning: ignoring malformed line in $file: $line" >&2
    fi
  done < "$file"
}

load_env .env

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