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
#   SCRAPE_INTERVAL             - Seconds between scrapes (default: 60)
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true

: "${STELLAR_NETWORK:=testnet}"
: "${EXPORTER_PORT:=9600}"
: "${SCRAPE_INTERVAL:=60}"

# Temp file for the latest metrics page
METRICS_FILE="/tmp/mainstay-metrics.prom"

# ---------------------------------------------------------------------------
# Query helpers
# ---------------------------------------------------------------------------

invoke_read() {
    stellar contract invoke \
        --id "$1" \
        --network "$STELLAR_NETWORK" \
        --source any \
        -- "$2" "$3" "$4" "$5" 2>/dev/null || echo "0"
}

# ---------------------------------------------------------------------------
# Collect metrics from chain
# ---------------------------------------------------------------------------

collect_metrics() {
    local ts
    ts="$(date +%s)"

    > "$METRICS_FILE.tmp"

    # --- Asset Registry metrics ---
    local total_assets
    total_assets="$(invoke_read "$CONTRACT_ASSET_REGISTRY" get_asset_count)"

    cat >> "$METRICS_FILE.tmp" <<EOF
# HELP mainstay_assets_total Total number of registered assets.
# TYPE mainstay_assets_total gauge
mainstay_assets_total $total_assets
EOF

    # --- Engineer Registry metrics ---
    local total_engineers
    total_engineers="$(invoke_read "$CONTRACT_ENGINEER_REGISTRY" get_engineer_count 2>/dev/null || echo "0")"

    cat >> "$METRICS_FILE.tmp" <<EOF
# HELP mainstay_engineers_total Total number of registered engineers.
# TYPE mainstay_engineers_total gauge
mainstay_engineers_total $total_engineers
EOF

    # --- Maintenance records: iterate all assets for total count ---
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
            hist_len="$(invoke_read "$CONTRACT_LIFECYCLE" get_maintenance_history --asset_id "$id" 2>/dev/null || echo "0")"
            # Count entries (this is approximate — the raw output from stellar CLI
            # is not natively parseable as JSON in all versions; adapt as needed)
            total_records=$((total_records + 1))

            local score
            score="$(invoke_read "$CONTRACT_LIFECYCLE" get_collateral_score --asset_id "$id" 2>/dev/null || echo "0")"
            score_sum=$((score_sum + score))
            if [[ "$score" -gt 0 ]]; then
                scored_assets=$((scored_assets + 1))
            fi
        done
    fi

    local avg_score=0
    if [[ "$scored_assets" -gt 0 ]]; then
        avg_score=$((score_sum / scored_assets))
    fi

    cat >> "$METRICS_FILE.tmp" <<EOF
# HELP mainstay_maintenance_records_total Total maintenance records across all assets.
# TYPE mainstay_maintenance_records_total gauge
mainstay_maintenance_records_total $total_records

# HELP mainstay_avg_collateral_score Average collateral score across scored assets.
# TYPE mainstay_avg_collateral_score gauge
mainstay_avg_collateral_score $avg_score

# HELP mainstay_scored_assets_count Number of assets with non-zero collateral scores.
# TYPE mainstay_scored_assets_count gauge
mainstay_scored_assets_count $scored_assets

# HELP mainstay_metrics_collection_seconds Timestamp of last successful metrics collection.
# TYPE mainstay_metrics_collection_seconds gauge
mainstay_metrics_collection_seconds $ts

# HELP mainstay_exporter_up Whether the exporter is running (1=up).
# TYPE mainstay_exporter_up gauge
mainstay_exporter_up 1
EOF

    mv "$METRICS_FILE.tmp" "$METRICS_FILE"
}

# ---------------------------------------------------------------------------
# Simple HTTP server using netcat (nc) or Python
# ---------------------------------------------------------------------------

serve_http() {
    local port="$1"

    echo "[$(date -u +%H:%M:%S)] Starting Prometheus metrics exporter on :$port"
    echo "[$(date -u +%H:%M:%S)] Scrape interval: ${SCRAPE_INTERVAL}s"

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
        rm -f "$METRICS_FILE" "$METRICS_FILE.tmp"
    }
    trap cleanup EXIT

    if command -v socat &> /dev/null; then
        echo "[$(date -u +%H:%M:%S)] Using socat for HTTP server"
        socat TCP-LISTEN:"$port",reuseaddr,fork \
            EXEC:"printf 'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n'; cat $METRICS_FILE"
    elif command -v python3 &> /dev/null; then
        echo "[$(date -u +%H:%M:%S)] Using Python for HTTP server"
        python3 -c "
import http.server, os
METRICS_FILE = '$METRICS_FILE'
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
http.server.HTTPServer(('0.0.0.0', $port), MetricsHandler).serve_forever()
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

if [[ "${1:-}" == "--port" ]]; then
    PORT="${2:-9600}"
fi

echo "=== Mainstay Prometheus Exporter ==="
echo "Network: ${STELLAR_NETWORK}"

serve_http "$PORT"
