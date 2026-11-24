#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  echo "sanitize-backup.sh requires jq" >&2
  exit 127
fi

jq 'walk(
  if type == "object" then
    with_entries(select(
      .key | test("(?i)(password|passwd|secret|token|api[_-]?key|private[_-]?key|mnemonic|seed[_-]?phrase)") | not
    ))
  else
    .
  end
)'
