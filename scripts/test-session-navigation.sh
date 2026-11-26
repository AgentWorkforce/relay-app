#!/usr/bin/env bash
# Open session: the rules and adapters (session-navigation/), the app's
# layer over them (desktop-core/src/session_open), the history feed it
# reads (desktop-core/src/session_locations), and the apps' callers.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test --locked --manifest-path session-navigation/Cargo.toml --workspace
for feature in macos linux windows; do
  cargo test --locked --manifest-path session-navigation/Cargo.toml \
    -p session-navigation-native --no-default-features --features "$feature"
done
cargo test --locked --manifest-path desktop-core/Cargo.toml --lib session_open
cargo test --locked --manifest-path desktop-core/Cargo.toml --lib session_locations
cargo test --locked --manifest-path gtk/Cargo.toml --no-default-features --lib session_open

if [[ "$(uname -s)" == Darwin ]]; then
  swift test --package-path macos --filter 'Session(Location|Open)|CoreClientTests'
fi
