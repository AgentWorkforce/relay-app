# desktop-core

Agent Relay Desktop's private core layer, added to relay's public core
(`agent-relay-probe`, pinned in `Cargo.toml`) through its plugin hooks. See [docs/workspace-layout.md](../docs/workspace-layout.md#crates).

    cargo test --manifest-path desktop-core/Cargo.toml --locked
    cargo build --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe

`scripts/build-probe.sh` builds the binary and installs it as `agent-relay-probe`.
