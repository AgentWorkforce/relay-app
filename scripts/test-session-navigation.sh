#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test --locked --manifest-path desktop-core/Cargo.toml --lib session_locations
cargo test --locked --manifest-path desktop-core/Cargo.toml --test session_navigation_contract
cargo test --locked --manifest-path gtk/Cargo.toml --no-default-features --lib session_navigation

if [[ "$(uname -s)" == Darwin ]]; then
  swift test --package-path macos --filter 'Session(Location|Plan|Terminal|Opening)|CoreClientTests'
fi
