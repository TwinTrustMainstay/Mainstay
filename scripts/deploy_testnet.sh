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

# Generate the deployer key. Do not suppress failures: a real key-generation
# error must abort the script rather than being silently ignored.
stellar keys generate deployer --network testnet

DEPLOYER_ADDR="$(stellar keys address deployer)"
if [[ -z "${DEPLOYER_ADDR}" ]]; then
  echo "Failed to resolve deployer address." >&2
  exit 1
fi
echo "Deployer address: ${DEPLOYER_ADDR}"

# Fund the deployer through friendbot if its balance is zero.
get_balance() {
  local addr="$1"
  local balance
  balance="$(curl -fsS "https://horizon-testnet.stellar.org/accounts/${addr}" 2>/dev/null \
    | grep -o '"balance":"[0-9.]*"' \
    | head -n1 \
    | sed -E 's/.*"balance":"([0-9.]*)".*/\1/' || true)"
  echo "${balance:-0}"
}

BALANCE="$(get_balance "${DEPLOYER_ADDR}")"
if [[ -z "${BALANCE}" || "${BALANCE}" == "0" || "${BALANCE}" == "0.0000000" ]]; then
  echo "Deployer balance is zero; funding via friendbot..."
  if ! curl -fsS "https://friendbot.stellar.org/?addr=${DEPLOYER_ADDR}" >/dev/null; then
    echo "Friendbot funding failed for ${DEPLOYER_ADDR}." >&2
    exit 1
  fi
  # Wait until the account is funded before continuing.
  for _ in $(seq 1 30); do
    BALANCE="$(get_balance "${DEPLOYER_ADDR}")"
    if [[ -n "${BALANCE}" && "${BALANCE}" != "0" && "${BALANCE}" != "0.0000000" ]]; then
      break
    fi
    sleep 2
  done
  if [[ -z "${BALANCE}" || "${BALANCE}" == "0" || "${BALANCE}" == "0.0000000" ]]; then
    echo "Deployer account ${DEPLOYER_ADDR} was not funded in time." >&2
    exit 1
  fi
fi
echo "Deployer balance: ${BALANCE} XLM"

deploy_contract() {
  local contract="$1"
  local wasm="target/wasm32-unknown-unknown/release/${contract//-/_}.wasm"
  echo "Deploying ${contract}..." >&2
  local id
  id="$(stellar contract deploy \
      --wasm "${wasm}" \
      --source deployer \
      --network testnet)"
  if [[ -z "${id}" ]]; then
    echo "Deployment of ${contract} returned an empty contract ID." >&2
    exit 1
  fi
  echo "${id}"
}

AR_ID="$(deploy_contract asset-registry)"
ER_ID="$(deploy_contract engineer-registry)"
LC_ID="$(deploy_contract lifecycle)"

mkdir -p deployments
cat > deployments/testnet.env <<EOF
CONTRACT_ASSET_REGISTRY=${AR_ID}
CONTRACT_ENGINEER_REGISTRY=${ER_ID}
CONTRACT_LIFECYCLE=${LC_ID}
EOF
echo "Wrote deployments/testnet.env"

# Initialize contracts with the deployer as admin, mirroring ci.yml.
run_init() {
  local label="$1"
  shift
  if ! "$@"; then
    echo "Initialization failed for ${label}." >&2
    exit 1
  fi
}

run_init "asset-registry" \
  stellar contract invoke \
    --id "${AR_ID}" \
    --source deployer \
    --network testnet \
    -- initialize_admin \
    --deployer "${DEPLOYER_ADDR}" \
    --admin "${DEPLOYER_ADDR}"

run_init "engineer-registry" \
  stellar contract invoke \
    --id "${ER_ID}" \
    --source deployer \
    --network testnet \
    -- initialize_admin \
    --deployer "${DEPLOYER_ADDR}" \
    --admin "${DEPLOYER_ADDR}"

run_init "lifecycle" \
  stellar contract invoke \
    --id "${LC_ID}" \
    --source deployer \
    --network testnet \
    -- initialize \
    --deployer "${DEPLOYER_ADDR}" \
    --asset_registry "${AR_ID}" \
    --engineer_registry "${ER_ID}" \
    --admin "${DEPLOYER_ADDR}" \
    --max_history 200

echo ""
echo "Deployment summary:"
printf '%-24s %s\n' "CONTRACT" "ID"
printf '%-24s %s\n' "asset-registry" "${AR_ID}"
printf '%-24s %s\n' "engineer-registry" "${ER_ID}"
printf '%-24s %s\n' "lifecycle" "${LC_ID}"
echo ""
echo "Contract IDs written to deployments/testnet.env."
echo "Source it (e.g. 'set -a; source deployments/testnet.env; set +a') before running backup.sh, batch-import.sh, or the exporter."