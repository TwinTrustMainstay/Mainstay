#!/usr/bin/env bash
# =============================================================================
# Mainstay Batch Import Script for Historical Maintenance Records
# =============================================================================
# Imports historical maintenance records from a CSV file into Mainstay
# smart contracts.  Supports dry-run validation, chunked imports, and
# detailed reporting.
#
# Usage:
#   ./scripts/batch-import.sh <csv_file> [options]
#
# Options:
#   --dry-run                  Validate CSV only, no chain writes
#   --admin-key <path>         Path to admin Stellar key (required for live import)
#   --start-row <N>            Start importing at CSV row N (header=1)
#   --end-row <N>              Stop importing at CSV row N
#   --skip-asset-registration  Skip asset registration (assets already on-chain)
#   --batch-size <N>           Records per batch (default: 50, max: 50)
#
# CSV columns (header required):
#   asset_serial_number,asset_type,metadata,engineer_name,engineer_address,
#   task_type,timestamp,notes
#
# Historical timestamps:
#   The Lifecycle contract records the ledger timestamp when
#   submit_maintenance is invoked; it has no parameter for a historical
#   time.  The CSV `timestamp` column is therefore validated (not in the
#   future) but the on-chain record will carry the import time.  Keep the
#   original CSV as the authoritative historical record.
#
# Environment variables:
#   CONTRACT_ASSET_REGISTRY     - Asset Registry contract ID
#   CONTRACT_LIFECYCLE          - Lifecycle contract ID
#   STELLAR_NETWORK             - Stellar network
#   STELLAR_RPC_URL             - Stellar RPC endpoint
#   EXPECTED_ADMIN_ADDRESS      - Expected admin address (optional; when set,
#                                 the derived --admin-key address must match)
#   ADMIN_IDENTITY              - Stellar CLI identity name for the admin key
#                                 (default: mainstay-admin)
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

source ~/.cargo/env 2>/dev/null || true
source .env 2>/dev/null || true

: "${STELLAR_NETWORK:=testnet}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

DRY_RUN=false
ADMIN_KEY_PATH=""
SKIP_ASSET_REG=false
START_ROW=2    # Row 1 is header
END_ROW=""
BATCH_SIZE=50
CSV_FILE=""
ADMIN_IDENTITY="${ADMIN_IDENTITY:-mainstay-admin}"
ADMIN_ADDRESS=""
# Serial number -> asset_id map, populated during registration / lookup.
declare -A ASSET_IDS=()

usage() {
    cat <<EOF
Usage: $0 <csv_file> [options]

Options:
  --dry-run                  Validate CSV only (no chain writes)
  --admin-key <path>         Path to admin Stellar key file (required for live import)
  --start-row <N>            Import starting at row N (default: 2, skips header)
  --end-row <N>              Import ending at row N (default: all rows)
  --skip-asset-registration  Skip registering assets (if already on-chain)
  --batch-size <N>           Records per batch (default: 50, max 50)

Environment:
  CONTRACT_ASSET_REGISTRY     Asset Registry contract ID
  CONTRACT_LIFECYCLE          Lifecycle contract ID
  STELLAR_NETWORK             Stellar network (testnet/mainnet)
  EXPECTED_ADMIN_ADDRESS      Expected admin address (optional check)
  ADMIN_IDENTITY              Stellar CLI identity name (default: mainstay-admin)

EOF
    exit 1
}

if [[ $# -lt 1 ]]; then usage; fi

CSV_FILE="$1"
shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --admin-key) ADMIN_KEY_PATH="$2"; shift 2 ;;
        --start-row) START_ROW="$2"; shift 2 ;;
        --end-row) END_ROW="$2"; shift 2 ;;
        --skip-asset-registration) SKIP_ASSET_REG=true; shift ;;
        --batch-size) BATCH_SIZE="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------

if [[ ! -f "$CSV_FILE" ]]; then
    echo "ERROR: CSV file not found: $CSV_FILE" >&2
    exit 1
fi

if [[ "$BATCH_SIZE" -gt 50 ]]; then
    echo "WARNING: Batch size capped at 50 (Soroban limit). Using 50." >&2
    BATCH_SIZE=50
fi

if [[ "$DRY_RUN" != "true" && -z "$ADMIN_KEY_PATH" ]]; then
    echo "ERROR: --admin-key is required for live import (or use --dry-run)." >&2
    exit 1
fi

if [[ "$DRY_RUN" != "true" && ! -f "$ADMIN_KEY_PATH" ]]; then
    echo "ERROR: admin key file not found: $ADMIN_KEY_PATH" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() { echo "[$(date -u +%H:%M:%S)] $*"; }
warn() { echo "[$(date -u +%H:%M:%S)] WARNING: $*" >&2; }
err() { echo "[$(date -u +%H:%M:%S)] ERROR: $*" >&2; }

# Map a CSV task_type to a contract Priority variant.
task_type_to_priority() {
    case "$1" in
        OIL_CHG|LUBE|FILTER) echo "Low" ;;
        INSPECT|TUNE_UP) echo "Medium" ;;
        BRAKE|ENGINE) echo "High" ;;
        OVERHAUL|REBUILD) echo "Critical" ;;
        *) echo "Low" ;;
    esac
}

# Import the admin key as an explicit Stellar CLI identity and verify it.
setup_admin_identity() {
    if [[ "$DRY_RUN" == "true" ]]; then
        return 0
    fi

    log "Importing admin key as identity '$ADMIN_IDENTITY'..."
    local secret
    secret="$(tr -d '[:space:]' < "$ADMIN_KEY_PATH")"
    if [[ -z "$secret" ]]; then
        err "Admin key file is empty: $ADMIN_KEY_PATH"
        exit 1
    fi

    # Remove any stale identity with the same name, then import the key.
    stellar keys remove "$ADMIN_IDENTITY" >/dev/null 2>&1 || true
    if ! stellar keys add "$ADMIN_IDENTITY" --secret-key "$secret" >/dev/null 2>&1; then
        err "Failed to import admin key as identity '$ADMIN_IDENTITY'."
        exit 1
    fi

    ADMIN_ADDRESS="$(stellar keys address "$ADMIN_IDENTITY" 2>/dev/null || true)"
    if [[ -z "$ADMIN_ADDRESS" ]]; then
        err "Could not derive address for identity '$ADMIN_IDENTITY'."
        exit 1
    fi

    if [[ -n "${EXPECTED_ADMIN_ADDRESS:-}" && "$ADMIN_ADDRESS" != "$EXPECTED_ADMIN_ADDRESS" ]]; then
        err "Admin address mismatch: derived $ADMIN_ADDRESS, expected $EXPECTED_ADMIN_ADDRESS."
        exit 1
    fi

    log "Admin identity ready: $ADMIN_ADDRESS"
}

read_csv_field() {
    local row="$1"
    local col="$2"
    # Extract field from CSV, handling quoted values with commas
    awk -F',' -v row="$row" -v col="$col" '
    NR == row {
        field = $col
        # Handle quoted fields
        if (field ~ /^"/) {
            while (field !~ /"$/) { col++; field = field "," $(col) }
            gsub(/^"|"$/, "", field)
        }
        print field
    }' "$CSV_FILE"
}

total_rows() {
    wc -l < "$CSV_FILE"
}

validate_csv_header() {
    local expected="asset_serial_number,asset_type,metadata,engineer_name,engineer_address,task_type,timestamp,notes"
    local actual
    actual="$(head -1 "$CSV_FILE" | tr -d '\r')"
    if [[ "$actual" != "$expected" ]]; then
        err "CSV header mismatch."
        err "Expected: $expected"
        err "Got:      $actual"
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Validation Checks
# ---------------------------------------------------------------------------

validate_row() {
    local row="$1"
    local issues=0

    local sn asset_type metadata eng_name eng_addr task_type ts notes
    sn="$(read_csv_field "$row" 1)"
    asset_type="$(read_csv_field "$row" 2)"
    eng_addr="$(read_csv_field "$row" 5)"
    task_type="$(read_csv_field "$row" 6)"
    ts="$(read_csv_field "$row" 7)"
    notes="$(read_csv_field "$row" 8)"

    # Future date check
    local now
    now="$(date +%s)"
    if [[ "$ts" -gt "$now" ]]; then
        warn "Row $row: timestamp is in the future ($ts > $now)"
        issues=$((issues + 1))
    fi

    # Serial number length
    if [[ "${#sn}" -gt 64 ]]; then
        warn "Row $row: serial_number exceeds 64 chars (${#sn})"
        issues=$((issues + 1))
    fi

    # Notes length
    if [[ "${#notes}" -gt 256 ]]; then
        warn "Row $row: notes exceed 256 chars (${#notes})"
        issues=$((issues + 1))
    fi

    # Asset type validation (alphanumeric + underscore)
    if [[ "$asset_type" =~ [^A-Za-z0-9_] ]]; then
        warn "Row $row: asset_type contains invalid characters: $asset_type"
        issues=$((issues + 1))
    fi

    # Engineer address length (Stellar addresses are 56 chars)
    if [[ "${#eng_addr}" -ne 56 ]]; then
        warn "Row $row: engineer_address should be 56 chars (got ${#eng_addr})"
        issues=$((issues + 1))
    fi

    # Task type must be one of the known types
    local valid_types="OIL_CHG LUBE INSPECT FILTER TUNE_UP BRAKE ENGINE OVERHAUL REBUILD"
    if ! echo "$valid_types" | grep -qw "$task_type"; then
        warn "Row $row: unrecognized task_type '$task_type' (defaults to weight 3)"
    fi

    return "$issues"
}

run_validation() {
    local end
    end="${END_ROW:-$(total_rows)}"
    log "Validating rows $START_ROW to $end..."

    local total_issues=0
    for (( row=START_ROW; row<=end; row++ )); do
        local line
        line="$(sed -n "${row}p" "$CSV_FILE")"
        if [[ -z "${line:-}" || "$line" =~ ^[[:space:]]*$ ]]; then
            continue
        fi
        if ! validate_row "$row"; then
            total_issues=$((total_issues + 1))
        fi
    done

    if [[ "$total_issues" -gt 0 ]]; then
        err "Validation found $total_issues issues. Fix them before importing."
        exit 1
    fi
    log "Validation passed: 0 issues found."
}

# ---------------------------------------------------------------------------
# Asset Registration
# ---------------------------------------------------------------------------

register_assets_from_csv() {
    local end
    end="${END_ROW:-$(total_rows)}"
    log "Registering assets from CSV..."

    local registered=0
    local skipped=0

    for (( row=START_ROW; row<=end; row++ )); do
        local sn asset_type metadata
        sn="$(read_csv_field "$row" 1)"
        asset_type="$(read_csv_field "$row" 2)"
        metadata="$(read_csv_field "$row" 3)"

        if [[ -z "$sn" || -z "$asset_type" || -z "$metadata" ]]; then
            skipped=$((skipped + 1))
            continue
        fi

        # Check if already registered
        # (In a full implementation, we'd query the chain first)
        if [[ "$DRY_RUN" != "true" ]]; then
            local out
            if out="$(stellar contract invoke \
                --id "$CONTRACT_ASSET_REGISTRY" \
                --network "$STELLAR_NETWORK" \
                --source "$ADMIN_IDENTITY" \
                -- register_asset \
                --asset_type "$asset_type" \
                --metadata "$metadata" \
                --serial_number "$sn" \
                --owner "$ADMIN_ADDRESS" 2>&1)"; then
                registered=$((registered + 1))
                local asset_id
                asset_id="$(printf '%s\n' "$out" | tr -d '"[:space:]')"
                if [[ -n "$asset_id" ]]; then
                    ASSET_IDS["$sn"]="$asset_id"
                fi
            else
                warn "Row $row: asset registration failed: $(printf '%s\n' "$out" | head -1)"
                skipped=$((skipped + 1))
            fi
        else
            log "[DRY RUN] Would register asset: type=$asset_type, sn=$sn"
            registered=$((registered + 1))
        fi
    done

    log "Asset registration: $registered registered, $skipped skipped"
}

# Resolve a serial number to an on-chain asset_id.
resolve_asset_id() {
    local sn="$1"
    if [[ -n "${ASSET_IDS[$sn]:-}" ]]; then
        echo "${ASSET_IDS[$sn]}"
        return 0
    fi
    local asset_id
    asset_id="$(stellar contract invoke \
        --id "$CONTRACT_ASSET_REGISTRY" \
        --network "$STELLAR_NETWORK" \
        --source "$ADMIN_IDENTITY" \
        -- get_asset_by_serial \
        --serial_number "$sn" 2>/dev/null | tr -d '"[:space:]')" || true
    if [[ -n "$asset_id" ]]; then
        ASSET_IDS["$sn"]="$asset_id"
        echo "$asset_id"
        return 0
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Maintenance Import
# ---------------------------------------------------------------------------

import_maintenance_records() {
    local end
    end="${END_ROW:-$(total_rows)}"
    log "Importing maintenance records (batch size: $BATCH_SIZE)..."

    local imported=0
    local skipped=0
    local failed=0
    local batch_count=0

    local -a batch_rows=()

    submit_batch() {
        local -a rows=("$@")
        if [[ "${#rows[@]}" -eq 0 ]]; then
            return 0
        fi
        batch_count=$((batch_count + 1))
        log "Submitting batch $batch_count (${#rows[@]} record(s))..."

        local row
        for row in "${rows[@]}"; do
            local sn eng_addr task_type ts notes
            sn="$(read_csv_field "$row" 1)"
            eng_addr="$(read_csv_field "$row" 5)"
            task_type="$(read_csv_field "$row" 6)"
            ts="$(read_csv_field "$row" 7)"
            notes="$(read_csv_field "$row" 8)"

            if [[ -z "$sn" || -z "$eng_addr" || -z "$task_type" || -z "$ts" ]]; then
                skipped=$((skipped + 1))
                continue
            fi

            local priority fee
            priority="$(task_type_to_priority "$task_type")"
            fee="${MAINTENANCE_FEE:-0}"

            if [[ "$DRY_RUN" != "true" ]]; then
                local asset_id
                if ! asset_id="$(resolve_asset_id "$sn")"; then
                    warn "Row $row: could not resolve asset_id for serial '$sn'"
                    failed=$((failed + 1))
                    continue
                fi

                local out
                if out="$(stellar contract invoke \
                    --id "$CONTRACT_LIFECYCLE" \
                    --network "$STELLAR_NETWORK" \
                    --source "$ADMIN_IDENTITY" \
                    -- submit_maintenance \
                    --asset_id "$asset_id" \
                    --engineer "$eng_addr" \
                    --task_type "$task_type" \
                    --priority "$priority" \
                    --fee "$fee" \
                    --notes "${notes:-Imported from paper records}" 2>&1)"; then
                    imported=$((imported + 1))
                else
                    failed=$((failed + 1))
                    err "Row $row: submit_maintenance failed: $(printf '%s\n' "$out" | head -1)"
                fi
            else
                log "[DRY RUN] Would submit: sn=$sn, engineer=$eng_addr, task=$task_type, priority=$priority, fee=$fee, ts=$ts"
                imported=$((imported + 1))
            fi
        done
    }

    local row
    for (( row=START_ROW; row<=end; row++ )); do
        local line
        line="$(sed -n "${row}p" "$CSV_FILE")"
        if [[ -z "${line:-}" || "$line" =~ ^[[:space:]]*$ ]]; then
            continue
        fi
        batch_rows+=("$row")
        if [[ "${#batch_rows[@]}" -ge "$BATCH_SIZE" ]]; then
            submit_batch "${batch_rows[@]}"
            batch_rows=()
        fi
    done
    if [[ "${#batch_rows[@]}" -gt 0 ]]; then
        submit_batch "${batch_rows[@]}"
    fi

    log "Maintenance import: $imported imported, $skipped skipped, $failed failed ($batch_count batch(es))"
}

# ---------------------------------------------------------------------------
# Summary Report
# ---------------------------------------------------------------------------

generate_report() {
    local end
    end="${END_ROW:-$(total_rows)}"
    local total_rows_processed=$((end - START_ROW + 1))

    cat <<EOF

========================================
   MAINTAINT IMPORT SUMMARY
========================================
CSV file:               $CSV_FILE
Rows processed:         $total_rows_processed
Dry run:                $DRY_RUN
Network:                $STELLAR_NETWORK
Asset Registry:         ${CONTRACT_ASSET_REGISTRY:-"not set"}
Lifecycle Contract:     ${CONTRACT_LIFECYCLE:-"not set"}
========================================
EOF
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    log "=== Mainstay Batch Import ==="
    log "CSV file: $CSV_FILE"
    log "Network: $STELLAR_NETWORK"
    log "Dry run: $DRY_RUN"
    log ""

    # 1. Validate CSV header
    validate_csv_header

    # 2. Validate all rows
    run_validation

    # 3. Set up the admin signing identity (live imports only)
    setup_admin_identity

    # 4. Register assets
    if [[ "$SKIP_ASSET_REG" != "true" ]]; then
        register_assets_from_csv
    else
        log "Skipping asset registration (--skip-asset-registration)"
    fi

    # 5. Import maintenance records
    import_maintenance_records

    # 6. Print summary
    generate_report
}

main
