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
# Helper: retry a command with exponential backoff
# ---------------------------------------------------------------------------
# Usage: retry_with_backoff <max_attempts> <command...>
# Returns the command's exit status on success, non-zero after all attempts.
retry_with_backoff() {
    local max_attempts="$1"
    shift
    local attempt=1
    local delay=1
    local rc=0
    while (( attempt <= max_attempts )); do
        if "$@"; then
            return 0
        fi
        rc=$?
        if (( attempt < max_attempts )); then
            log "  attempt ${attempt}/${max_attempts} failed (rc=${rc}); retrying in ${delay}s..." >&2
            sleep "$delay"
            delay=$(( delay * 2 ))
        fi
        attempt=$(( attempt + 1 ))
    done
    return "$rc"
}

# ---------------------------------------------------------------------------
# Failure tracking
# ---------------------------------------------------------------------------
FAILURES_FILE=""

record_failure() {
    local kind="$1"
    local id="$2"
    local error="$3"
    local tmp
    tmp="$(mktemp)"
    if [[ -f "$FAILURES_FILE" ]]; then
        jq --arg kind "$kind" --arg id "$id" --arg error "$error" \
           '.failures += [{kind: $kind, id: $id, error: $error}]' \
           "$FAILURES_FILE" > "$tmp"
    else
        jq -n --arg kind "$kind" --arg id "$id" --arg error "$error" \
           '{failures: [{kind: $kind, id: $id, error: $error}]}' > "$tmp"
    fi
    mv "$tmp" "$FAILURES_FILE"
}

init_failures() {
    FAILURES_FILE="${BACKUP_DIR}/failures.json"
    jq -n '{failures: []}' > "$FAILURES_FILE"
}

failure_count() {
    if [[ -f "$FAILURES_FILE" ]]; then
        jq '.failures | length' "$FAILURES_FILE"
    else
        echo 0
    fi
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
    local failed=0
    for (( id=1; id<=total; id++ )); do
        local asset_json
        local read_err
        read_err="$(mktemp)"
        if asset_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_ASSET_REGISTRY" get_asset --asset_id "$id" 2>"$read_err")"; then
            if [[ -n "$asset_json" && "$asset_json" != "null" ]]; then
                echo "$asset_json" > "$BACKUP_DIR/assets/${id}.json"
                exported=$((exported + 1))
            fi
        else
            local msg
            msg="$(tr '\n' ' ' < "$read_err" | sed 's/  */ /g')"
            err "Failed to read asset ${id} after retries: ${msg}"
            record_failure "asset" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
        fi
        rm -f "$read_err"
    done

    jq -n --argjson exported "$exported" \
       --argjson failed "$failed" \
       --argjson total "$total" \
       '{exported: $exported, failed: $failed, total: $total}' \
       > "$BACKUP_DIR/asset_export_summary.json"

    log "Exported $exported / $total assets (${failed} failed)"
}

export_analysis_formats() {
    log "Exporting analysis formats..."
    python3 "$ROOT/scripts/export-assets.py" "$BACKUP_DIR" --format both
}

export_engineers() {
    log "Exporting engineer credentials..."

    local count_json
    local count_err
    count_err="$(mktemp)"
    if ! count_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_ENGINEER_REGISTRY" get_engineer_count 2>"$count_err")"; then
        local msg
        msg="$(tr '\n' ' ' < "$count_err" | sed 's/  */ /g')"
        err "Failed to read engineer count after retries: ${msg}"
        record_failure "engineer_count" "0" "${msg:-read failed}"
        rm -f "$count_err"
        jq -n '{engineers: [], count: 0, exported_at: (now | todate)}' \
           > "$BACKUP_DIR/engineers.json"
        return 0
    fi
    rm -f "$count_err"

    if [[ -z "$count_json" || "$count_json" == "null" ]]; then
        count_json=0
    fi
    local total="$count_json"
    log "Exporting $total engineers..."

    local engineers_tmp
    engineers_tmp="$(mktemp)"
    echo '[]' > "$engineers_tmp"

    local exported=0
    local failed=0
    for (( id=1; id<=total; id++ )); do
        local eng_json
        local eng_err
        eng_err="$(mktemp)"
        if ! eng_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_ENGINEER_REGISTRY" get_engineer --engineer_id "$id" 2>"$eng_err")"; then
            local msg
            msg="$(tr '\n' ' ' < "$eng_err" | sed 's/  */ /g')"
            err "Failed to read engineer ${id} after retries: ${msg}"
            record_failure "engineer" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
            rm -f "$eng_err"
            continue
        fi
        rm -f "$eng_err"

        if [[ -z "$eng_json" || "$eng_json" == "null" ]]; then
            continue
        fi

        # Fetch credential status (best-effort; recorded as failure on error).
        local status_json="null"
        local status_err
        status_err="$(mktemp)"
        if status_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_ENGINEER_REGISTRY" get_credential_status --engineer_id "$id" 2>"$status_err")"; then
            :
        else
            local msg
            msg="$(tr '\n' ' ' < "$status_err" | sed 's/  */ /g')"
            err "Failed to read credential status for engineer ${id}: ${msg}"
            record_failure "engineer_credential_status" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
            status_json="null"
        fi
        rm -f "$status_err"

        # Fetch issuer relationships (best-effort).
        local issuers_json="[]"
        local issuers_err
        issuers_err="$(mktemp)"
        if issuers_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_ENGINEER_REGISTRY" get_issuers --engineer_id "$id" 2>"$issuers_err")"; then
            if [[ -z "$issuers_json" || "$issuers_json" == "null" ]]; then
                issuers_json="[]"
            fi
        else
            local msg
            msg="$(tr '\n' ' ' < "$issuers_err" | sed 's/  */ /g')"
            err "Failed to read issuers for engineer ${id}: ${msg}"
            record_failure "engineer_issuers" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
            issuers_json="[]"
        fi
        rm -f "$issuers_err"

        # Normalise the engineer record into a stable shape.
        local record
        record="$(jq -n \
            --argjson raw "$eng_json" \
            --argjson status "$status_json" \
            --argjson issuers "$issuers_json" \
            --argjson id "$id" \
            '{
                engineer_id: $id,
                address: ($raw.address // $raw.engineer // $raw.id // null),
                credential_hash: ($raw.credential_hash // $raw.credential // $status.credential_hash // null),
                issuer: ($raw.issuer // $status.issuer // null),
                active: ($status.active // $raw.active // null),
                expiry: ($status.expiry // $status.expires_at // $raw.expiry // null),
                suspended: ($status.suspended // $raw.suspended // false),
                issuers: $issuers,
                raw: $raw
            }')"

        jq --argjson rec "$record" '. + [$rec]' "$engineers_tmp" > "${engineers_tmp}.new"
        mv "${engineers_tmp}.new" "$engineers_tmp"
        exported=$((exported + 1))
    done

    jq -n \
        --slurpfile engineers "$engineers_tmp" \
        --argjson exported "$exported" \
        --argjson failed "$failed" \
        --argjson total "$total" \
        --arg timestamp "$(date -u -Iseconds)" \
        '{
            exported_at: $timestamp,
            count: $exported,
            total: $total,
            failed: $failed,
            engineers: $engineers[0]
        }' > "$BACKUP_DIR/engineers.json"

    rm -f "$engineers_tmp"

    log "Exported $exported / $total engineers (${failed} failed)"
}

export_maintenance_history() {
    local total="$1"
    log "Exporting maintenance history..."
    mkdir -p "$BACKUP_DIR/maintenance"

    local count=0
    local failed=0
    for (( id=1; id<=total; id++ )); do
        local hist_json
        local hist_err
        hist_err="$(mktemp)"
        if hist_json="$(retry_with_backoff 3 invoke_read "$CONTRACT_LIFECYCLE" get_maintenance_history --asset_id "$id" 2>"$hist_err")"; then
            if [[ -n "$hist_json" && "$hist_json" != "null" && "$hist_json" != "[]" ]]; then
                echo "$hist_json" > "$BACKUP_DIR/maintenance/${id}_history.json"
                count=$((count + 1))
            fi
        else
            local msg
            msg="$(tr '\n' ' ' < "$hist_err" | sed 's/  */ /g')"
            err "Failed to read maintenance history for asset ${id} after retries: ${msg}"
            record_failure "maintenance_history" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
        fi
        rm -f "$hist_err"

        # Also export collateral score
        local score
        local score_err
        score_err="$(mktemp)"
        if score="$(retry_with_backoff 3 invoke_read "$CONTRACT_LIFECYCLE" get_collateral_score --asset_id "$id" 2>"$score_err")"; then
            if [[ -n "$score" && "$score" != "null" ]]; then
                echo "$score" > "$BACKUP_DIR/maintenance/${id}_score.json"
            fi
        else
            local msg
            msg="$(tr '\n' ' ' < "$score_err" | sed 's/  */ /g')"
            err "Failed to read collateral score for asset ${id} after retries: ${msg}"
            record_failure "collateral_score" "$id" "${msg:-read failed}"
            failed=$((failed + 1))
        fi
        rm -f "$score_err"
    done

    jq -n --argjson assets_with_history "$count" \
       --argjson failed "$failed" \
       '{assets_with_maintenance_history: $assets_with_history, failed: $failed}' \
       > "$BACKUP_DIR/maintenance_summary.json"

    log "Maintenance history exported for $count assets (${failed} failed)"
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

    # Verify bucket versioning is enabled BEFORE uploading. Versioning must be
    # provisioned out-of-band (e.g. via Terraform); we never enable it here.
    local versioning_status
    if ! versioning_status="$(aws s3api get-bucket-versioning \
            --bucket "$BACKUP_S3_BUCKET" \
            --query 'Status' \
            --output text 2>/dev/null)"; then
        err "Unable to read versioning status for bucket '${BACKUP_S3_BUCKET}'."
        err "Refusing to upload: versioning state is unknown."
        exit 1
    fi
    if [[ "$versioning_status" != "Enabled" ]]; then
        err "Bucket '${BACKUP_S3_BUCKET}' does not have versioning enabled (status: ${versioning_status:-<none>})."
        err "Enable versioning (e.g. via Terraform) before running backups."
        exit 1
    fi

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

    log "Backup uploaded to $s3_path"
}

sign_manifest() {
    local manifest="$1"
    local sig="${manifest}.sig"

    if [[ -n "${BACKUP_KMS_SIGNING_KEY:-}" ]]; then
        log "Signing manifest with KMS key ${BACKUP_KMS_SIGNING_KEY}..."
        local digest_b64
        digest_b64="$(openssl dgst -sha256 -binary "$manifest" | base64 -w0)"
        local sig_b64
        if ! sig_b64="$(aws kms sign \
                --key-id "$BACKUP_KMS_SIGNING_KEY" \
                --message "$digest_b64" \
                --message-type DIGEST \
                --signing-algorithm RSASSA_PKCS1_V1_5_SHA_256 \
                --query Signature \
                --output text)"; then
            err "KMS signing of manifest failed."
            return 1
        fi
        jq -n \
            --arg method "kms" \
            --arg key_id "$BACKUP_KMS_SIGNING_KEY" \
            --arg algorithm "RSASSA_PKCS1_V1_5_SHA_256" \
            --arg signature "$sig_b64" \
            '{method: $method, key_id: $key_id, algorithm: $algorithm, signature: $signature}' \
            > "$sig"
        log "Manifest signature written to $sig"
        return 0
    fi

    if command -v minisign >/dev/null 2>&1 && [[ -n "${BACKUP_MINISIGN_KEY:-}" ]]; then
        log "Signing manifest with minisign..."
        if ! minisign -S -s "$BACKUP_MINISIGN_KEY" -m "$manifest" -x "$sig"; then
            err "minisign signing of manifest failed."
            return 1
        fi
        log "Manifest signature written to $sig"
        return 0
    fi

    err "No manifest signing key configured (set BACKUP_KMS_SIGNING_KEY or BACKUP_MINISIGN_KEY)."
    return 1
}

generate_manifest() {
    log "Generating backup manifest..."
    local manifest="${BACKUP_DIR}/manifest.json"

    # 1. Write the manifest FIRST so it is covered by the checksums below.
    jq -n \
        --arg backup_id "$TIMESTAMP" \
        --arg network "$STELLAR_NETWORK" \
        --arg timestamp "$(date -u -Iseconds)" \
        --arg asset_registry "$CONTRACT_ASSET_REGISTRY" \
        --arg engineer_registry "$CONTRACT_ENGINEER_REGISTRY" \
        --arg lifecycle "$CONTRACT_LIFECYCLE" \
        --argjson failures "$(failure_count)" \
        '{
            backup_id: $backup_id,
            network: $network,
            created_at: $timestamp,
            contracts: {
                asset_registry: $asset_registry,
                engineer_registry: $engineer_registry,
                lifecycle: $lifecycle
            },
            read_failures: $failures,
            contents: "See individual files in this directory."
        }' > "$manifest"

    # 2. Sign the manifest and write manifest.json.sig alongside it.
    sign_manifest "$manifest"

    # 3. Hash every file in the backup directory except checksums.sha256
    #    itself (which does not exist yet). This includes manifest.json,
    #    manifest.json.sig and failures.json.
    local file_list
    file_list="$(cd "$BACKUP_DIR" && find . -type f ! -name 'checksums.sha256' -print0 \
        | sort -z \
        | xargs -0 sha256sum)"

    echo "$file_list" > "${BACKUP_DIR}/checksums.sha256"

    log "Manifest written to $manifest"
    log "Checksums written to ${BACKUP_DIR}/checksums.sha256"
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

    # 0. Initialise failure tracking
    init_failures

    # 1. Export contract metadata
    export_contract_metadata

    # 2. Export asset count
    local total
    total="$(export_asset_count)"

    # 3. Export all assets
    export_assets "$total"

    # 4. Export analysis-friendly asset formats
    export_analysis_formats

    # 5. Export engineer credentials
    export_engineers

    # 6. Export maintenance history and scores
    export_maintenance_history "$total"

    # 7. Generate manifest, sign it, and compute checksums
    generate_manifest

    # 8. Abort if any reads failed after retries
    local failures
    failures="$(failure_count)"
    if [[ "$failures" -gt 0 ]]; then
        err "Backup incomplete: ${failures} read(s) failed after retries."
        err "See ${FAILURES_FILE} for details."
        exit 1
    fi

    # 9. Upload to S3 (if configured)
    upload_to_s3

    log "=== Backup complete ==="
    log "Local backup: ${BACKUP_DIR}"
    if [[ "${S3_ENABLED:-false}" == "true" ]]; then
        log "S3 location: s3://${BACKUP_S3_BUCKET}/backups/${TIMESTAMP}/"
    fi
}

main "$@"
