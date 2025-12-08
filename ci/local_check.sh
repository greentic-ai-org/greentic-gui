#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
npm run build-sdk --if-present
npm run test-sdk --if-present
