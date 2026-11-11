#!/bin/sh
# Format check, clippy with warnings as errors, and all tests: the same as CI.
# Run it after every change; work is not done until it passes.
#
#   ./scripts/check.sh
set -e
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
