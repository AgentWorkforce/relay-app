#!/usr/bin/env bash
#
# generate-contract.sh -- write the Swift bridge contract types into the macOS
# app, from the pinned core's registry and the methods desktop-core adds:
#
#   macos/Sources/RelayDesktopCore/CoreContract.swift
#
# CI runs this and fails when `git diff --exit-code` shows the app's copy differs.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACT="$REPO_ROOT/macos/Sources/RelayDesktopCore/CoreContract.swift"

cargo run --quiet --locked --manifest-path "$REPO_ROOT/desktop-core/Cargo.toml" \
  --bin agent-relay-desktop-contract -- "$CONTRACT"
echo "wrote $CONTRACT"
