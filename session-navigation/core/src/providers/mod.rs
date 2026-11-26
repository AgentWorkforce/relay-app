//! Provider policy, one module per provider behind one trait. Adding a
//! provider means adding a module and one entry in [`POLICIES`].
pub mod claude;
pub mod codex;
pub mod grok;
pub mod opencode;

use crate::decide::Context;
use crate::model::{Component, Provider, Reason};
use crate::plan::{Decision, SessionState};

pub trait ProviderPolicy: Sync {
    fn provider(&self) -> Provider;
    /// Versions each assumption was verified against (from the facts files).
    fn verified_versions(&self) -> &'static [(Component, &'static str)];
    fn decide(&self, cx: &Context<'_>) -> Decision;
}

pub static POLICIES: [&dyn ProviderPolicy; 4] = [
    &claude::ClaudePolicy,
    &codex::CodexPolicy,
    &opencode::OpenCodePolicy,
    &grok::GrokPolicy,
];

struct Missing;

impl ProviderPolicy for Missing {
    fn provider(&self) -> Provider {
        Provider::Claude
    }
    fn verified_versions(&self) -> &'static [(Component, &'static str)] {
        &[]
    }
    fn decide(&self, _: &Context<'_>) -> Decision {
        Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
    }
}

static MISSING: Missing = Missing;

pub fn policy_for(provider: Provider) -> &'static dyn ProviderPolicy {
    POLICIES
        .iter()
        .copied()
        .find(|p| p.provider() == provider)
        .unwrap_or(&MISSING)
}

/// Evidence of the wrong provider shape: refuse rather than guess.
pub(crate) fn mismatched() -> Decision {
    Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
}
