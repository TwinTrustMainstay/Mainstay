#!/usr/bin/env bash
# Poll Soroban events and emit newline-delimited JSON for a live dashboard.
#
# Usage:
#   CONTRACT_LIFECYCLE=... STELLAR_RPC_URL=... ./scripts/stream-events.sh

set -euo pipefail

: "${CONTRACT_LIFECYCLE:?Set CONTRACT_LIFECYCLE}"
: "${STELLAR_RPC_URL:?Set STELLAR_RPC_URL}"
: "${POLL_SECONDS:=5}"

cursor=""
while true; do
    request="$(jq -nc \
        --arg contract "$CONTRACT_LIFECYCLE" \
        --arg cursor "$cursor" \
        '{
          jsonrpc:"2.0", id:1, method:"getEvents",
          params:{startLedger:0, filters:[{type:"contract", contractIds:[$contract]}],
                  pagination:({limit:100} + (if $cursor == "" then {} else {cursor:$cursor} end))}
        }')"
    response="$(curl --fail-with-body --silent --show-error \
        -H 'content-type: application/json' \
        --data "$request" "$STELLAR_RPC_URL")"
    echo "$response" | jq -c '.result.events[]?'
    cursor="$(echo "$response" | jq -r '.result.pagination.cursor // empty')"
    sleep "$POLL_SECONDS"
done
