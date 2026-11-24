#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT="$(mktemp)"
trap 'rm -f "$OUTPUT"' EXIT

bash "$ROOT/scripts/sanitize-backup.sh" \
  < "$SCRIPT_DIR/fixtures/sensitive-backup.json" \
  > "$OUTPUT"

for secret in \
  fixture-api-key-secret \
  fixture-password-secret \
  fixture-private-key-secret \
  fixture-seed-phrase-secret \
  fixture-refresh-token-secret; do
  if grep -Fq "$secret" "$OUTPUT"; then
    echo "Sensitive fixture value leaked: $secret" >&2
    exit 1
  fi
done

jq -e '
  def has_sensitive_fields:
    if type == "object" then
      any(to_entries[]; (.key | test("(?i)(password|passwd|secret|token|api[_-]?key|private[_-]?key|mnemonic|seed[_-]?phrase)")) or (.value | has_sensitive_fields))
    elif type == "array" then
      any(.[]; has_sensitive_fields)
    else
      false
    end;

  (has_sensitive_fields | not)
  and .version == 1
  and .account.name == "sample-account"
  and .account.profile.region == "eu-west"
  and .wallet != null
  and .sessions[0].status == "active"
' "$OUTPUT" > /dev/null

echo "Backup sanitizer test passed."
