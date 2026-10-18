#!/usr/bin/env bash
# =============================================================================
# Mainstay Restore Script
# =============================================================================
# Restores assets and maintenance records from a backup produced by
# scripts/backup.sh.  Supports both local and S3 backup sources.
#
# NOTE: Engineer restoration is currently PARTIAL.  If engineers.json is
# empty or missing, engineer credentials and issuer relationships are NOT
# recovered; the script reports this explicitly as partial recovery.
=======
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

verify_manifest_signature() {
    local dir="$1"
    local manifest="$dir/manifest.json"

    # Prefer minisign if a signature file is present.
    if [[ -f "$manifest.minisig" ]]; then
        if ! command -v minisign > /dev/null 2>&1; then
            err "  manifest.minisig present but minisign is not installed"
            return 1
        fi
        local pubkey="${MINISIGN_PUBKEY:-}"
        if [[ -z "$pubkey" ]]; then
            err "  MINISIGN_PUBKEY must be set to verify manifest.minisig"
            return 1
        fi
        if minisign -Vm "$manifest" -x "$manifest.minisig" -P "$pubkey" > /dev/null 2>&1; then
            log "  Manifest minisign signature verified OK"
            return 0
        fi
        err "  Manifest minisign signature verification FAILED"
        return 1
    fi

    # Fall back to a KMS signature.
    if [[ -f "$manifest.kms.sig" ]]; then
        if ! command -v aws > /dev/null 2>&1; then
            err "  manifest.kms.sig present but aws CLI is not available"
            return 1
        fi
        local key_id="${MANIFEST_KMS_KEY_ID:-}"
        if [[ -z "$key_id" ]]; then
            err "  MANIFEST_KMS_KEY_ID must be set to verify manifest.kms.sig"
            return 1
        fi
        local sig_b64
        sig_b64="$(cat "$manifest.kms.sig")"
        local verify_out
        if ! verify_out="$(aws kms verify \
                --key-id "$key_id" \
                --message "fileb://$manifest" \
                --message-type RAW \
                --signature "$sig_b64" \
                --signing-algorithm RSASSA_PKCS1_V1_5_SHA_256 \
                --output json 2>/dev/null)"; then
            err "  Manifest KMS signature verification FAILED"
            return 1
        fi
        if [[ "$(echo "$verify_out" | jq -r '.SignatureValid // false')" == "true" ]]; then
            log "  Manifest KMS signature verified OK"
            return 0
        fi
        err "  Manifest KMS signature reported invalid"
        return 1
    fi

    err "  No manifest signature found (expected manifest.minisig or manifest.kms.sig)"
    return 1
}

verify_manifest_coverage() {
    local dir="$1"
    local manifest="$dir/manifest.json"

    # The manifest must list every file in the backup except the checksums
    # file and the signature files themselves.
    local expected actual
    expected="$(jq -r '.file_list[]?' "$manifest" 2>/dev/null | sort -u)"
    if [[ -z "$expected" ]]; then
        err "  manifest.json has no file_list entries"
        return 1
    fi

    actual="$(cd "$dir" && find . -type f \
        ! -name 'checksums.sha256' \
        ! -name '*.minisig' \
        ! -name '*.kms.sig' \
        -printf '%P\n' | sort -u)"

    local uncovered
    uncovered="$(comm -13 <(echo "$expected") <(echo "$actual"))"
    if [[ -n "$uncovered" ]]; then
        err "  Files present but not covered by manifest:"
        echo "$uncovered" | sed 's/^/    /' >&2
        return 1
    fi

    local absent
    absent="$(comm -23 <(echo "$expected") <(echo "$actual"))"
    if [[ -n "$absent" ]]; then
        err "  Files listed in manifest but missing from backup:"
        echo "$absent" | sed 's/^/    /' >&2
        return 1
    fi

    log "  Manifest covers all backup files"
    return 0
}

verify_integrity() {
    local dir="$1"
    log "Verifying backup integrity..."

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

    # Verify manifest is valid JSON
    if ! jq -e . "$dir/manifest.json" > /dev/null 2>&1; then
        err "manifest.json is not valid JSON"
        return 1
    fi

    # Verify the manifest signature before trusting its contents
    if ! verify_manifest_signature "$dir"; then
        err "Manifest signature verification failed"
        return 1
    fi

    # Confirm the manifest covers every file in the backup
    if ! verify_manifest_coverage "$dir"; then
        err "Manifest does not cover all backup files"
        return 1
    fi

    # Check for checksums file
    if [[ -f "$dir/checksums.sha256" ]]; then
        log "Verifying SHA-256 checksums..."
        pushd "$dir" > /dev/null
        if sha256sum -c checksums.sha256 --quiet 2>/dev/null; then
            log "  All checksums verified OK"
        else
            err "  Checksum verification FAILED"
            popd > /dev/null
            return 1
        fi
        popd > /dev/null
    else
        err "No checksums file found; cannot verify file integrity"
        return 1
    fi

    log "Backup integrity check passed."
    return 0
}

report_backup_failures() {
    local dir="$1"
    local failures="$dir/failures.json"

    if [[ ! -f "$failures" ]]; then
        return 0
    fi

    local count
    count="$(jq -r 'if type == "array" then length else 0 end' "$failures" 2>/dev/null || echo 0)"
    if [[ "$count" -gt 0 ]]; then
        err "WARNING: Backup is PARTIAL - $count read failure(s) recorded in failures.json:"
        jq -r '.[] | "    - \(.kind // "unknown") id=\(.id // "?") : \(.error // "unknown error")"' \
            "$failures" 2>/dev/null >&2 || cat "$failures" >&2
        return 1
    fi
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

    # Surface any read failures recorded during backup
    if ! report_backup_failures "$work_dir"; then
        err "Restoring from a partial backup; some records may be missing."
    fi

    # Detect an empty engineers.json and report partial recovery
    local engineers_file="$work_dir/engineers.json"
    if [[ ! -f "$engineers_file" ]] || \
       [[ "$(jq -r 'if type == "object" then (keys | length) elif type == "array" then length else 0 end' "$engineers_file" 2>/dev/null || echo 0)" -eq 0 ]]; then
        err "WARNING: engineers.json is empty or missing."
        err "         Engineer credentials and issuer relationships will NOT be restored."
        err "         This restore is a PARTIAL recovery."
    fi

    log "Starting restoration..."

    # 1. Restore assets from backup
    restore_assets "$work_dir"

    # 2. Restore maintenance records (placeholder)
    restore_maintenance "$work_dir"

    log "=== Restore complete ==="
}

main "$@"
