#!/usr/bin/env bash
# =============================================================================
# Mainstay Metrics Query API
# =============================================================================
# Thin wrapper over the Prometheus HTTP query API so operators and scripts can
# read Mainstay metrics without hand-writing curl calls.
#
# Usage:
#   Instant query:  ./scripts/monitoring/metrics-query.sh 'mainstay_assets_total'
#   Range query:    ./scripts/monitoring/metrics-query.sh --range \
#                     --start 1767225600 --end 1767229200 --step 60s 'mainstay:rpc_error_ratio'
#                   (--start/--end are Unix seconds or RFC 3339; default is the last hour)
#   Show URL only:  ./scripts/monitoring/metrics-query.sh --dry-run 'mainstay_exporter_up'
#
# Environment variables:
#   PROMETHEUS_URL  - Prometheus base URL (default: http://localhost:9090)
#
# Output is the raw JSON response from Prometheus, so it can be piped to jq.
# =============================================================================

set -euo pipefail

: "${PROMETHEUS_URL:=http://localhost:9090}"

MODE="instant"
START=""   # default: one hour ago (Unix seconds)
END=""     # default: now (Unix seconds)
STEP="60s"
DRY_RUN=false
QUERY=""

usage() {
    sed -n '3,15p' "$0" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --range)   MODE="range"; shift ;;
        --start)   START="$2"; shift 2 ;;
        --end)     END="$2"; shift 2 ;;
        --step)    STEP="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        -h|--help) usage; exit 0 ;;
        -*)        echo "Unknown option: $1" >&2; exit 1 ;;
        *)         QUERY="$1"; shift ;;
    esac
done

if [[ -z "$QUERY" ]]; then
    echo "ERROR: a PromQL expression is required" >&2
    usage >&2
    exit 1
fi

# Only the Mainstay namespace and its recording rules (mainstay:...) may be queried.
# This keeps the wrapper from becoming a general-purpose Prometheus proxy.
if ! [[ "$QUERY" == mainstay* ]]; then
    echo "ERROR: query must reference a mainstay_* metric or mainstay:* rule" >&2
    exit 1
fi

NOW="$(date +%s)"
: "${END:=$NOW}"
: "${START:=$((NOW - 3600))}"

if [[ "$MODE" == "instant" ]]; then
    ENDPOINT="$PROMETHEUS_URL/api/v1/query"
    PARAMS=(--data-urlencode "query=$QUERY")
else
    ENDPOINT="$PROMETHEUS_URL/api/v1/query_range"
    PARAMS=(
        --data-urlencode "query=$QUERY"
        --data-urlencode "start=$START"
        --data-urlencode "end=$END"
        --data-urlencode "step=$STEP"
    )
fi

if [[ "$DRY_RUN" == true ]]; then
    echo "$ENDPOINT"
    printf '  %s\n' "${PARAMS[@]}"
    exit 0
fi

curl -sS -G "${PARAMS[@]}" "$ENDPOINT"
