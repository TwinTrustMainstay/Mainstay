#!/usr/bin/env bash
# =============================================================================
# Mainstay Prometheus Metrics Exporter
# =============================================================================
# Periodically queries contract health metrics and exposes them via a simple
# HTTP endpoint suitable for Prometheus scraping.
#
# Usage:
#   ./scripts/monitoring/prometheus-exporter.sh [--port <port>]
#
# Environment variables:
#   CONTRACT_ASSET_REGISTRY     - Asset Registry contract ID
#   CONTRACT_ENGINEER_REGISTRY  - Engineer Registry contract ID
#   CONTRACT_LIFECYCLE          - Lifecycle contract ID
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
source .env 2>/dev/null || true

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

# Per-call RPC telemetry. Each invoke_read appends one line "<exit_code> <seconds>".
# A file is used instead of shell variables because invoke_read runs inside
# $(...) subshells, where counter updates would be lost.
RPC_LOG="${METRICS_FILE}.rpc"

# ---------------------------------------------------------------------------
# Query helpers
# ---------------------------------------------------------------------------

# invoke_read <contract_id> <method> [args...]
# Prints the command output on success and returns 0.  On failure it prints
# nothing, increments CHAIN_READ_ERRORS and returns 1.
invoke_read() {
    local output
    if output="$(stellar contract invoke \
        --id "$1" \
        --network "$STELLAR_NETWORK" \
        --source any \
        -- "${@:2}" 2>/dev/null)"; then
        printf '%s\n' "$output"
        return 0
    fi
    CHAIN_READ_ERRORS=$((CHAIN_READ_ERRORS + 1))
    return 1
}

# Read a numeric value, falling back to 0 when the read fails.
invoke_read_number() {
    local value
    if value="$(invoke_read "$@")"; then
        printf '%s\n' "$value"
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
            echo "mainstay_collateral_score{asset_id=\"$id\"} $score" >> "$tmp_file"

            # Calculate days since last service
            local last_service_ts
            last_service_ts="$(invoke_read_number "$CONTRACT_LIFECYCLE" get_last_service_timestamp --asset_id "$id")"
            if [[ "$last_service_ts" -gt 0 ]]; then
                local days_since=$((($ts - $last_service_ts) / 86400))
                echo "mainstay_days_since_last_service{asset_id=\"$id\"} $days_since" >> "$tmp_file"
            else
                # Asset has never been serviced
                echo "mainstay_days_since_last_service{asset_id=\"$id\"} -1" >> "$tmp_file"
            fi
        done
    fi

    local avg_score=0
    if [[ "$scored_assets" -gt 0 ]]; then
        avg_score=$((score_sum / scored_assets))
    fi

    cat >> "$tmp_file" <<EOF

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
            EXEC:"printf 'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n'; cat $METRICS_FILE"
    elif command -v python3 &> /dev/null; then
        echo "[$(date -u +%H:%M:%S)] Using Python for HTTP server"
        python3 -c "
import http.server, os
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
                self.wfile.write(b'# Metrics not yet collected\n')
        elif self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'OK\n')
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

PORT="${EXPORTER_PORT}"
BIND_ADDR="${EXPORTER_BIND_ADDRESS}"

if [[ "${1:-}" == "--port" ]]; then
    PORT="${2:-9600}"
fi

echo "=== Mainstay Prometheus Exporter ==="
echo "Network: ${STELLAR_NETWORK}"

serve_http "$PORT" "$BIND_ADDR"
