#!/usr/bin/env bash
# =============================================================================
# Tests for backup verification and retention tooling.
# Runs entirely on temporary directories; needs no network or Stellar CLI.
#
# Usage: scripts/test_backup_tooling.sh
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERIFY="$ROOT/scripts/verify-backup.sh"
RETAIN="$ROOT/scripts/backup-retention.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

failures=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# Build a fake backup directory with a manifest and checksums in the same
# format as backup.sh (relative paths, checksums.sha256 excluded).
make_backup() {
    local dir="$1"
    mkdir -p "$dir/data"
    echo '{"backup_id":"test","network":"testnet"}' > "$dir/manifest.json"
    echo '[{"id":1}]' > "$dir/data/assets.json"
    echo 'engineers' > "$dir/data/engineers.txt"
    (cd "$dir" && find . -type f ! -name checksums.sha256 -exec sha256sum {} \; | sort -k2 > checksums.sha256)
}

# --- Verification -----------------------------------------------------------

B1="$WORK/b1"
make_backup "$B1"
if "$VERIFY" "$B1" >/dev/null; then pass "valid backup verifies"; else fail "valid backup verifies"; fi

# Checksums use relative paths, so a copy elsewhere must still verify.
B1_COPY="$WORK/relocated/b1"
mkdir -p "$WORK/relocated" && cp -a "$B1" "$B1_COPY"
if "$VERIFY" "$B1_COPY" >/dev/null; then pass "relocated copy verifies (relative checksums)"; else fail "relocated copy verifies"; fi

# A replica with identical checksums passes --compare.
if "$VERIFY" "$B1_COPY" --compare "$B1" >/dev/null; then pass "identical replica passes --compare"; else fail "identical replica passes --compare"; fi

# Tampered content must fail.
echo 'tampered' >> "$B1_COPY/data/assets.json"
if "$VERIFY" "$B1_COPY" >/dev/null 2>&1; then fail "tampered file is detected"; else pass "tampered file is detected"; fi

# A file added after checksums were written must fail.
B2="$WORK/b2"
make_backup "$B2"
echo 'extra' > "$B2/data/unlisted.txt"
if "$VERIFY" "$B2" >/dev/null 2>&1; then fail "unlisted file is detected"; else pass "unlisted file is detected"; fi

# Replica that differs in content must fail --compare.
B3="$WORK/b3"
make_backup "$B3"
echo 'different' > "$B3/data/engineers.txt"
(cd "$B3" && sha256sum data/engineers.txt >/dev/null && find . -type f ! -name checksums.sha256 -exec sha256sum {} \; | sort -k2 > checksums.sha256)
if "$VERIFY" "$B3" --compare "$B1" >/dev/null 2>&1; then fail "divergent replica is detected"; else pass "divergent replica is detected"; fi

# Missing manifest must fail.
B4="$WORK/b4"
make_backup "$B4"
rm "$B4/manifest.json"
if "$VERIFY" "$B4" >/dev/null 2>&1; then fail "missing manifest is detected"; else pass "missing manifest is detected"; fi

# --- Retention --------------------------------------------------------------

# Fixed evaluation time so results do not depend on the wall clock.
NOW=$(date -u -d "2026-10-16 12:00:00" +%s)
R="$WORK/retention"
mkdir -p "$R"

stamp() { date -u -d "@$1" +%Y%m%dT%H%M%SZ; }
mk() { mkdir -p "$R/$1"; echo x > "$R/$1/manifest.json"; }

# Daily tier: every backup from the last 30 days is kept.
for d in 0 3 10 29; do mk "$(stamp $((NOW - d * 86400)))"; done

# Weekly tier (31-84 days): two backups in one ISO week, so only the newer is kept.
mk "$(stamp $((NOW - 45 * 86400)))"
mk "$(stamp $((NOW - 45 * 86400 - 3600)))"

# Monthly tier (85-365 days): two backups in one month, so only the newer is kept.
mk "$(stamp $((NOW - 120 * 86400)))"
mk "$(stamp $((NOW - 120 * 86400 - 7200)))"

# Older than a year: deleted, unless pinned.
mk "$(stamp $((NOW - 400 * 86400)))"
mk "$(stamp $((NOW - 401 * 86400)))"
touch "$R/$(stamp $((NOW - 401 * 86400)))/.keep"

# Non-backup directories are never touched.
mkdir -p "$R/not-a-backup"

RETENTION_NOW="$NOW" "$RETAIN" "$R" --dry-run >/dev/null
if [[ "$(find "$R" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 11 ]]; then
    pass "dry run deletes nothing"
else
    fail "dry run deletes nothing"
fi

RETENTION_NOW="$NOW" "$RETAIN" "$R" >/dev/null

remaining=$(find "$R" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
expected=$(printf '%s\n' \
    "$(stamp $((NOW - 0)))" "$(stamp $((NOW - 3 * 86400)))" \
    "$(stamp $((NOW - 10 * 86400)))" "$(stamp $((NOW - 29 * 86400)))" \
    "$(stamp $((NOW - 45 * 86400)))" \
    "$(stamp $((NOW - 120 * 86400)))" \
    "$(stamp $((NOW - 401 * 86400)))" \
    "not-a-backup" | sort)

if [[ "$remaining" == "$expected" ]]; then
    pass "retention keeps daily, weekly, monthly, pinned, and non-backup directories"
else
    fail "retention result differs from policy"
    echo "expected:"; echo "$expected"; echo "actual:"; echo "$remaining"
fi

if [[ "$failures" -gt 0 ]]; then
    echo "$failures test(s) failed" >&2
    exit 1
fi
echo "All backup tooling tests passed."
