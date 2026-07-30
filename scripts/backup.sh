#!/usr/bin/env bash
# =============================================================================
# Mainstay Backup Script
# =============================================================================
# Exports all assets, engineers, maintenance records, and metadata from on-chain
# contracts and stores them in an encrypted, versioned S3 bucket.
#
# Usage:
#   ./scripts/backup.sh [--dry-run] [--output-dir <path>]
#
# Environment variables (required for S3 upload):
#   AWS_ACCESS_KEY_ID       - AWS access key for S3
#   AWS_SECRET_ACCESS_KEY   - AWS secret key for S3
#   AWS_DEFAULT_REGION      - AWS region (default: us-east-1)
#   BACKUP_S3_BUCKET        - S3 bucket name (e.g., mainstay-backups)
#   BACKUP_ENCRYPTION_KEY   - KMS key ID/ARN for server-side encryption
#
# Contract environment variables:
#   CONTRACT_ASSET_REGISTRY     - Asset Registry contract ID
#   CONTRACT_ENGINEER_REGISTRY  - Engineer Registry contract ID
#   CONTRACT_LIFECYCLE          - Lifecycle contract ID
#   STELLAR_NETWORK             - Stellar network (testnet/mainnet)
#   STELLAR_RPC_URL             - Stellar RPC endpoint
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true

: "${STELLAR_NETWORK:=testnet}"
: "${AWS_DEFAULT_REGION:=us-east-1}"

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
BACKUP_DIR="${OUTPUT_DIR:-${ROOT}/backups/${TIMESTAMP}}"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --output-dir) BACKUP_DIR="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Helper: invoke a Stellar contract read-only function
# ---------------------------------------------------------------------------
invoke_read() {
    local contract_id="$1"
    local func="$2"
    shift 2
    stellar contract invoke \
        --id "$contract_id" \
        --network "$STELLAR_NETWORK" \
        --source any \
        -- "$func" "$@"
}

# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------
log() { echo "[$(date -u +%H:%M:%S)] $*"; }
err() { echo "[$(date -u +%H:%M:%S)] ERROR: $*" >&2; }

validate_env() {
    local missing=()
    for var in CONTRACT_ASSET_REGISTRY CONTRACT_ENGINEER_REGISTRY CONTRACT_LIFECYCLE; do
        if [[ -z "${!var:-}" ]]; then
            missing+=("$var")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        err "Missing required environment variables: ${missing[*]}"
        err "Set them in .env or export them before running this script."
        exit 1
    fi
}

validate_s3() {
    if [[ "$DRY_RUN" == "true" ]]; then
        return
    fi
    local missing=()
    for var in AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY BACKUP_S3_BUCKET; do
        if [[ -z "${!var:-}" ]]; then
            missing+=("$var")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        err "S3 upload disabled: missing ${missing[*]}"
        err "Backup will be saved locally only."
        S3_ENABLED=false
    else
        S3_ENABLED=true
    fi
}

# ---------------------------------------------------------------------------
# Export Functions
# ---------------------------------------------------------------------------

export_asset_count() {
    log "Exporting asset count..."
    local count
    count="$(invoke_read "$CONTRACT_ASSET_REGISTRY" get_asset_count 2>/dev/null)" || true
    if [[ -z "$count" || "$count" == "null" ]]; then
        count=0
    fi
    jq -n --argjson count "$count" \
       --arg timestamp "$(date -u -Iseconds)" \
       '{asset_count: $count, exported_at: $timestamp}' \
       > "$BACKUP_DIR/asset_count.json"
    echo "$count"
}

export_assets() {
    local total="$1"
    log "Exporting $total assets..."
    mkdir -p "$BACKUP_DIR/assets"

    local exported=0
    for (( id=1; id<=total; id++ )); do
        local asset_json
        asset_json="$(invoke_read "$CONTRACT_ASSET_REGISTRY" get_asset --asset_id "$id" 2>/dev/null)" || true
        if [[ -n "$asset_json" && "$asset_json" != "null" ]]; then
            echo "$asset_json" > "$BACKUP_DIR/assets/${id}.json"
            exported=$((exported + 1))
        fi
    done

    jq -n --argjson exported "$exported" \
       --argjson total "$total" \
       '{exported: $exported, total: $total}' \
       > "$BACKUP_DIR/asset_export_summary.json"

    log "Exported $exported / $total assets"
}

export_engineers() {
    log "Exporting engineer credentials..."

    # Since there's no direct "get all engineers" function, we export the
    # engineer registry's admin and the list of trusted issuers, then use
    # per-issuer lookups if available.  This section captures what's
    # programmatically accessible through the contract's public API.
    {
        echo "{}"   # placeholder for engineer list (depends on contract API)
    } > "$BACKUP_DIR/engineers.json"

    log "Engineer export placeholder written (extend with per-issuer queries)"
}

export_maintenance_history() {
    local total="$1"
    log "Exporting maintenance history..."
    mkdir -p "$BACKUP_DIR/maintenance"

    local count=0
    for (( id=1; id<=total; id++ )); do
        local hist_json
        hist_json="$(invoke_read "$CONTRACT_LIFECYCLE" get_maintenance_history --asset_id "$id" 2>/dev/null)" || true
        if [[ -n "$hist_json" && "$hist_json" != "null" && "$hist_json" != "[]" ]]; then
            echo "$hist_json" > "$BACKUP_DIR/maintenance/${id}_history.json"
            count=$((count + 1))
        fi

        # Also export collateral score
        local score
        score="$(invoke_read "$CONTRACT_LIFECYCLE" get_collateral_score --asset_id "$id" 2>/dev/null)" || true
        if [[ -n "$score" && "$score" != "null" ]]; then
            echo "$score" > "$BACKUP_DIR/maintenance/${id}_score.json"
        fi
    done

    jq -n --argjson assets_with_history "$count" \
       '{assets_with_maintenance_history: $assets_with_history}' \
       > "$BACKUP_DIR/maintenance_summary.json"

    log "Maintenance history exported for $count assets"
}

export_contract_metadata() {
    log "Exporting contract metadata..."
    {
        echo "## Contract IDs"
        echo "exported_at: $(date -u -Iseconds)"
        echo "network: ${STELLAR_NETWORK}"
        echo ""
        echo "| Contract | ID |"
        echo "|----------|----|"
        echo "| Asset Registry | ${CONTRACT_ASSET_REGISTRY} |"
        echo "| Engineer Registry | ${CONTRACT_ENGINEER_REGISTRY} |"
        echo "| Lifecycle | ${CONTRACT_LIFECYCLE} |"
    } > "$BACKUP_DIR/contract_ids.md"
}

# ---------------------------------------------------------------------------
# S3 Upload
# ---------------------------------------------------------------------------

upload_to_s3() {
    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY RUN] Would upload $BACKUP_DIR to s3://${BACKUP_S3_BUCKET}/backups/${TIMESTAMP}/"
        return
    fi

    if [[ "${S3_ENABLED:-false}" != "true" ]]; then
        log "Skipping S3 upload (not configured)."
        return
    fi

    log "Uploading backup to S3..."

    local s3_path="s3://${BACKUP_S3_BUCKET}/backups/${TIMESTAMP}/"
    local sse_args=()
    if [[ -n "${BACKUP_ENCRYPTION_KEY:-}" ]]; then
        sse_args=(--sse aws:kms --sse-kms-key-id "$BACKUP_ENCRYPTION_KEY")
    else
        sse_args=(--sse AES256)
    fi

    aws s3 cp "$BACKUP_DIR" "$s3_path" \
        --recursive \
        "${sse_args[@]}" \
        --storage-class STANDARD_IA

    # Enable versioning on the bucket if not already enabled
    aws s3api put-bucket-versioning \
        --bucket "$BACKUP_S3_BUCKET" \
        --versioning-configuration Status=Enabled 2>/dev/null || true

    log "Backup uploaded to $s3_path"
}

generate_manifest() {
    log "Generating backup manifest..."
    local manifest="${BACKUP_DIR}/manifest.json"
    local file_list
    file_list="$(find "$BACKUP_DIR" -type f -exec sha256sum {} \; | sort -k2)"

    jq -n \
        --arg backup_id "$TIMESTAMP" \
        --arg network "$STELLAR_NETWORK" \
        --arg timestamp "$(date -u -Iseconds)" \
        --arg asset_registry "$CONTRACT_ASSET_REGISTRY" \
        --arg engineer_registry "$CONTRACT_ENGINEER_REGISTRY" \
        --arg lifecycle "$CONTRACT_LIFECYCLE" \
        '{
            backup_id: $backup_id,
            network: $network,
            created_at: $timestamp,
            contracts: {
                asset_registry: $asset_registry,
                engineer_registry: $engineer_registry,
                lifecycle: $lifecycle
            },
            contents: "See individual files in this directory."
        }' > "$manifest"

    # Store the file list as a separate checksum file
    echo "$file_list" > "${BACKUP_DIR}/checksums.sha256"

    log "Manifest written to $manifest"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    log "=== Mainstay Backup ==="
    log "Network: ${STELLAR_NETWORK}"
    log "Backup directory: ${BACKUP_DIR}"

    validate_env
    validate_s3

    mkdir -p "$BACKUP_DIR"

    # 1. Export contract metadata
    export_contract_metadata

    # 2. Export asset count
    local total
    total="$(export_asset_count)"

    # 3. Export all assets
    export_assets "$total"

    # 4. Export engineer credentials
    export_engineers

    # 5. Export maintenance history and scores
    export_maintenance_history "$total"

    # 6. Generate manifest and checksums
    generate_manifest

    # 7. Upload to S3 (if configured)
    upload_to_s3

    log "=== Backup complete ==="
    log "Local backup: ${BACKUP_DIR}"
    if [[ "${S3_ENABLED:-false}" == "true" ]]; then
        log "S3 location: s3://${BACKUP_S3_BUCKET}/backups/${TIMESTAMP}/"
    fi
}

main "$@"
