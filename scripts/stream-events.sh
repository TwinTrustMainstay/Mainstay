#!/usr/bin/env bash
# Poll Soroban events and emit newline-delimited JSON for a live dashboard.
#
# Usage:
#   CONTRACT_LIFECYCLE=... STELLAR_RPC_URL=... ./scripts/stream-events.sh
#
# Environment:
#   CONTRACT_LIFECYCLE  (required) contract ID to filter events for
#   STELLAR_RPC_URL     (required) Soroban RPC endpoint
#   POLL_SECONDS        (default 5) delay between successful polls
#   STATE_FILE          (default .stream-events-state.json) persistence file
#   START_LEDGER        (optional) ledger to start from when no state exists;
#                       defaults to the current ledger reported by the RPC
#   MAX_RETRIES         (default 0 = retry forever) max consecutive RPC failures
#   BACKOFF_BASE_SECONDS (default 1) initial backoff delay
#   BACKOFF_MAX_SECONDS  (default 60) maximum backoff delay
#   SEEN_IDS_MAX        (default 10000) number of recent event IDs to remember

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
source .env 2>/dev/null || true
source "$ROOT/scripts/load-network-config.sh"

: "${CONTRACT_LIFECYCLE:?Set CONTRACT_LIFECYCLE}"
: "${STELLAR_RPC_URL:?Set STELLAR_RPC_URL}"
: "${POLL_SECONDS:=5}"
: "${STATE_FILE:=.stream-events-state.json}"
: "${MAX_RETRIES:=0}"
: "${BACKOFF_BASE_SECONDS:=1}"
: "${BACKOFF_MAX_SECONDS:=60}"
: "${SEEN_IDS_MAX:=10000}"

log() {
    echo "[stream-events] $*" >&2
}

# ---------------------------------------------------------------------------
# State persistence
# ---------------------------------------------------------------------------

cursor=""
last_ledger=""

load_state() {
    if [[ -f "$STATE_FILE" ]]; then
        local loaded_cursor loaded_ledger
        loaded_cursor="$(jq -r '.cursor // empty' "$STATE_FILE" 2>/dev/null || true)"
        loaded_ledger="$(jq -r '.lastLedger // empty' "$STATE_FILE" 2>/dev/null || true)"
        if [[ -n "$loaded_cursor" || -n "$loaded_ledger" ]]; then
            cursor="$loaded_cursor"
            last_ledger="$loaded_ledger"
            log "resuming from state file $STATE_FILE (cursor=${cursor:-<none>} lastLedger=${last_ledger:-<none>})"
            return 0
        fi
        log "state file $STATE_FILE present but unreadable; ignoring"
    fi
    return 1
}

save_state() {
    local tmp
    tmp="$(mktemp "${STATE_FILE}.XXXXXX")"
    jq -nc \
        --arg cursor "$cursor" \
        --arg lastLedger "$last_ledger" \
        '{cursor:$cursor, lastLedger:$lastLedger}' >"$tmp"
    mv -f "$tmp" "$STATE_FILE"
}

# ---------------------------------------------------------------------------
# RPC helpers
# ---------------------------------------------------------------------------

# rpc_call <json-request> -> prints response body on success, returns non-zero on failure
rpc_call() {
    local request="$1"
    curl --fail-with-body --silent --show-error \
        -H 'content-type: application/json' \
        --data "$request" "$STELLAR_RPC_URL"
}

# rpc_call_with_retry <json-request> -> prints response body, retries with backoff
rpc_call_with_retry() {
    local request="$1"
    local attempt=0
    local delay="$BACKOFF_BASE_SECONDS"
    local response

    while true; do
        if response="$(rpc_call "$request")" && [[ -n "$response" ]] \
            && echo "$response" | jq -e '.result' >/dev/null 2>&1; then
            printf '%s' "$response"
            return 0
        fi

        attempt=$((attempt + 1))
        if [[ "$MAX_RETRIES" -gt 0 && "$attempt" -ge "$MAX_RETRIES" ]]; then
            log "RPC failed after $attempt attempt(s); giving up"
            return 1
        fi

        # Exponential backoff with jitter.
        local jitter=$((RANDOM % (delay + 1)))
        local sleep_for=$((delay + jitter))
        log "RPC error (attempt $attempt); retrying in ${sleep_for}s"
        sleep "$sleep_for"

        delay=$((delay * 2))
        if [[ "$delay" -gt "$BACKOFF_MAX_SECONDS" ]]; then
            delay="$BACKOFF_MAX_SECONDS"
        fi
    done
}

# fetch_current_ledger -> prints the latest ledger sequence
fetch_current_ledger() {
    local request response
    request='{"jsonrpc":"2.0","id":1,"method":"getLatestLedger","params":{}}'
    response="$(rpc_call_with_retry "$request")" || return 1
    echo "$response" | jq -r '.result.sequence // empty'
}

# ---------------------------------------------------------------------------
# De-duplication
# ---------------------------------------------------------------------------

declare -A seen_ids=()
declare -a seen_order=()

remember_id() {
    local id="$1"
    [[ -z "$id" ]] && return 0
    if [[ -n "${seen_ids[$id]:-}" ]]; then
        return 0
    fi
    seen_ids["$id"]=1
    seen_order+=("$id")
    while [[ "${#seen_order[@]}" -gt "$SEEN_IDS_MAX" ]]; do
        local evicted="${seen_order[0]}"
        seen_order=("${seen_order[@]:1}")
        unset 'seen_ids[$evicted]'
    done
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

if ! load_state; then
    if [[ -n "${START_LEDGER:-}" ]]; then
        last_ledger="$START_LEDGER"
        log "starting from START_LEDGER=$last_ledger"
    else
        last_ledger="$(fetch_current_ledger)" || {
            log "unable to determine current ledger; aborting"
            exit 1
        }
        log "starting from current ledger $last_ledger"
    fi
    save_state
fi

while true; do
    if [[ -n "$cursor" ]]; then
        request="$(jq -nc \
            --arg contract "$CONTRACT_LIFECYCLE" \
            --arg cursor "$cursor" \
            '{
              jsonrpc:"2.0", id:1, method:"getEvents",
              params:{filters:[{type:"contract", contractIds:[$contract]}],
                      pagination:{limit:100, cursor:$cursor}}
            }')"
    else
        request="$(jq -nc \
            --arg contract "$CONTRACT_LIFECYCLE" \
            --argjson startLedger "${last_ledger:-0}" \
            '{
              jsonrpc:"2.0", id:1, method:"getEvents",
              params:{startLedger:$startLedger,
                      filters:[{type:"contract", contractIds:[$contract]}],
                      pagination:{limit:100}}
            }')"
    fi

    if ! response="$(rpc_call_with_retry "$request")"; then
        log "unable to fetch events; exiting"
        exit 1
    fi

    # Emit events, skipping any IDs already seen.
    while IFS= read -r event; do
        [[ -z "$event" ]] && continue
        event_id="$(echo "$event" | jq -r '.id // empty')"
        if [[ -n "$event_id" && -n "${seen_ids[$event_id]:-}" ]]; then
            continue
        fi
        remember_id "$event_id"
        echo "$event"
        ledger="$(echo "$event" | jq -r '.ledger // empty')"
        if [[ -n "$ledger" ]]; then
            last_ledger="$ledger"
        fi
    done < <(echo "$response" | jq -c '.result.events[]?')

    new_cursor="$(echo "$response" | jq -r '.result.pagination.cursor // empty')"
    if [[ -n "$new_cursor" ]]; then
        cursor="$new_cursor"
    fi

    latest_ledger="$(echo "$response" | jq -r '.result.latestLedger // empty')"
    if [[ -n "$latest_ledger" ]]; then
        last_ledger="$latest_ledger"
    fi

    save_state

    sleep "$POLL_SECONDS"
done