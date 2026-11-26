#!/usr/bin/env bash
# examples/bridge_atomic_swap.sh
# Hashed-timelock atomic swap flow on the Mainstay Bridge contract:
# create_swap → claim_swap with the preimage. A refund branch is shown
# for the timeout case.
#
# Prerequisites: Stellar CLI (`stellar`) installed, the bridge contract
# already deployed with initialize(admin) run, and a supported token
# whose allowance has been granted to the bridge for the swap amount.
#
# Required env vars (strkeys unless stated otherwise):
#   CONTRACT_BRIDGE          Deployed bridge contract id (C…)
#   ADMIN_ADDRESS            Admin signer
#   INITIATOR_ADDRESS        Party funding the swap
#   COUNTERPARTY_ADDRESS     Party able to claim with the preimage
#   TOKEN_ADDRESS            SAC/TOKEN contract id for the locked asset
#   PREIMAGE_HEX             32-byte preimage, hex-encoded (no 0x prefix)
#   AMOUNT                   i128, positive
#
# Optional:
#   STELLAR_NETWORK          Default: testnet
#   STELLAR_RPC_URL          Default: https://soroban-testnet.stellar.org
#   TIMEOUT_SECS             Default: 3600 (1 hour)
#
# Usage: ./examples/bridge_atomic_swap.sh

set -euo pipefail

NETWORK="${STELLAR_NETWORK:-testnet}"
RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"
CONTRACT="${CONTRACT_BRIDGE:?CONTRACT_BRIDGE not set}"
ADMIN="${ADMIN_ADDRESS:?ADMIN_ADDRESS not set}"
INITIATOR="${INITIATOR_ADDRESS:?INITIATOR_ADDRESS not set}"
COUNTERPARTY="${COUNTERPARTY_ADDRESS:?COUNTERPARTY_ADDRESS not set}"
TOKEN="${TOKEN_ADDRESS:?TOKEN_ADDRESS not set}"
PREIMAGE_HEX="${PREIMAGE_HEX:?PREIMAGE_HEX not set (32-byte hex)}"
AMOUNT="${AMOUNT:?AMOUNT not set}"
TIMEOUT_SECS="${TIMEOUT_SECS:-3600}"

# Compute the SHA-256 of the preimage (the hashlock the contract stores).
HASHLOCK_HEX=$(printf '%s' "$PREIMAGE_HEX" | xxd -r -p | sha256sum | cut -d' ' -f1)

invoke() {
    stellar contract invoke \
        --network "$NETWORK" \
        --rpc-url "$RPC_URL" \
        --id "$CONTRACT" \
        -- "$@"
}

# 1. Initiator opens the swap: locks AMOUNT of TOKEN behind the hashlock.
#    `create_swap` returns the new swap_id.
echo "Creating swap: initiator=$INITIATOR counterparty=$COUNTERPARTY amount=$AMOUNT ..."
SWAP_ID=$(invoke create_swap \
    --initiator "$INITIATOR" \
    --counterparty "$COUNTERPARTY" \
    --token "$TOKEN" \
    --amount "$AMOUNT" \
    --hashlock "$HASHLOCK_HEX" \
    --timeout_secs "$TIMEOUT_SECS")
echo "Created swap #$SWAP_ID"

# 2. Counterparty reveals the preimage to claim the funds.
echo "Counterparty claiming swap #$SWAP_ID with preimage ..."
invoke claim_swap \
    --swap_id "$SWAP_ID" \
    --preimage "$PREIMAGE_HEX"

# 3. Read-back: confirm the swap moved to the Claimed state.
echo "Final swap state:"
invoke get_swap --swap_id "$SWAP_ID"

cat <<'NOTE'

Refund branch (not executed here)
---------------------------------
If the counterparty does not claim within timeout_secs, the initiator can
recover the funds instead of claiming:

    stellar contract invoke --id "$CONTRACT" -- refund_swap --swap_id "$SWAP_ID"

NOTE
echo "Done."
