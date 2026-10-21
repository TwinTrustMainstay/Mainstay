#!/usr/bin/env bash
# Validates documentation versioning and maintenance metadata (issue #1724).
#
#   1. All contract crates and the Rust SDK share one release version (lockstep).
#   2. Every docs/*.md has a metadata line under its title:
#        > **Last updated:** YYYY-MM-DD · **Doc version:** X.Y.Z
#   3. Each doc's Doc version equals the release version.
#   4. Deprecation banners link to a replacement document.
#   5. docs/DOC_OWNERS.md exists.
#
# Usage: bash scripts/check_doc_metadata.sh
set -euo pipefail

cd "$(dirname "$0")/.."
status=0

# 1. Lockstep release version across crates.
mapfile -t CRATE_MANIFESTS < <(ls contracts/*/Cargo.toml sdk/rust/Cargo.toml)
versions=()
for manifest in "${CRATE_MANIFESTS[@]}"; do
  v=$(grep -m1 -E '^version = "' "$manifest" | sed -E 's/^version = "(.*)"/\1/')
  versions+=("$v")
done
RELEASE_VERSION="${versions[0]}"
for i in "${!versions[@]}"; do
  if [[ "${versions[$i]}" != "$RELEASE_VERSION" ]]; then
    echo "ERROR: ${CRATE_MANIFESTS[$i]} is at ${versions[$i]}, expected $RELEASE_VERSION (crates must be released in lockstep)" >&2
    status=1
  fi
done

META_RE='^> \*\*Last updated:\*\* [0-9]{4}-[0-9]{2}-[0-9]{2} · \*\*Doc version:\*\* [0-9]+\.[0-9]+\.[0-9]+$'

for doc in docs/*.md; do
  meta=$(grep -m1 -E '^> \*\*Last updated:\*\*' "$doc" || true)
  if [[ -z "$meta" ]]; then
    echo "ERROR: $doc has no metadata line (> **Last updated:** YYYY-MM-DD · **Doc version:** X.Y.Z)" >&2
    status=1
    continue
  fi
  if ! grep -qE "$META_RE" <<<"$meta"; then
    echo "ERROR: $doc has a malformed metadata line: $meta" >&2
    status=1
    continue
  fi
  doc_version="${meta##*\*\* }"
  if [[ "$doc_version" != "$RELEASE_VERSION" ]]; then
    echo "ERROR: $doc is at doc version $doc_version, expected release version $RELEASE_VERSION" >&2
    status=1
  fi

  while IFS= read -r line; do
    if ! grep -qE '\]\([^)]+\)' <<<"$line"; then
      echo "ERROR: $doc has a Deprecated banner without a link to its replacement" >&2
      status=1
    fi
  done < <(grep -E '^> \*\*Deprecated:\*\*' "$doc" || true)
done

if [[ ! -f docs/DOC_OWNERS.md ]]; then
  echo "ERROR: docs/DOC_OWNERS.md is missing" >&2
  status=1
fi

if [[ $status -eq 0 ]]; then
  echo "✓ Doc metadata check passed (release $RELEASE_VERSION, $(ls docs/*.md | wc -l) docs)"
fi
exit $status
