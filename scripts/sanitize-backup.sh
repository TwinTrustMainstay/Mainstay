#!/usr/bin/env bash
# Create a safe, synthetic development dataset from a Mainstay backup.
#
# The source backup is never modified. Production networks are rejected and
# sensitive values are replaced before the output is used by local tooling.
#
# Usage:
#   ./scripts/sanitize-backup.sh <backup-dir> <output-dir>

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 <backup-dir> <output-dir>" >&2
    exit 1
fi

SOURCE_DIR="$1"
OUTPUT_DIR="$2"

[[ -d "$SOURCE_DIR" ]] || { echo "Backup directory not found: $SOURCE_DIR" >&2; exit 1; }
[[ -f "$SOURCE_DIR/contract_ids.md" ]] || {
    echo "Refusing input without contract_ids.md (not a Mainstay backup)." >&2
    exit 1
}
if grep -Eiq 'mainnet|pubnet' "$SOURCE_DIR/contract_ids.md"; then
    echo "Refusing to sanitize a mainnet backup. Export a controlled test fixture instead." >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR/assets" "$OUTPUT_DIR/maintenance"
cp "$SOURCE_DIR/asset_count.json" "$OUTPUT_DIR/asset_count.json"
cp "$SOURCE_DIR/maintenance_summary.json" "$OUTPUT_DIR/maintenance_summary.json" 2>/dev/null || true

shopt -s nullglob
for source in "$SOURCE_DIR"/assets/*.json; do
    id="$(basename "$source")"
    jq --arg id "${id%.json}" '
      .owner = ("GDEV" + ($id | tostring))
      | .serial_number = ("DEV-SERIAL-" + ($id | tostring))
      | .metadata = "Synthetic development asset " + ($id | tostring)
      | .lender = null
      | .loan_id = null
    ' "$source" > "$OUTPUT_DIR/assets/$id"
done

for source in "$SOURCE_DIR"/maintenance/*.json; do
    id="$(basename "$source")"
    case "$id" in
        *_history.json)
            jq 'map(
              .engineer = "GDEV-ENGINEER"
              | .notes = "Synthetic development maintenance record"
            )' "$source" > "$OUTPUT_DIR/maintenance/$id"
            ;;
        *) cp "$source" "$OUTPUT_DIR/maintenance/$id" ;;
    esac
done

cat > "$OUTPUT_DIR/contract_ids.md" <<'EOF'
## Contract IDs

This dataset is synthetic and intended for local or standalone development.
It must not be restored to testnet or mainnet.
EOF

echo "Sanitized development dataset written to $OUTPUT_DIR"
