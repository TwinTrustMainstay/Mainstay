#!/usr/bin/env bash
# =============================================================================
# Mainstay Restore Script
# =============================================================================
# Restores assets, engineers, and maintenance records from a backup produced
# by scripts/backup.sh.  Supports both local and S3 backup sources.
#
# Usage:
#   ./scripts/restore.sh <backup_path_or_s3_uri> [--dry-run] [--verify-only]
#
#   backup_path_or_s3_uri   - Path to local backup dir or s3:// URI
#   --dry-run               - Show what would be restored without executing
#   --verify-only           - Only validate backup integrity, do not restore
#   --admin-key             - Path to the admin Stellar key (required for writes)
#   --start-asset-id <N>    - Only restore assets with ID >= N
#
# Environment variables:
#   CONTRACT_ASSET_REGISTRY     - Asset Registry contract ID
#   CONTRACT_ENGINEER_REGISTRY  - Engineer Registry contract ID
#   CONTRACT_LIFECYCLE          - Lifecycle contract ID
#   STELLAR_NETWORK             - Stellar network (testnet/mainnet)
#   STELLAR_RPC_URL             - Stellar RPC endpoint
#   AWS_ACCESS_KEY_ID           - AWS access key (for S3 sources)
#   AWS_SECRET_ACCESS_KEY       - AWS secret key (for S3 sources)
#   AWS_DEFAULT_REGION          - AWS region
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true

: "${STELLAR_NETWORK:=testnet}"
: "${AWS_DEFAULT_REGION:=us-east-1}"

DRY_RUN=false
VERIFY_ONLY=false
ADMIN_KEY_PATH=""
START_ASSET_ID=1

usage() {
    cat <<EOF
Usage: $0 <backup_path_or_s3_uri> [options]

Options:
  --dry-run              Preview what would be restored
  --verify-only          Validate backup integrity only (no writes)
  --admin-key <path>     Path to the admin Stellar secret key file
  --start-asset-id <N>   Only restore assets with ID >= N (default: 1)

EOF
    exit 1
}

# Parse arguments
if [[ $# -lt 1 ]]; then
    usage
fi

BACKUP_SOURCE="$1"
shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --verify-only) VERIFY_ONLY=true; shift ;;
        --admin-key) ADMIN_KEY_PATH="$2"; shift 2 ;;
        --start-asset-id) START_ASSET_ID="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() { echo "[$(date -u +%H:%M:%S)] $*"; }
err() { echo "[$(date -u +%H:%M:%S)] ERROR: $*" >&2; }

resolve_backup_source() {
    local src="$1"
    local work_dir

    if [[ "$src" == s3://* ]]; then
        log "Downloading backup from S3: $src"
        work_dir="$(mktemp -d)"
        aws s3 cp "$src" "$work_dir" --recursive
        echo "$work_dir"
    else
        if [[ ! -d "$src" ]]; then
            err "Backup directory not found: $src"
            exit 1
        fi
        echo "$src"
    fi
}

verify_integrity() {
    local dir="$1"
    log "Verifying backup integrity..."

    # Check for checksums file
    if [[ -f "$dir/checksums.sha256" ]]; then
        log "Verifying SHA-256 checksums..."
        pushd "$dir" > /dev/null
        if sha256sum -c checksums.sha256 --quiet 2>/dev/null; then
            log "  All checksums verified OK"
        else
            err "  Checksum verification FAILED"
            return 1
        fi
        popd > /dev/null
    else
        log "  No checksums file found; skipping hash verification"
    fi

    # Check for required files
    local required=("manifest.json" "asset_count.json")
    local missing=()
    for f in "${required[@]}"; do
        if [[ ! -f "$dir/$f" ]]; then
            missing+=("$f")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        err "Missing required files: ${missing[*]}"
        return 1
    fi

    # Verify manifest
    if ! jq -e . "$dir/manifest.json" > /dev/null 2>&1; then
        err "manifest.json is not valid JSON"
        return 1
    fi

    log "Backup integrity check passed."
    return 0
}

invoke_write() {
    local contract_id="$1"
    local func="$2"
    shift 2
    local source_flag=""
    if [[ -n "$ADMIN_KEY_PATH" ]]; then
        # Import the key temporarily if using a key file
        stellar keys import admin --secret-key "$(cat "$ADMIN_KEY_PATH")" --network "$STELLAR_NETWORK" 2>/dev/null || true
        source_flag="--source admin"
    fi
    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY RUN] stellar contract invoke --id $contract_id --network $STELLAR_NETWORK -- $func $*"
        return 0
    fi
    stellar contract invoke \
        --id "$contract_id" \
        --network "$STELLAR_NETWORK" \
        $source_flag \
        -- "$func" "$@"
}

# ---------------------------------------------------------------------------
# Restore Functions
# ---------------------------------------------------------------------------

restore_assets() {
    local dir="$1"
    local assets_dir="$dir/assets"

    if [[ ! -d "$assets_dir" ]]; then
        log "No assets directory found; skipping asset restore."
        return 0
    fi

    log "Restoring assets..."

    local restored=0
    local skipped=0
    local failed=0

    for asset_file in "$assets_dir"/*.json; do
        [[ -f "$asset_file" ]] || continue

        local asset_id
        asset_id="$(basename "$asset_file" .json)"

        if [[ "$asset_id" -lt "$START_ASSET_ID" ]]; then
            skipped=$((skipped + 1))
            continue
        fi

        # Extract asset fields from JSON
        local asset_type metadata serial_number owner
        asset_type="$(jq -r '.asset_type // empty' "$asset_file")"
        metadata="$(jq -r '.metadata // empty' "$asset_file")"
        serial_number="$(jq -r '.serial_number // empty' "$asset_file")"
        owner="$(jq -r '.owner // empty' "$asset_file")"

        if [[ -z "$asset_type" || -z "$metadata" || -z "$serial_number" || -z "$owner" ]]; then
            log "  WARNING: Asset $asset_id has incomplete data; skipping"
            failed=$((failed + 1))
            continue
        fi

        if invoke_write "$CONTRACT_ASSET_REGISTRY" register_asset \
            --asset_type "$asset_type" \
            --metadata "$metadata" \
            --serial_number "$serial_number" \
            --owner "$owner" 2>&1; then
            restored=$((restored + 1))
        else
            log "  WARNING: Failed to restore asset $asset_id (may already exist)"
            failed=$((failed + 1))
        fi
    done

    log "Asset restore complete: $restored restored, $skipped skipped, $failed failed"
}

restore_maintenance() {
    local dir="$1"
    local maint_dir="$dir/maintenance"

    if [[ ! -d "$maint_dir" ]]; then
        log "No maintenance directory found; skipping."
        return 0
    fi

    log "Restoring maintenance records..."
    log "NOTE: Full maintenance restore requires engineer re-registration."
    log "      Only assets and metadata are restored in this version."
    log "      Maintenance history files are in: $maint_dir"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    log "=== Mainstay Restore ==="
    log "Network: ${STELLAR_NETWORK}"
    log "Backup source: ${BACKUP_SOURCE}"
    log "Dry run: ${DRY_RUN}"
    log "Verify only: ${VERIFY_ONLY}"

    # Resolve backup source (download from S3 if needed)
    local work_dir
    work_dir="$(resolve_backup_source "$BACKUP_SOURCE")"

    # Always verify integrity first
    if ! verify_integrity "$work_dir"; then
        err "Backup integrity check failed. Aborting."
        exit 1
    fi

    if [[ "$VERIFY_ONLY" == "true" ]]; then
        log "Verification complete. No state was modified."
        exit 0
    fi

    if [[ "$DRY_RUN" != "true" && -z "$ADMIN_KEY_PATH" ]]; then
        err "--admin-key is required for restoration (or use --dry-run to preview)."
        exit 1
    fi

    log "Starting restoration..."

    # 1. Restore assets from backup
    restore_assets "$work_dir"

    # 2. Restore maintenance records (placeholder)
    restore_maintenance "$work_dir"

    log "=== Restore complete ==="
}

main "$@"
