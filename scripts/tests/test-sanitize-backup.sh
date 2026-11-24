#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SOURCE="$SCRIPT_DIR/fixtures/sensitive-backup"
WORK="$(mktemp -d)"
OUTPUT="$WORK/output"
trap 'rm -rf "$WORK"' EXIT

"$ROOT/scripts/sanitize-backup.sh" "$SOURCE" "$OUTPUT" > /dev/null

for secret in \
  ASSET-CONTRACT-ID-SECRET \
  ENGINEER-CONTRACT-ID-SECRET \
  ORIGINAL-OWNER-SECRET \
  PRODUCTION-SERIAL-SECRET \
  CONFIDENTIAL_PRODUCTION_LOCATION_SECRET \
  PRODUCTION-LENDER-SECRET \
  PRODUCTION-LOAN-ID-SECRET \
  PRODUCTION-ENGINEER-SECRET \
  CONFIDENTIAL_MAINTENANCE_NOTES_SECRET; do
  if grep -R -Fq "$secret" "$OUTPUT"; then
    echo "Sensitive fixture value leaked: $secret" >&2
    exit 1
  fi
done

jq -e --arg original_owner "ORIGINAL-OWNER-SECRET" '
  .owner != $original_owner
  and (.owner | test("^G[A-Z2-7]{55}$"))
  and .serial_number == "DEV-SERIAL-1"
  and .metadata == "Synthetic development asset 1"
  and .lender == null
  and .loan_id == null
  and .asset_type == "generator"
' "$OUTPUT/assets/1.json" > /dev/null

jq -e '
  length == 1
  and .[0].engineer == "GDEV-ENGINEER"
  and .[0].task_type == "inspection"
  and .[0].notes == "Synthetic development maintenance record"
' "$OUTPUT/maintenance/1_history.json" > /dev/null

jq -e '.score == 72' "$OUTPUT/maintenance/1_score.json" > /dev/null
grep -q "synthetic and intended for local or standalone development" "$OUTPUT/contract_ids.md"

echo "Backup sanitizer test passed."
