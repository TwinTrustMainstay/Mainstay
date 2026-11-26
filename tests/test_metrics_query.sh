#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
QUERY_SCRIPT="$SCRIPT_DIR/scripts/monitoring/metrics-query.sh"

echo "Testing metrics-query.sh..."

# 1. Test --help
"$QUERY_SCRIPT" --help > /dev/null

# 2. Test --dry-run instant query
output=$("$QUERY_SCRIPT" --dry-run 'mainstay_assets_total')
if ! [[ "$output" =~ "mainstay_assets_total" ]]; then
  echo "FAIL: dry run output unexpected: $output"
  exit 1
fi

# 3. Test --dry-run range query
output_range=$("$QUERY_SCRIPT" --dry-run --range --start 1000 --end 2000 --step 30s 'mainstay_assets_total')
if ! [[ "$output_range" =~ "query_range" ]]; then
  echo "FAIL: dry run range output unexpected: $output_range"
  exit 1
fi

# 4. Test missing query exits 1
if "$QUERY_SCRIPT" 2>/dev/null; then
  echo "FAIL: expected error on missing query"
  exit 1
fi

# 5. Test non-mainstay metric rejected
if "$QUERY_SCRIPT" --dry-run 'other_metric' 2>/dev/null; then
  echo "FAIL: expected error on non-mainstay metric"
  exit 1
fi

echo "All metrics-query.sh tests passed."
