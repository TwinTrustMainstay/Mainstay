#!/usr/bin/env bash
# examples/marketplace_auction.sh
# Open descending-price auction flow against the Mainstay Marketplace
# contract: create_auction → place_bid → finalize_auction, with read-backs
# of the current dynamic price and winning bid.
#
# Prerequisites: Stellar CLI (`stellar`) installed, the marketplace already
# deployed and initialized, and the bidder registered in the engineer
# registry referenced by the marketplace.
#
# Required env vars (strkeys unless stated otherwise):
#   CONTRACT_MARKETPLACE     Deployed marketplace contract id (C…)
#   ADMIN_ADDRESS            Admin signer who creates the auction
#   BIDDER_ADDRESS           Credentialed engineer who will bid
#   ASSET_ID                 u64 asset the auction is for
#   TASK_TYPE                Symbol task type, e.g. design
#   BID_PRICE                u64 bid amount (must be ≤ ceiling_price)
#
# Optional:
#   STELLAR_NETWORK          Default: testnet
#   STELLAR_RPC_URL          Default: https://soroban-testnet.stellar.org
#   BID_WINDOW_SECS          Default: 300 (5 minutes)
#   WINNER_CRITERION         Default: "LowestPrice"
#   CEILING_PRICE            Optional override, default: null (use dynamic)
#
# Usage: ./examples/marketplace_auction.sh

set -euo pipefail

NETWORK="${STELLAR_NETWORK:-testnet}"
RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"
CONTRACT="${CONTRACT_MARKETPLACE:?CONTRACT_MARKETPLACE not set}"
ADMIN="${ADMIN_ADDRESS:?ADMIN_ADDRESS not set}"
BIDDER="${BIDDER_ADDRESS:?BIDDER_ADDRESS not set}"
ASSET_ID="${ASSET_ID:?ASSET_ID not set}"
TASK_TYPE="${TASK_TYPE:?TASK_TYPE not set (symbol)}"
BID_PRICE="${BID_PRICE:?BID_PRICE not set}"
BID_WINDOW_SECS="${BID_WINDOW_SECS:-300}"
WINNER_CRITERION="${WINNER_CRITERION:-LowestPrice}"

NOW=$(date +%s)
BID_END=$((NOW + BID_WINDOW_SECS))

if [ -n "${CEILING_PRICE:-}" ]; then
    CEILING_FIELD="\"Some\": $CEILING_PRICE"
else
    CEILING_FIELD="\"None\": null"
fi

invoke() {
    stellar contract invoke \
        --network "$NETWORK" \
        --rpc-url "$RPC_URL" \
        --id "$CONTRACT" \
        -- "$@"
}

# 1. Read current dynamic price so the user knows the ceiling they'll hit
#    if they don't override it.
echo "Current dynamic price:"
invoke get_price

# 2. Admin creates an open (non-sealed) descending-price auction.
echo "Creating open auction: asset=$ASSET_ID task_type=$TASK_TYPE bid_end=$BID_END ..."
PARAMS=$(cat <<EOF
{
  "asset_id": $ASSET_ID,
  "task_type": "$TASK_TYPE",
  "ceiling_price": { $CEILING_FIELD },
  "sealed": false,
  "criterion": "$WINNER_CRITERION",
  "bid_end": $BID_END,
  "reveal_end": 0
}
EOF
)
AUCTION_ID=$(invoke create_auction --admin "$ADMIN" --params "$PARAMS")
echo "Created auction #$AUCTION_ID"

# 3. Bidder places a bid.
echo "Placing bid: engineer=$BIDDER price=$BID_PRICE ..."
invoke place_bid \
    --engineer "$BIDDER" \
    --auction_id "$AUCTION_ID" \
    --price "$BID_PRICE"

# 4. Wait out the bidding window before finalizing. In automated CI you can
#    advance the ledger timestamp instead of a real sleep.
echo "Waiting $BID_WINDOW_SECS s for the bidding window to close ..."
sleep "$BID_WINDOW_SECS"

echo "Finalizing auction #$AUCTION_ID ..."
invoke finalize_auction --auction_id "$AUCTION_ID"

echo "Final auction state:"
invoke get_auction --auction_id "$AUCTION_ID"

echo "Done."
