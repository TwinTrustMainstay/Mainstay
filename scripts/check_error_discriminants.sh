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
for contract in asset-registry engineer-registry lifecycle lending; do
  contract_path="${REPO_ROOT}/contracts/${contract}"
  [ -d "$contract_path" ] || continue

  src_path="${contract_path}/src"
  [ -d "$src_path" ] || continue

  # Scan every Rust source file under src/ for a `pub enum ContractError` definition.
  while IFS= read -r source_file; do
    [ -f "$source_file" ] || continue

    # Extract the ContractError enum section from this file
    temp_enum=$(mktemp)
    sed -n '/^pub enum ContractError/,/^}/p' "$source_file" > "$temp_enum" 2>/dev/null || true

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
          rel_file="${source_file#"${REPO_ROOT}/"}"
          echo "DUPLICATE: ${contract} has duplicate ContractError discriminant ${discriminant} in ${rel_file}: '${variant}' and '${discriminants[$discriminant]}'"
          violations=$((violations + 1))
        else
          discriminants[$discriminant]="$variant"
        fi
      done < "$temp_enum"

      unset discriminants
    fi

    rm -f "$temp_enum"
  done < <(find "$src_path" -name '*.rs' -type f | sort)
done

if [ "$violations" -ne 0 ]; then
  echo ""
  echo "Found ${violations} ContractError discriminant violation(s). Each discriminant must be unique within a contract's enum."
  exit 1
fi

echo "All ContractError discriminants are unique within each contract."
