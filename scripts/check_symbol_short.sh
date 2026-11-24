#!/usr/bin/env bash
# Validates that all symbol_short! usages in the codebase have string literals
# with 9 or fewer characters, as enforced by Soroban at runtime.
# Fails CI if any violations are found.
#
# Usage: scripts/check_symbol_short.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Track violations
violations=0

# Find all Rust source files and check for symbol_short! violations
while IFS= read -r file; do
  # Extract all symbol_short!("...") usages
  # Pattern matches: symbol_short!("string_literal")
  matches=$(grep -oE 'symbol_short!\("([^"]+)"\)' "$file" || true)

  while IFS= read -r match; do
    # Skip empty lines
    [ -z "$match" ] && continue

    # Extract the string content (between quotes)
    # From: symbol_short!("example")
    # Extract: example
    str=$(echo "$match" | sed -E 's/symbol_short!\("([^"]+)"\)/\1/')

    # Count characters in the string
    len=${#str}

    # Check if length exceeds 9 characters
    if [ "$len" -gt 9 ]; then
      echo "VIOLATION: symbol_short!(\"${str}\") has ${len} characters (max 9) in ${file}"
      violations=$((violations + 1))
    fi
  done <<< "$matches"
done < <(find "$REPO_ROOT/contracts" -name "*.rs" -type f)

if [ "$violations" -ne 0 ]; then
  echo ""
  echo "Found ${violations} symbol_short! violation(s). All symbol_short!() literals must be 9 characters or fewer."
  exit 1
fi

echo "All symbol_short!() usages are within the 9-character limit."
