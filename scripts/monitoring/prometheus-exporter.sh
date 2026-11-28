#!/usr/bin/env bash
# =============================================================================
# Mainstay Prometheus Metrics Exporter
# =============================================================================
# Periodically queries contract health metrics and exposes them via a simple
# HTTP endpoint suitable for Prometheus scraping.
#
# Usage:
#   ./scripts/monitoring/prometheus-exporter.sh [--port <port>] [--bind <address>]
#
# Environment variables:
#   CONTRACT_ASSET_REGISTRY     - Asset Registry contract ID
#   CONTRACT_ENGINEER_REGISTRY  - Engineer Registry contract ID
#   CONTRACT_LIFECYCLE          - Lifecycle contract ID
#   CONTRACT_INSURANCE_POOL     - Insurance Pool contract ID (optional)
#   STELLAR_NETWORK             - Stellar network
#   STELLAR_RPC_URL             - Stellar RPC endpoint
#   EXPORTER_PORT               - HTTP port (default: 9600)
#   EXPORTER_BIND_ADDRESS       - Interface to bind (default: 127.0.0.1)
#                                 Set to the private metrics interface IP so only
#                                 the Prometheus host can reach the exporter.
#   SCRAPE_INTERVAL             - Seconds between scrapes (default: 60)
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

source ~/.cargo/env 2>/dev/null || true

# Load .env safely: only accept strict KEY=value lines and never execute
# the file as shell code (no `source`/`.` on untrusted content).
load_env_file() {
    local file="$1"
    [[ -f "$file" ]] || return 0
    local line k v
    while IFS= read -r line || [[ -n "$line" ]]; do
        # Skip blank lines and comments
        [[ "$line" =~ ^[[:space:]]*$ ]] && continue
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        # Strip a trailing CR (CRLF files) and surrounding whitespace
        line="${line%$'
'}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        if [[ "$line" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
            k="${BASH_REMATCH[1]}"
            v="${BASH_REMATCH[2]}"
            # Strip matching single or double quotes around the value
            if [[ "$v" =~ ^"(.*)"$ ]] || [[ "$v" =~ ^'(.*)'$ ]]; then
                v="${BASH_REMATCH[1]}"
            fi
            export "$k=$v"
        else
            echo "WARNING: ignoring malformed line in $file: $line" >&2
        fi
    done < "$file"
}

load_env_file "$ROOT/.env"

: "${STELLAR_NETWORK:=testnet}"
: "${EXPORTER_PORT:=9600}"
: "${EXPORTER_BIND_ADDRESS:=127.0.0.1}"
: "${SCRAPE_INTERVAL:=60}"

# Private runtime directory for the latest metrics page.  Created with
# mktemp -d under $XDG_RUNTIME_DIR (or a umask 077 directory) so that a local
# user cannot pre-create predictable paths and hijack the writes.
METRICS_DIR=""
METRICS_FILE=""

# Number of chain reads that failed during the current collection cycle.
CHAIN_READ_ERRORS=0

# RPC telemetry counters. Each invoke_read appends one line "<exit_code> <seconds>".
# A file is used instead of shell variables because invoke_read runs inside
# $(...) subshells, where counter updates would be lost.
RPC_LOG="${METRICS_FILE}.rpc"

# Metrics emitted at end of each collection cycle.
MAINSTAY_RPC_REQUESTS_LAST_SCRAPE=0
MAINSTAY_RPC_ERRORS_LAST_SCRAPE=0
MAINSTAY_RPC_REQUEST_DURATION_SECONDS_SUM=0

# ---------------------------------------------------------------------------
# Query helpers
# ---------------------------------------------------------------------------

# invoke_read <contract_id> <method> [args...]
# Prints the command output on success and returns 0.  On failure it prints
# nothing, increments CHAIN_READ_ERRORS and returns 1.
invoke_read() {
    local output
    local start_ts
    start_ts="$(date +%s.%N)"
    if output="$(stellar contract invoke \
        --id "$1" \
        --network "$STELLAR_NETWORK" \
        --source any \
        -- "${@:2}" 2>/dev/null)"; then
        local exit_code=$?
        local end_ts
        end_ts="$(date +%s.%N)"
        local duration
        duration="$(echo "$end_ts - $start_ts" | bc)"
        printf '%s %s
' "$exit_code" "$duration" >> "$RPC_LOG"
        printf '%s
' "$output"
        return 0
    fi
    CHAIN_READ_ERRORS=$((CHAIN_READ_ERRORS + 1))
    printf '1 0
' >> "$RPC_LOG"
    return 1
}

# Read a numeric value, falling back to 0 when the read fails.
invoke_read_number() {
    local value
    if value="$(invoke_read "$@")"; then
        printf '%s
' "$value"
    else
        echo "0"
    fi
}

# ---------------------------------------------------------------------------
# Private metrics directory
# ---------------------------------------------------------------------------

init_metrics_dir() {
    local base="${XDG_RUNTIME_DIR:-}"
    if [[ -z "$base" || ! -d "$base" || ! -w "$base" ]]; then
        base="$(mktemp -d -t mainstay-exporter.XXXXXXXX)"
        chmod 700 "$base"
    fi
    METRICS_DIR="$(mktemp -d "$base/mainstay-exporter.XXXXXXXX")"
    chmod 700 "$METRICS_DIR"
    METRICS_FILE="$METRICS_DIR/mainstay-metrics.prom"
}

# Length of a get_maintenance_history view. A bare integer is used as-is.
# Otherwise each MaintenanceRecord in the CLI output contributes one "asset_id".
history_len_from_view() {
    local raw="$1"
    local trimmed="${raw//[[:space:]]/}"
    if [[ "$trimmed" =~ ^[0-9]+$ ]]; then
        printf '%s
' "$trimmed"
        return 0
    fi
    local without="${raw//"asset_id"/}"
    printf '%s
' "$(( (${#raw} - ${#without}) / 10 ))"
}

# Extract a numeric field from JSON-like contract output (e.g. PoolStats).
# Handles both unquoted numbers ("reserved":100) and quoted i128 strings ("reserved":"100").
extract_json_int() {
    local raw="$1"
    local field="$2"
    if [[ "$raw" =~ \"${field}\"[[:space:]]*:[[:space:]]*\"?(-?[0-9]+)\"? ]]; then
        printf '%s\n' "${BASH_REMATCH[1]}"
    else
        echo "0"
    fi
}

# ---------------------------------------------------------------------------
# Collect metrics from chain
# ---------------------------------------------------------------------------

collect_metrics() {
    local ts
    ts="$(date +%s)"

    CHAIN_READ_ERRORS=0

    # Write to a fresh mktemp file inside the private directory, then move it
    # into place atomically.
    local tmp_file
    tmp_file="$(mktemp "$METRICS_DIR/metrics.XXXXXXXX")"

    # --- Asset Registry metrics ---
    local total_assets
    total_assets="$(invoke_read_number "$CONTRACT_ASSET_REGISTRY" get_asset_count)"

    cat >> "$tmp_file" <<EOF
# HELP mainstay_assets_total Total number of registered assets.
# TYPE mainstay_assets_total gauge
mainstay_assets_total $total_assets

# HELP mainstay_collateral_score Per-asset collateral score (0-100).
# TYPE mainstay_collateral_score gauge
EOF

    # --- Engineer Registry metrics ---
    local total_engineers
    total_engineers="$(invoke_read_number "$CONTRACT_ENGINEER_REGISTRY" get_engineer_count)"

    cat >> "$tmp_file" <<EOF
# HELP mainstay_engineers_total Total number of registered engineers.
# TYPE mainstay_engineers_total gauge
mainstay_engineers_total $total_engineers

# HELP mainstay_days_since_last_service Days since last maintenance service per asset.
# TYPE mainstay_days_since_last_service gauge
EOF

    # --- Maintenance records: iterate all assets for per-asset metrics ---
    # NOTE: For production deployments with 1,000+ assets, this per-asset
    # iteration is expensive.  Consider using a cached metric or a contract
    # view that returns aggregate counts.  Increase SCRAPE_INTERVAL to 300s
    # for large portfolios.
    local total_records=0
    local score_sum=0
    local scored_assets=0

    if [[ -n "$total_assets" && "$total_assets" -gt 0 ]]; then
        for (( id=1; id<=total_assets; id++ )); do
            local hist_len
            hist_len="$(invoke_read_number "$CONTRACT_LIFECYCLE" get_maintenance_history --asset_id "$id")"
            # Count entries (this is approximate — the raw output from stellar CLI
            # is not natively parseable as JSON in all versions; adapt as needed)
            total_records=$((total_records + 1))

            local score
            score="$(invoke_read_number "$CONTRACT_LIFECYCLE" get_collateral_score --asset_id "$id")"
            score_sum=$((score_sum + score))
            if [[ "$score" -gt 0 ]]; then
                scored_assets=$((scored_assets + 1))
            fi

            # Export per-asset collateral score metric
            echo "mainstay_collateral_score{asset_id="$id"} $score" >> "$tmp_file"

            # Calculate days since last service
            local last_service_ts
            last_service_ts="$(invoke_read_number "$CONTRACT_LIFECYCLE" get_last_service_timestamp --asset_id "$id")"
            if [[ "$last_service_ts" -gt 0 ]]; then
                local days_since=$((($ts - $last_service_ts) / 86400))
                echo "mainstay_days_since_last_service{asset_id="$id"} $days_since" >> "$tmp_file"
            else
                # Asset has never been serviced
                echo "mainstay_days_since_last_service{asset_id="$id"} -1" >> "$tmp_file"
            fi
        done
    fi

    local avg_score=0
    if [[ "$scored_assets" -gt 0 ]]; then
        avg_score=$((score_sum / scored_assets))
    fi

    # --- Insurance Pool metrics ---
    local insurance_reserved=0
    local insurance_free=0
    local insurance_open_claims=0

    if [[ -n "${CONTRACT_INSURANCE_POOL:-}" ]]; then
        local pool_stats
        if pool_stats="$(invoke_read "$CONTRACT_INSURANCE_POOL" get_stats)"; then
            local pool_total_assets claims_filed claims_paid claims_rejected
            pool_total_assets="$(extract_json_int "$pool_stats" total_assets)"
            insurance_reserved="$(extract_json_int "$pool_stats" reserved)"
            claims_filed="$(extract_json_int "$pool_stats" claims_filed)"
            claims_paid="$(extract_json_int "$pool_stats" claims_paid)"
            claims_rejected="$(extract_json_int "$pool_stats" claims_rejected)"

            insurance_free=$((pool_total_assets - insurance_reserved))
            if (( insurance_free < 0 )); then
                insurance_free=0
            fi

            insurance_open_claims=$((claims_filed - claims_paid - claims_rejected))
            if (( insurance_open_claims < 0 )); then
                insurance_open_claims=0
            fi
        fi
    fi

    cat >> "$tmp_file" <<EOF

# HELP mainstay_insurance_reserved_capital Insurance pool capital reserved for pending claims.
# TYPE mainstay_insurance_reserved_capital gauge
mainstay_insurance_reserved_capital $insurance_reserved

# HELP mainstay_insurance_free_capital Unencumbered insurance pool capital available for new coverage or withdrawals.
# TYPE mainstay_insurance_free_capital gauge
mainstay_insurance_free_capital $insurance_free

# HELP mainstay_insurance_open_claims_count Number of open (pending) insurance claims.
# TYPE mainstay_insurance_open_claims_count gauge
mainstay_insurance_open_claims_count $insurance_open_claims

# HELP mainstay_maintenance_records_total Total maintenance records across all assets.
# TYPE mainstay_maintenance_records_total gauge
mainstay_maintenance_records_total $total_records

# HELP mainstay_avg_collateral_score Average collateral score across scored assets.
# TYPE mainstay_avg_collateral_score gauge
mainstay_avg_collateral_score $avg_score

# HELP mainstay_scored_assets_count Number of assets with non-zero collateral scores.
# TYPE mainstay_scored_assets_count gauge
mainstay_scored_assets_count $scored_assets

# HELP mainstay_chain_read_errors_total Number of chain reads that failed during the last collection.
# TYPE mainstay_chain_read_errors_total gauge
mainstay_chain_read_errors_total $CHAIN_READ_ERRORS
EOF

    # --- Bridge metrics ---
    # The bridge contract exposes aggregate counters so the exporter does not
    # need to enumerate persistent release or swap keys.
    local bridge_pending_releases=0
    local bridge_open_swaps=0
    if [[ -n "${CONTRACT_BRIDGE:-}" ]]; then
        bridge_pending_releases="$(invoke_read_number "$CONTRACT_BRIDGE" get_pending_release_count)"
        bridge_open_swaps="$(invoke_read_number "$CONTRACT_BRIDGE" get_open_swap_count)"
    fi

    cat >> "$tmp_file" <<EOF

# HELP mainstay_bridge_pending_releases Releases with at least one attestation that have not executed or expired.
# TYPE mainstay_bridge_pending_releases gauge
mainstay_bridge_pending_releases $bridge_pending_releases

# HELP mainstay_bridge_open_swaps Swaps whose current status is Open.
# TYPE mainstay_bridge_open_swaps gauge
mainstay_bridge_open_swaps $bridge_open_swaps

# HELP mainstay_bridge_outbound_volume_total Cumulative outbound bridge amount in each token's smallest units.
# TYPE mainstay_bridge_outbound_volume_total counter
EOF

    if [[ -n "${CONTRACT_BRIDGE:-}" && -n "${BRIDGE_TOKEN_ADDRESSES:-}" ]]; then
        local -a bridge_tokens=()
        local token volume
        IFS=',' read -r -a bridge_tokens <<< "$BRIDGE_TOKEN_ADDRESSES"
        for token in "${bridge_tokens[@]}"; do
            # Token addresses are alphanumeric Stellar identifiers, safe as labels.
            [[ -z "$token" ]] && continue
            volume="$(invoke_read_number "$CONTRACT_BRIDGE" get_outbound_volume --token "$token")"
            printf 'mainstay_bridge_outbound_volume_total{token="%s"} %s\n' "$token" "$volume" >> "$tmp_file"
        done
    fi

    # --- Marketplace auction metrics ---
    # Skipped when no marketplace is configured so that deployments without
    # auctions do not report misleading zeroed series.
    if [[ -n "${CONTRACT_MARKETPLACE:-}" ]]; then
        local total_auctions
        total_auctions="$(invoke_read_number "$CONTRACT_MARKETPLACE" get_auction_count)"

        # Counts by AuctionStatus discriminant:
        # 0=open, 1=unawarded, 2=awarded, 3=disputed, 4=resolved.
        local open_auctions=0
        local unawarded_auctions=0
        local awarded_auctions=0
        local disputed_auctions=0
        local resolved_auctions=0
        local total_bids=0

        local auction_id status bid_count
        for (( auction_id=1; auction_id<=total_auctions; auction_id++ )); do
            status="$(invoke_read_number "$CONTRACT_MARKETPLACE" get_auction_status --auction_id "$auction_id")"
            case "$status" in
                0) open_auctions=$((open_auctions + 1)) ;;
                1) unawarded_auctions=$((unawarded_auctions + 1)) ;;
                2) awarded_auctions=$((awarded_auctions + 1)) ;;
                3) disputed_auctions=$((disputed_auctions + 1)) ;;
                4) resolved_auctions=$((resolved_auctions + 1)) ;;
            esac
            bid_count="$(invoke_read_number "$CONTRACT_MARKETPLACE" get_bid_count --auction_id "$auction_id")"
            total_bids=$((total_bids + bid_count))
        done

        cat >> "$tmp_file" <<EOF

# HELP mainstay_auctions_total Number of auctions by lifecycle status.
# TYPE mainstay_auctions_total gauge
mainstay_auctions_total{status="open"} $open_auctions
mainstay_auctions_total{status="unawarded"} $unawarded_auctions
mainstay_auctions_total{status="awarded"} $awarded_auctions
mainstay_auctions_total{status="disputed"} $disputed_auctions
mainstay_auctions_total{status="resolved"} $resolved_auctions

# HELP mainstay_auction_bids_total Number of valid bids across all auctions.
# TYPE mainstay_auction_bids_total gauge
mainstay_auction_bids_total $total_bids

# HELP mainstay_auction_disputes_total Number of auction disputes by state (an auction is disputed while it waits for admin resolution and resolved once the admin has ruled).
# TYPE mainstay_auction_disputes_total gauge
mainstay_auction_disputes_total{state="open"} $disputed_auctions
mainstay_auction_disputes_total{state="resolved"} $resolved_auctions
EOF
    fi

    # --- Compute RPC metrics from the log ---
    local rpc_requests=0
    local rpc_errors=0
    local rpc_duration_sum=0
    if [[ -f "$RPC_LOG" ]]; then
        rpc_requests="$(wc -l < "$RPC_LOG")"
        rpc_errors="$(grep -c '^1 ' "$RPC_LOG" || true)"
        rpc_duration_sum="$(awk '{sum += $2} END {print sum+0}' "$RPC_LOG" || true)"
    fi
    MAINSTAY_RPC_REQUESTS_LAST_SCRAPE=$rpc_requests
    MAINSTAY_RPC_ERRORS_LAST_SCRAPE=$rpc_errors
    MAINSTAY_RPC_REQUEST_DURATION_SECONDS_SUM=$rpc_duration_sum

    cat >> "$tmp_file" <<EOF

# HELP mainstay_rpc_requests_last_scrape Number of read-only RPC calls made in the latest collection.
# TYPE mainstay_rpc_requests_last_scrape gauge
mainstay_rpc_requests_last_scrape $rpc_requests

# HELP mainstay_rpc_errors_last_scrape Number of read-only RPC calls that failed in the latest collection.
# TYPE mainstay_rpc_errors_last_scrape gauge
mainstay_rpc_errors_last_scrape $rpc_errors

# HELP mainstay_rpc_request_duration_seconds_sum Total read-only RPC call time in the latest collection (response time = sum / requests).
# TYPE mainstay_rpc_request_duration_seconds_sum gauge
mainstay_rpc_request_duration_seconds_sum $rpc_duration_sum
EOF

    if [[ "$CHAIN_READ_ERRORS" -eq 0 ]]; then
        cat >> "$tmp_file" <<EOF

# HELP mainstay_metrics_collection_seconds Timestamp of last successful metrics collection.
# TYPE mainstay_metrics_collection_seconds gauge
mainstay_metrics_collection_seconds $ts

# HELP mainstay_exporter_up Whether the exporter is healthy (1=up, 0=chain reads failing).
# TYPE mainstay_exporter_up gauge
mainstay_exporter_up 1
EOF
    else
        cat >> "$tmp_file" <<EOF

# HELP mainstay_exporter_up Whether the exporter is healthy (1=up, 0=chain reads failing).
# TYPE mainstay_exporter_up gauge
mainstay_exporter_up 0
EOF
    fi

    mv -f "$tmp_file" "$METRICS_FILE"
}

# ---------------------------------------------------------------------------
# Simple HTTP server using netcat (nc) or Python
# ---------------------------------------------------------------------------

serve_http() {
    local port="$1"
    local bind_addr="$2"

    echo "[$(date -u +%H:%M:%S)] Starting Prometheus metrics exporter on ${bind_addr}:${port}"
    echo "[$(date -u +%H:%M:%S)] Scrape interval: ${SCRAPE_INTERVAL}s"

    init_metrics_dir

    # Initial collection
    collect_metrics

    # Background collection loop
    while true; do
        sleep "$SCRAPE_INTERVAL"
        collect_metrics 2>/dev/null
    done &
    local collector_pid=$!

    # HTTP server loop using socat or a simple Python server
    cleanup() {
        kill "$collector_pid" 2>/dev/null || true
        if [[ -n "$METRICS_DIR" && -d "$METRICS_DIR" ]]; then
            rm -rf -- "$METRICS_DIR"
        fi
    }
    trap cleanup EXIT

    if command -v socat &> /dev/null; then
        echo "[$(date -u +%H:%M:%S)] Using socat for HTTP server"
        socat TCP-LISTEN:"$port",bind="$bind_addr",reuseaddr,fork \
            EXEC:"printf 'HTTP/1.1 200 OK
Content-Type: text/plain

'; cat $METRICS_FILE"
    elif command -v python3 &> /dev/null; then
        echo "[$(date -u +%H:%M:%S)] Using Python for HTTP server"
        python3 -c "
import http.server, os, sys
METRICS_FILE = '$METRICS_FILE'
BIND_ADDRESS = '$bind_addr'
class MetricsHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/metrics' or self.path == '/':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            try:
                with open(METRICS_FILE) as f:
                    self.wfile.write(f.read().encode())
            except FileNotFoundError:
                self.wfile.write(b'# Metrics not yet collected
')
        elif self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'OK
')
        else:
            self.send_response(404)
            self.end_headers()
    def log_message(self, fmt, *args):
        pass
http.server.HTTPServer((BIND_ADDRESS, $port), MetricsHandler).serve_forever()
" 2>/dev/null
    else
        echo "[$(date -u +%H:%M:%S)] Neither socat nor python3 found."
        echo "[$(date -u +%H:%M:%S)] Install one to serve metrics over HTTP."
        echo "[$(date -u +%H:%M:%S)] Metrics file at: $METRICS_FILE"
        echo "[$(date -u +%H:%M:%S)] Press Ctrl+C to stop."
        wait "$collector_pid"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

# Sourcing this file (as the fixture test does) must not start the exporter.
if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    return 0 2>/dev/null || exit 0
fi

PORT="${EXPORTER_PORT}"
BIND_ADDR="${EXPORTER_BIND_ADDRESS}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port)
            if [[ -z "${2:-}" ]]; then
                echo "Option --port requires an integer from 1 to 65535." >&2
                exit 1
            fi
            PORT="$2"
            shift 2
            ;;
        --bind)
            if [[ -z "${2:-}" ]]; then
                echo "Option --bind requires an address." >&2
                exit 1
            fi
            BIND="$2"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [[ ! "$PORT" =~ ^[0-9]+$ ]] || (( PORT < 1 || PORT > 65535 )); then
    echo "Invalid port: ${PORT} (expected an integer from 1 to 65535)." >&2
    exit 1
fi

echo "=== Mainstay Prometheus Exporter ==="
echo "Network: ${STELLAR_NETWORK}"

serve_http "$PORT" "$BIND_ADDR"
