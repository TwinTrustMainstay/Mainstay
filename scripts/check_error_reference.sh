#!/usr/bin/env bash
# Validates that every ContractError variant defined in source has a matching
# entry in docs/error-reference.md. Fails CI if a variant is added to a
# contract's `enum ContractError` without a corresponding doc entry.
#
# Usage: scripts/check_error_reference.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC_FILE="${REPO_ROOT}/docs/error-reference.md"

ERROR_SOURCE_FILES=(
  "contracts/asset-registry/src/lib.rs"
  "contracts/asset-registry/src/errors.rs"
  "contracts/engineer-registry/src/lib.rs"
  "contracts/engineer-registry/src/errors.rs"
  "contracts/lifecycle/src/lib.rs"
  "contracts/lifecycle/src/errors.rs"
  "contracts/lending/src/lib.rs"
  "contracts/lending/src/errors.rs"
)

missing=0

for src in "${ERROR_SOURCE_FILES[@]}"; do
  path="${REPO_ROOT}/${src}"
  [ -f "$path" ] || continue

  # Extract variant names from `VariantName = <number>,` lines inside ContractError enums.
  variants=$(grep -oE '^\s*[A-Za-z][A-Za-z0-9]* = [0-9]+,' "$path" | sed -E 's/^\s*([A-Za-z][A-Za-z0-9]*) = .*/\1/' || true)

  for variant in $variants; do
    if ! grep -qE "\`${variant}\`" "$DOC_FILE"; then
      echo "MISSING: '${variant}' (from ${src}) has no entry in docs/error-reference.md"
      missing=1
    fi
  done
done

if [ "$missing" -ne 0 ]; then
  echo ""
  echo "One or more ContractError variants are undocumented. Add them to docs/error-reference.md."
  exit 1
fi

echo "All ContractError variants are documented in docs/error-reference.md."
