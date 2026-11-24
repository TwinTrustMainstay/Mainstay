#!/usr/bin/env bash
# Fixture check: one asset view with two maintenance records must contribute 2
# to mainstay_maintenance_records_total, not 1 (the asset count).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=prometheus-exporter.sh
source "${SCRIPT_DIR}/prometheus-exporter.sh"

raw="$(cat "${SCRIPT_DIR}/fixtures/history-two-records.txt")"
got="$(history_len_from_view "$raw")"

if [[ "$got" != "2" ]]; then
    echo "expected mainstay_maintenance_records_total contribution 2, got ${got}" >&2
    exit 1
fi

echo "history length fixture ok: ${got}"
