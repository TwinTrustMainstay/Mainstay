#!/usr/bin/env bash
set -euo pipefail

# Repo root = parent of scripts/
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# rustup/cargo on macOS/Linux (non-login shells)
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

echo "Running Mainstay tests (workspace) from $ROOT ..."
cargo test --workspace "$@"

# Event streaming resume-from-state test (stream-events.sh)
if [[ -f "$ROOT/scripts/stream-events.sh" ]]; then
  echo "Running stream-events.sh resume test ..."
  bash "$ROOT/scripts/test-stream-events.sh"
fi

echo "All tests passed."
