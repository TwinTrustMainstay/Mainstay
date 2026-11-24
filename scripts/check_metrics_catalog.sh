#!/usr/bin/env bash
# Validates that the metrics catalog (monitoring/metrics-catalog.yml) and the
# code that emits Mainstay metrics stay in sync:
#   1. every catalogued metric is emitted by some script, and
#   2. every mainstay_* metric emitted by a script is catalogued.
#
# Usage: scripts/check_metrics_catalog.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CATALOG="$REPO_ROOT/monitoring/metrics-catalog.yml"

if [[ ! -f "$CATALOG" ]]; then
  echo "ERROR: missing $CATALOG" >&2
  exit 1
fi

# Sources that emit metrics. Keep in sync with the `source:` fields in the catalog.
EMITTER_GLOBS=("scripts/monitoring/*.sh" "scripts/finops/*.py")

catalogued=$(grep -E '^\s*- name: ' "$CATALOG" | sed -E 's/^\s*- name: //' | sort -u)
emitted=""
for glob in "${EMITTER_GLOBS[@]}"; do
  for file in $REPO_ROOT/$glob; do
    [[ -f "$file" ]] || continue
    emitted+=$'\n'"$(grep -ohE 'mainstay_[a-z_]+' "$file" || true)"
  done
done
emitted=$(printf '%s\n' "$emitted" | sed '/^$/d' | sort -u)

violations=0

# 1. Catalogued but never emitted
while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  if ! grep -qxF "$name" <<< "$emitted"; then
    echo "ERROR: catalogued metric '$name' is not emitted by any script" >&2
    violations=$((violations + 1))
  fi
done <<< "$catalogued"

# 2. Emitted but not catalogued
while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  if ! grep -qxF "$name" <<< "$catalogued"; then
    echo "ERROR: emitted metric '$name' is missing from monitoring/metrics-catalog.yml" >&2
    violations=$((violations + 1))
  fi
done <<< "$emitted"

if [[ "$violations" -gt 0 ]]; then
  echo "Metrics catalog check failed with $violations violation(s)." >&2
  exit 1
fi

echo "Metrics catalog is in sync ($(grep -c . <<< "$catalogued") catalogued metrics)."
