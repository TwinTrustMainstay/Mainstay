#!/usr/bin/env bash
# Pre/post-upgrade state capture and validation (issue #1725).
#
# Captures a normalized snapshot of contract state via read-only view calls,
# and validates that a later snapshot matches it. CI calls `capture` before
# the upgrade and `validate` after it, so any state drift fails the job.
#
# Usage:
#   scripts/upgrade_state.sh capture  <snapshot-file>
#   scripts/upgrade_state.sh validate <pre-upgrade-snapshot-file>
#
# Required env: AR_ID, ER_ID, LC_ID (deployed contract ids).
# Optional env: NETWORK (default: local), SOURCE (default: deployer).
set -euo pipefail

NETWORK="${NETWORK:-local}"
SOURCE="${SOURCE:-deployer}"

MODE="${1:-}"
FILE="${2:-}"
if [[ -z "$MODE" || -z "$FILE" ]]; then
  echo "usage: $0 capture|validate <file>" >&2
  exit 2
fi

: "${AR_ID:?AR_ID must be set}"
: "${ER_ID:?ER_ID must be set}"
: "${LC_ID:?LC_ID must be set}"

# Invoke a view function and strip all whitespace so the snapshot is stable
# regardless of how the CLI pretty-prints its output.
view() {
  local contract_id="$1"
  shift
  stellar contract invoke --id "$contract_id" --network "$NETWORK" --source "$SOURCE" -- "$@" \
    | tr -d '[:space:]'
}

capture() {
  local out="$1"
  {
    echo "asset_registry.admin=$(view "$AR_ID" get_admin)"
    echo "asset_registry.asset_count=$(view "$AR_ID" asset_count)"
    echo "engineer_registry.admin=$(view "$ER_ID" get_admin)"
    echo "lifecycle.config=$(view "$LC_ID" get_config)"
  } > "$out"

  if [[ ! -s "$out" ]]; then
    echo "ERROR: snapshot $out is empty" >&2
    return 1
  fi
  echo "Captured $(wc -l < "$out") state entries to $out"
}

validate() {
  local pre="$1"
  if [[ ! -s "$pre" ]]; then
    echo "ERROR: pre-upgrade snapshot $pre is missing or empty" >&2
    return 1
  fi

  local post
  post="$(mktemp)"
  trap 'rm -f "$post"' RETURN
  capture "$post" > /dev/null

  if diff -u "$pre" "$post"; then
    echo "✓ Post-upgrade state matches the pre-upgrade snapshot"
  else
    echo "ERROR: contract state changed across the upgrade" >&2
    return 1
  fi
}

case "$MODE" in
  capture) capture "$FILE" ;;
  validate) validate "$FILE" ;;
  *)
    echo "unknown mode: $MODE (expected capture or validate)" >&2
    exit 2
    ;;
esac
