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
echo "All tests passed."
