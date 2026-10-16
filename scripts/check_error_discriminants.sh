#!/usr/bin/env bash
# Validates that all ContractError enum discriminants are unique within each contract.
# Duplicate discriminants are an error that can lead to silent bugs at runtime.
# Fails CI if any duplicate discriminants are found.
#
# Usage: scripts/check_error_discriminants.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

violations=0

# Process each contract
for contract in asset-registry engineer-registry lifecycle lending mobile-api zk-privacy; do
  contract_path="${REPO_ROOT}/contracts/${contract}"
  [ -d "$contract_path" ] || continue

  # Look for ContractError enum discriminants
  # We'll use a temporary file to extract just the ContractError enum section
  temp_enum=$(mktemp)
  trap "rm -f '$temp_enum'" EXIT

  # Extract the ContractError enum section from lib.rs
  if [ -f "$contract_path/src/lib.rs" ]; then
    # Use sed to extract lines between "enum ContractError" and the closing brace
    sed -n '/^pub enum ContractError/,/^}/p' "$contract_path/src/lib.rs" > "$temp_enum" 2>/dev/null || true

    # Extract all discriminant assignments: VariantName = number,
    if [ -s "$temp_enum" ]; then
      declare -A discriminants

      while IFS= read -r line; do
        # Skip lines that don't have discriminant assignments
        if [[ ! "$line" =~ ^[[:space:]]+[A-Za-z][A-Za-z0-9]*[[:space:]]*=[[:space:]]*[0-9]+, ]]; then
          continue
        fi

        # Extract variant name and discriminant value
        variant=$(echo "$line" | sed -E 's/^[[:space:]]+([A-Za-z][A-Za-z0-9]*)[[:space:]]*=.*/\1/')
        discriminant=$(echo "$line" | sed -E 's/^[[:space:]]+[A-Za-z][A-Za-z0-9]*[[:space:]]*=([[:space:]]*[0-9]+).*/\1/' | tr -d ' ')

        # Check for duplicates
        if [[ -v discriminants[$discriminant] ]]; then
          echo "DUPLICATE: ${contract} has duplicate ContractError discriminant ${discriminant}: '${variant}' and '${discriminants[$discriminant]}'"
          violations=$((violations + 1))
        else
          discriminants[$discriminant]="$variant"
        fi
      done < "$temp_enum"

      unset discriminants
    fi
  fi
done

if [ "$violations" -ne 0 ]; then
  echo ""
  echo "Found ${violations} ContractError discriminant violation(s). Each discriminant must be unique within a contract's enum."
  exit 1
fi

echo "All ContractError discriminants are unique within each contract."
