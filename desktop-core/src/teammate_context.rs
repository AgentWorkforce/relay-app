//! What is left of the retired teammate-context hook, a Claude Code
//! SessionStart hook that shipped off by default in v2026.11.6.
//!
//! Turn-end coordination advisories replaced it
//! ([`crate::coordination_advisories`]). Two pieces remain for logins that
//! still carry it:
//! - every installer refresh, and the core's reclaim pass, take the hook out
//!   of every watched Claude login and delete its saved choice
//!   ([`crate::mcp_settings`], `teammate_hook.rs`);
//! - the probe still answers the subcommand, printing nothing and exiting 0,
//!   so a login not cleaned yet never sees an error or old guidance.
use std::io::Read;

/// The subcommand the hook ran; also how a leftover hook is recognized.
pub const SUBCOMMAND: &str = "teammate-context-hook";

/// `teammate-context-hook`: reads and discards Claude's hook input, prints
/// nothing, and exits 0, so a session always starts.
pub fn main(_arguments: &[String]) -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(1 << 20).read_to_end(&mut input);
    0
}

/// The retired setting as the core's reclaim pass sees it: no keys, and a
/// reclaim that takes any leftover hook out of the watched Claude logins.
pub struct LegacyCleanup {
    installer: std::sync::Arc<crate::mcp_settings::Installer>,
}

impl LegacyCleanup {
    pub(crate) fn new(installer: std::sync::Arc<crate::mcp_settings::Installer>) -> Self {
        Self { installer }
    }
}

impl agent_relay_node::relay::agents::SettingsExtension for LegacyCleanup {
    fn get(&self) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    fn set(&self, _key: &str, _value: &serde_json::Value) -> Option<Result<(), String>> {
        None
    }

    fn reclaim(&self) {
        self.installer.remove_legacy_teammate_context();
    }
}
