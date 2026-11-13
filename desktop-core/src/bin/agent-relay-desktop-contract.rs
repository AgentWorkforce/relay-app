//! Writes the macOS app's Swift contract types: the pinned core's registry and
//! the methods this build adds (`scripts/generate-contract.sh`).

use agent_relay_desktop_core::contract::all_specs;
use agent_relay_node::relay::contract::codegen::{Contract, REGENERATE};

fn main() -> std::io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .expect("agent-relay-desktop-contract writes to the path it is given");
    let swift = Contract::collect(&all_specs())
        .swift()
        .replace(REGENERATE, "scripts/generate-contract.sh")
        .replace(
            "(crates/relay-node/src/relay/contract)",
            "(relay's crates/relay-node/src/relay/contract and desktop-core/src/contract.rs)",
        );
    std::fs::write(path, swift)
}
