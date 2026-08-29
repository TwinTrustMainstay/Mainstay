#!/usr/bin/env bash
# examples/credential_management.sh
# Credential rotation workflow for the Mainstay Engineer Registry.
#
# Prerequisites: Stellar CLI installed and available on PATH.
# Usage: ./examples/credential_management.sh

set -euo pipefail

NETWORK="${STELLAR_NETWORK:-testnet}"
RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"
CONTRACT="${CONTRACT_ENGINEER_REGISTRY:?CONTRACT_ENGINEER_REGISTRY not set}"
ADMIN="${ADMIN_ADDRESS:?ADMIN_ADDRESS not set}"
ISSUER="${ISSUER_ADDRESS:?ISSUER_ADDRESS not set}"
ENGINEER="${ENGINEER_ADDRESS:?ENGINEER_ADDRESS not set}"
CRED_HASH="${CREDENTIAL_HASH:?CREDENTIAL_HASH not set}"
VALIDITY_PERIOD="${VALIDITY_PERIOD:-31536000}"  # 1 year in seconds

invoke() {
    stellar contract invoke \
        --network "$NETWORK" \
        --rpc-url "$RPC_URL" \
        --id "$CONTRACT" \
        -- "$@"
}

# 1. Register engineer
echo "Registering engineer $ENGINEER ..."
invoke register_engineer \
    --engineer "$ENGINEER" \
    --credential_hash "$CRED_HASH" \
    --issuer "$ISSUER" \
    --validity_period "$VALIDITY_PERIOD"

# 2. Verify credential is active
echo "Verifying engineer credential ..."
active=$(invoke verify_engineer --engineer "$ENGINEER")
[ "$active" = "true" ] || { echo "ERROR: verification failed"; exit 1; }
echo "Credential verified: $active"

# 3. Renew credential (rotation)
echo "Renewing credential for $ENGINEER ..."
invoke renew_credential \
    --engineer "$ENGINEER" \
    --new_validity_period "$VALIDITY_PERIOD"

# 4. Confirm status after renewal
echo "Checking engineer status after renewal ..."
status=$(invoke get_engineer_status --engineer "$ENGINEER")
echo "Status: $status"

# 5. Revoke credential
echo "Revoking credential for $ENGINEER ..."
invoke revoke_credential --engineer "$ENGINEER"

# 6. Confirm revocation
echo "Confirming revocation ..."
revoked=$(invoke verify_engineer --engineer "$ENGINEER")
echo "Active after revocation: $revoked"
[ "$revoked" = "false" ] || { echo "ERROR: revocation check failed"; exit 1; }

echo "Credential management workflow completed successfully."
