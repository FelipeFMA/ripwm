#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is not installed or not in PATH" >&2
  exit 1
fi

echo "==> Running format check"
cargo fmt --all --check

echo "==> Running clippy (deny warnings)"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> Running cargo check"
cargo check --all-targets --all-features

echo "✅ Local CI checks passed"
