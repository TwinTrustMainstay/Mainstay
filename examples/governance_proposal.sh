#!/usr/bin/env bash
# examples/governance_proposal.sh
# End-to-end proposal flow against the Mainstay Governance contract:
# fund treasury → mint shares → create proposal → vote → finalize → execute.
#
# Prerequisites: Stellar CLI (`stellar`) installed, and the governance
# contract already deployed with its initialize(admin, config) call run.
#
# Required env vars (addresses are Stellar strkeys unless stated otherwise):
#   CONTRACT_GOVERNANCE      Deployed governance contract id (C…)
#   ADMIN_ADDRESS            Admin / signer address (G… or C…)
#   HOLDER_ADDRESS           Share-holder who proposes and votes
#   PARAM_NAME               Symbol parameter the proposal targets, e.g. fee_bps
#   PARAM_VALUE              New u64 value for the parameter
#
# Optional:
#   STELLAR_NETWORK          Default: testnet
#   STELLAR_RPC_URL          Default: https://soroban-testnet.stellar.org
#   SHARES_TO_MINT           Default: 1000
#   PROPOSAL_TITLE           Default: "Raise platform fee"
#
# Usage: ./examples/governance_proposal.sh

set -euo pipefail

NETWORK="${STELLAR_NETWORK:-testnet}"
RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"
CONTRACT="${CONTRACT_GOVERNANCE:?CONTRACT_GOVERNANCE not set}"
ADMIN="${ADMIN_ADDRESS:?ADMIN_ADDRESS not set}"
HOLDER="${HOLDER_ADDRESS:?HOLDER_ADDRESS not set}"
PARAM_NAME="${PARAM_NAME:?PARAM_NAME not set (symbol)}"
PARAM_VALUE="${PARAM_VALUE:?PARAM_VALUE not set (u64)}"
SHARES_TO_MINT="${SHARES_TO_MINT:-1000}"
PROPOSAL_TITLE="${PROPOSAL_TITLE:-Raise platform fee}"

invoke() {
    stellar contract invoke \
        --network "$NETWORK" \
        --rpc-url "$RPC_URL" \
        --id "$CONTRACT" \
        -- "$@"
}

# 1. Admin mints voting shares to the holder.
echo "Minting $SHARES_TO_MINT shares to $HOLDER ..."
invoke mint_shares \
    --admin "$ADMIN" \
    --holder "$HOLDER" \
    --amount "$SHARES_TO_MINT"

# 2. Holder creates a parameter proposal.
echo "Creating proposal \"$PROPOSAL_TITLE\" (param=$PARAM_NAME value=$PARAM_VALUE) ..."
PROPOSAL_ID=$(invoke create_proposal \
    --proposer "$HOLDER" \
    --title "$PROPOSAL_TITLE" \
    --kind "{\"Parameter\": [\"$PARAM_NAME\", $PARAM_VALUE]}")
echo "Created proposal #$PROPOSAL_ID"

# 3. Holder casts a supporting vote.
echo "Voting in favour of proposal #$PROPOSAL_ID ..."
invoke vote \
    --voter "$HOLDER" \
    --proposal_id "$PROPOSAL_ID" \
    --support true

# 4. Finalize the proposal (anyone can call once voting_period_secs has elapsed).
# In automated environments you may want to advance the ledger time first.
echo "Finalizing proposal #$PROPOSAL_ID ..."
STATUS=$(invoke finalize_proposal --proposal_id "$PROPOSAL_ID")
echo "Finalized status: $STATUS"

# 5. Execute if it passed.
if [ "$STATUS" = '"Passed"' ]; then
    echo "Executing proposal #$PROPOSAL_ID ..."
    invoke execute_proposal --proposal_id "$PROPOSAL_ID"
    echo "Executed."
else
    echo "Proposal did not pass; nothing to execute."
fi

echo "Done."
