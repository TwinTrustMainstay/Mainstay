#!/usr/bin/env bash
#
# Test: scripts/restore.sh --dry-run must not import the admin key.
#
# Regression test for issue #1835: `invoke_write` ran `stellar keys import`
# before checking DRY_RUN, so a "preview" wrote the admin secret into the
# local identity store. This test runs restore.sh in --dry-run mode inside an
# isolated STELLAR config directory and asserts that no `admin` identity is
# created.
#
# Usage: scripts/test-restore-dry-run.sh
# Exit code 0 on success, non-zero on failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RESTORE_SH="${REPO_ROOT}/scripts/restore.sh"

if [[ ! -f "${RESTORE_SH}" ]]; then
  echo "FAIL: restore.sh not found at ${RESTORE_SH}" >&2
  exit 1
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "SKIP: stellar CLI not found in PATH" >&2
  exit 0
fi

# Isolated STELLAR config directory so we never touch the developer's keys.
TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

export STELLAR_CONFIG_DIR="${TMP_DIR}/stellar-config"
mkdir -p "${STELLAR_CONFIG_DIR}"

# Fixture admin key. This is a well-known test-only secret; never use in prod.
FIXTURE_ADMIN_KEY="SDSAVCXPQJZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQZQ"

# Fixture backup directory. restore.sh only needs the path to exist for a
# dry-run preview; it must not perform any writes.
FIXTURE_BACKUP="${TMP_DIR}/fixture-backup"
mkdir -p "${FIXTURE_BACKUP}"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Snapshot the keys directory before the run (may not exist yet).
KEYS_DIR="${STELLAR_CONFIG_DIR}/identity"
before_listing=""
if [[ -d "${KEYS_DIR}" ]]; then
  before_listing="$(find "${KEYS_DIR}" -mindepth 1 -maxdepth 1 -printf '%f\n' 2>/dev/null | sort || true)"
fi

echo "Running restore.sh --dry-run in isolated config dir: ${STELLAR_CONFIG_DIR}"

set +e
"${RESTORE_SH}" "${FIXTURE_BACKUP}" --dry-run --admin-key "${FIXTURE_ADMIN_KEY}" \
  >"${TMP_DIR}/restore.out" 2>"${TMP_DIR}/restore.err"
restore_status=$?
set -e

# A dry-run may legitimately exit non-zero for unrelated reasons (e.g. missing
# network access), but it must never import the admin key. We surface the
# output for debugging regardless.
echo "--- restore.sh stdout ---"
cat "${TMP_DIR}/restore.out" || true
echo "--- restore.sh stderr ---"
cat "${TMP_DIR}/restore.err" || true
echo "--- restore.sh exit status: ${restore_status} ---"

# Assertion 1: `stellar keys address admin` must fail (identity not created).
if stellar keys address admin >/dev/null 2>&1; then
  fail "admin identity was created during --dry-run (issue #1835 regression)"
fi

# Assertion 2: the keys directory contents must be unchanged.
after_listing=""
if [[ -d "${KEYS_DIR}" ]]; then
  after_listing="$(find "${KEYS_DIR}" -mindepth 1 -maxdepth 1 -printf '%f\n' 2>/dev/null | sort || true)"
fi

if [[ "${before_listing}" != "${after_listing}" ]]; then
  fail "identity store changed during --dry-run:
before:
${before_listing}
after:
${after_listing}"
fi

# Assertion 3: no file named 'admin' should exist anywhere in the config dir.
if find "${STELLAR_CONFIG_DIR}" -name 'admin' -print -quit 2>/dev/null | grep -q .; then
  fail "found an 'admin' entry in the STELLAR config dir after --dry-run"
fi

echo "PASS: restore.sh --dry-run did not import the admin key"
exit 0