#!/usr/bin/env bash
# Local equivalent of the CI jobs in .github/workflows/ci.yml.
#
# Usage: ./scripts/ci-local.sh
#
# Runs the same three gates CI runs, in the same order, so a green local run
# means the push should be green as well. Requires protobuf-compiler because
# rt-grpc generates its tonic bindings at build time.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo fmt --all --check"
cargo fmt --all --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace --all-targets"
cargo test --workspace --all-targets

echo "All local CI gates passed."
