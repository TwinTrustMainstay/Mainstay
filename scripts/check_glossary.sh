#!/usr/bin/env bash
# Validates docs/GLOSSARY.md (issue #1723):
#   1. Every required domain term has a heading entry.
#   2. Every internal glossary anchor used in the docs resolves to a heading.
# Usage: bash scripts/check_glossary.sh
set -euo pipefail

cd "$(dirname "$0")/.."
GLOSSARY="docs/GLOSSARY.md"
REQUIRED_TERMS=("Collateral" "Lien" "Covenant" "Liquidation")

if [[ ! -f "$GLOSSARY" ]]; then
  echo "ERROR: $GLOSSARY is missing" >&2
  exit 1
fi

# GitHub-style heading slug: lowercase, drop punctuation, spaces -> hyphens.
slugify() {
  echo "$1" | tr '[:upper:]' '[:lower:]' | sed -e 's/[^a-z0-9 -]//g' -e 's/ /-/g'
}

mapfile -t HEADINGS < <(grep -E '^## ' "$GLOSSARY" | sed -e 's/^## //')
SLUGS=()
for h in "${HEADINGS[@]}"; do
  SLUGS+=("$(slugify "$h")")
done

status=0

for term in "${REQUIRED_TERMS[@]}"; do
  if ! printf '%s\n' "${HEADINGS[@]}" | grep -qx "$term"; then
    echo "ERROR: required term '$term' has no entry in $GLOSSARY" >&2
    status=1
  fi
done

# Links of the form (GLOSSARY.md#anchor) from any doc.
while IFS= read -r link; do
  anchor="${link#*#}"
  if ! printf '%s\n' "${SLUGS[@]}" | grep -qx "$anchor"; then
    echo "ERROR: broken glossary anchor '#$anchor' (from: $link)" >&2
    status=1
  fi
done < <(grep -rhoE 'GLOSSARY\.md#[a-z0-9-]+' docs README.md 2>/dev/null | sort -u)

# In-glossary links of the form (#anchor).
while IFS= read -r anchor; do
  if ! printf '%s\n' "${SLUGS[@]}" | grep -qx "$anchor"; then
    echo "ERROR: broken in-glossary anchor '#$anchor'" >&2
    status=1
  fi
done < <(grep -oE '\(#[a-z0-9-]+\)' "$GLOSSARY" | sed -e 's/^(#//' -e 's/)$//' | sort -u)

if [[ $status -eq 0 ]]; then
  echo "✓ Glossary check passed (${#SLUGS[@]} terms)"
fi
exit $status
