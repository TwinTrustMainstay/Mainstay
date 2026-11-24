#!/usr/bin/env bash
# =============================================================================
# Mainstay Backup Verifier
# =============================================================================
# Checks that a backup directory is complete and unmodified:
#   1. manifest.json and checksums.sha256 exist, and the manifest is valid JSON
#   2. every file listed in checksums.sha256 matches its SHA-256 digest
#   3. every file in the backup (except checksums.sha256) is listed, so nothing
#      was added or left out after the checksums were written
#
# With --compare, also checks that the replica's checksum list is identical to
# the reference backup's, so a replica is known to hold the same content.
#
# Usage:
#   ./scripts/verify-backup.sh <backup_dir>
#   ./scripts/verify-backup.sh <backup_dir> --compare <reference_dir>
#
# Exit status: 0 if verified, 1 otherwise.
# =============================================================================

set -euo pipefail

usage() {
    sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//'
}

err() { echo "ERROR: $*" >&2; }

if [[ $# -lt 1 || "$1" == "-h" || "$1" == "--help" ]]; then
    usage
    exit 1
fi

BACKUP_DIR="$1"
REFERENCE_DIR=""
if [[ "${2:-}" == "--compare" ]]; then
    REFERENCE_DIR="${3:?--compare requires a reference directory}"
fi

if [[ ! -d "$BACKUP_DIR" ]]; then
    err "backup directory not found: $BACKUP_DIR"
    exit 1
fi

for required in manifest.json checksums.sha256; do
    if [[ ! -f "$BACKUP_DIR/$required" ]]; then
        err "missing $required in $BACKUP_DIR"
        exit 1
    fi
done

if ! jq -e . "$BACKUP_DIR/manifest.json" >/dev/null 2>&1; then
    err "manifest.json is not valid JSON"
    exit 1
fi

# Digests (relative paths) must match the files on disk.
if ! (cd "$BACKUP_DIR" && sha256sum -c checksums.sha256 --quiet); then
    err "checksum mismatch in $BACKUP_DIR"
    exit 1
fi

# Every file on disk must be covered by the checksum list.
listed=$(awk '{ print $2 }' "$BACKUP_DIR/checksums.sha256" | sed 's|^\./||' | sort)
on_disk=$(cd "$BACKUP_DIR" && find . -type f ! -name checksums.sha256 | sed 's|^\./||' | sort)
if [[ "$listed" != "$on_disk" ]]; then
    err "files on disk do not match checksums.sha256 in $BACKUP_DIR"
    diff <(echo "$listed") <(echo "$on_disk") >&2 || true
    exit 1
fi

if [[ -n "$REFERENCE_DIR" ]]; then
    if [[ ! -f "$REFERENCE_DIR/checksums.sha256" ]]; then
        err "reference has no checksums.sha256: $REFERENCE_DIR"
        exit 1
    fi
    if ! diff -q <(sort "$BACKUP_DIR/checksums.sha256") \
                 <(sort "$REFERENCE_DIR/checksums.sha256") >/dev/null; then
        err "replica $BACKUP_DIR differs from reference $REFERENCE_DIR"
        exit 1
    fi
fi

echo "VERIFIED: $BACKUP_DIR ($(wc -l < "$BACKUP_DIR/checksums.sha256") files)"
