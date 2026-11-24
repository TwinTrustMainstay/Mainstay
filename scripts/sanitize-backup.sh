#!/usr/bin/env bash
# Create a safe, synthetic development dataset from a Mainstay backup.
#
# The source backup is never modified. Production networks are rejected and
# sensitive values are replaced before the output is used by local tooling.
#
# Synthetic owners are deterministic Stellar account strkeys derived from the
# documented public test seed "mainstay-dev-seed-v1". For each asset the seed
# is combined with the asset id, hashed with SHA-256 into 32 bytes, and encoded
# as an ed25519 public key strkey (version byte 0x30, CRC16-XModem checksum,
# unpadded base32). Every generated owner is validated against ^G[A-Z2-7]{55}$.
#
# Usage:
#   ./scripts/sanitize-backup.sh <backup-dir> <output-dir>

set -euo pipefail

# Documented public test seed. Not a secret; changing it changes all owners.
DEV_SEED="mainstay-dev-seed-v1"

# Encode 32 raw bytes (hex on stdin) as a Stellar ed25519 public key strkey.
strkey_from_hex() {
    local hex="$1"
    local version="30"  # 6 << 3, ed25519 public key version byte
    local payload="${version}${hex}"
    local crc
    crc="$(printf '%s' "$payload" | xxd -r -p | cksum_crc16)"
    printf '%s' "${payload}${crc}" | xxd -r -p | base32 | tr -d '=\n'
}

# CRC16-XModem (polynomial 0x1021, init 0x0000) over stdin; prints 4 hex chars.
cksum_crc16() {
    local crc=0
    local byte i
    while IFS= read -r -n1 byte; do
        printf -v byte '%d' "'$byte"
        crc=$(( crc ^ (byte << 8) ))
        for (( i = 0; i < 8; i++ )); do
            if (( crc & 0x8000 )); then
                crc=$(( ((crc << 1) ^ 0x1021) & 0xFFFF ))
            else
                crc=$(( (crc << 1) & 0xFFFF ))
            fi
        done
    done
    printf '%04x' "$crc"
}

# Derive a deterministic valid Stellar account address for an asset id.
owner_for_id() {
    local id="$1"
    local digest
    digest="$(printf '%s:%s' "$DEV_SEED" "$id" | sha256sum | cut -d' ' -f1)"
    strkey_from_hex "$digest"
}

# Abort unless the value is a valid 56-character Stellar account strkey.
validate_owner() {
    local owner="$1"
    if [[ ! "$owner" =~ ^G[A-Z2-7]{55}$ ]] || [[ ${#owner} -ne 56 ]]; then
        echo "Invalid synthetic owner generated: '$owner'" >&2
        exit 1
    fi
}

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
    owner="$(owner_for_id "${id%.json}")"
    validate_owner "$owner"
    jq --arg id "${id%.json}" --arg owner "$owner" '
      .owner = $owner
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
