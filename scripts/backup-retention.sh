#!/usr/bin/env bash
# =============================================================================
# Mainstay Backup Retention
# =============================================================================
# Prunes backup directories under a root according to the retention policy in
# docs/backup-procedures.md:
#
#   age <= 30 days          keep every backup              (daily tier)
#   30 days < age <= 84     keep the newest per ISO week   (weekly tier, 12 weeks)
#   84 days < age <= 365    keep the newest per month      (monthly tier, 12 months)
#   age > 365 days          delete
#
# A backup directory containing a file named `.keep` is never deleted. Use this
# for on-demand backups taken before contract upgrades (permanent retention).
#
# Only directories named like backup.sh's timestamps (YYYYMMDDTHHMMSSZ) are
# considered, so unrelated files under the root are never touched.
#
# Usage:
#   ./scripts/backup-retention.sh <backup_root> [--dry-run]
#
# Environment:
#   RETENTION_NOW  - epoch seconds to evaluate against (default: current time).
#                    Useful for testing and for re-running a prune for a past date.
# =============================================================================

set -euo pipefail

DAY=86400
DAILY_DAYS=30
WEEKLY_DAYS=84
MONTHLY_DAYS=365

usage() {
    sed -n '3,25p' "$0" | sed 's/^# \{0,1\}//'
}

if [[ $# -lt 1 || "$1" == "-h" || "$1" == "--help" ]]; then
    usage
    exit 1
fi

ROOT="$1"
DRY_RUN=false
if [[ "${2:-}" == "--dry-run" ]]; then
    DRY_RUN=true
fi

if [[ ! -d "$ROOT" ]]; then
    echo "ERROR: backup root not found: $ROOT" >&2
    exit 1
fi

NOW="${RETENTION_NOW:-$(date -u +%s)}"

# Convert a backup_id such as 20261016T000001Z into epoch seconds.
ts_to_epoch() {
    local t="$1"
    date -u -d "${t:0:4}-${t:4:2}-${t:6:2} ${t:9:2}:${t:11:2}:${t:13:2}" +%s
}

# Collect candidate backup directories, newest first.
mapfile -t candidates < <(
    find "$ROOT" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' \
        | grep -E '^[0-9]{8}T[0-9]{6}Z$' \
        | sort -r
)

declare -A kept_week=()
declare -A kept_month=()
kept=0
deleted=0

for name in "${candidates[@]}"; do
    dir="$ROOT/$name"
    epoch="$(ts_to_epoch "$name")"
    age_days=$(( (NOW - epoch) / DAY ))
    reason=""
    keep=false

    if [[ -f "$dir/.keep" ]]; then
        keep=true; reason="pinned (.keep)"
    elif (( age_days <= DAILY_DAYS )); then
        keep=true; reason="daily tier"
    elif (( age_days <= WEEKLY_DAYS )); then
        key="$(date -u -d "@$epoch" +%G-%V)"
        if [[ -z "${kept_week[$key]:-}" ]]; then
            kept_week[$key]=1; keep=true; reason="weekly tier ($key)"
        fi
    elif (( age_days <= MONTHLY_DAYS )); then
        key="$(date -u -d "@$epoch" +%Y-%m)"
        if [[ -z "${kept_month[$key]:-}" ]]; then
            kept_month[$key]=1; keep=true; reason="monthly tier ($key)"
        fi
    fi

    if [[ "$keep" == true ]]; then
        kept=$((kept + 1))
        echo "keep    $name  (${age_days}d, $reason)"
    else
        deleted=$((deleted + 1))
        if [[ "$DRY_RUN" == true ]]; then
            echo "would delete  $name  (${age_days}d)"
        else
            rm -rf -- "$dir"
            echo "deleted $name  (${age_days}d)"
        fi
    fi
done

echo "Retention summary: kept=$kept deleted=$deleted dry_run=$DRY_RUN"
