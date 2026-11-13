//! Optional sync extension. Old Cloud routes reject unknown fields, so only
//! advertise native chat targets, and the native attestation key, after this
//! exact account generation opts in.
use super::DesktopIdentitySync;
use super::Identities;
use agent_relay_node::relay::{
    cloud_session::CloudAccount,
    store::{SessionKind, Target},
};
use serde_json::{json, Value};

#[derive(PartialEq)]
pub(super) struct Capability {
    account: CloudAccount,
    generation: u64,
    supported: bool,
    provider_supported: bool,
    claude_attestation: bool,
    copilot_attestation: bool,
}
/// Which native session kinds Cloud accepts for this account generation.
#[derive(Clone, Copy, Default)]
pub(super) struct Support {
    pub(super) codex: bool,
    pub(super) providers: bool,
    /// Claude Code chats, whose calls carry this installation's proof.
    pub(super) claude: bool,
    pub(super) copilot: bool,
}
impl DesktopIdentitySync {
    pub(super) fn native_providers_supported(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        self.native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|c| {
                c.account == *account && c.generation == generation && c.provider_supported
            })
    }
    /// Cloud verifies Claude chat proofs (`claude_session_attestation`).
    pub(crate) fn claude_attestation_supported(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        self.native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|c| {
                c.account == *account && c.generation == generation && c.claude_attestation
            })
    }
    /// Cloud verifies Copilot chat proofs for this exact account generation.
    pub(crate) fn copilot_attestation_supported(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        self.native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|c| {
                c.account == *account && c.generation == generation && c.copilot_attestation
            })
    }
    pub(super) fn native_supported(&self, account: &CloudAccount, generation: u64) -> bool {
        self.native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|c| c.account == *account && c.generation == generation && c.supported)
    }
    /// Whether a capability is recorded for this account generation.
    pub(super) fn capability_known(&self, account: &CloudAccount, generation: u64) -> bool {
        self.native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|c| c.account == *account && c.generation == generation)
    }
    /// Records what Cloud advertises; whether that changed what is known.
    pub(super) fn record_native_capability(
        &self,
        account: &CloudAccount,
        generation: u64,
        response: &Value,
    ) -> bool {
        let recorded = Some(Capability {
            account: account.clone(),
            generation,
            supported: response["capabilities"]["native_session_identity"].as_u64() == Some(1),
            provider_supported: response["capabilities"]["provider_native_session_identity"]
                .as_u64()
                == Some(1),
            copilot_attestation: response["capabilities"]["copilot_session_attestation"].as_u64()
                == Some(1),
            claude_attestation: response["capabilities"]["claude_session_attestation"].as_u64()
                == Some(1),
        });
        let mut capability = self
            .native_capability
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let changed = *capability != recorded;
        *capability = recorded;
        changed
    }
}
impl Identities {
    /// This installation's key for Cloud to verify proofs with, when it has one.
    fn attestation_registration(&self) -> Option<Value> {
        let key = self
            .account
            .as_ref()?
            .attestation_key
            .get(&self.config.app_data_dir)?;
        Some(key.registration())
    }

    /// The native kinds Cloud accepts for this account generation, and the
    /// attestation key it verifies Claude and Copilot proofs with. Those chats
    /// are named only together with that key.
    pub(super) fn native_support(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> (Support, Option<Value>) {
        let sync = &self.state;
        let copilot = sync.copilot_attestation_supported(account, generation);
        let key = (sync.claude_attestation_supported(account, generation) || copilot)
            .then(|| self.attestation_registration())
            .flatten();
        let support = Support {
            codex: sync.native_supported(account, generation),
            providers: sync.native_providers_supported(account, generation),
            claude: key.is_some() && sync.claude_attestation_supported(account, generation),
            copilot: key.is_some() && copilot,
        };
        (support, key)
    }

    /// Cloud verifies Claude chat proofs for the account signed in now and
    /// its current identity-sync generation. Signing and provider status
    /// both ask this, so a switched account or generation reads false until
    /// Cloud advertises the capability for it.
    pub fn claude_attestation_verified(&self) -> bool {
        let cloud = &self.plugins.cloud;
        cloud.account().is_some_and(|account| {
            self.state
                .claude_attestation_supported(&account, cloud.identity_sync_generation())
        })
    }

    /// Copilot proofs are enabled only for the current account generation.
    pub fn copilot_attestation_verified(&self) -> bool {
        let cloud = &self.plugins.cloud;
        cloud.account().is_some_and(|account| {
            self.state
                .copilot_attestation_supported(&account, cloud.identity_sync_generation())
        })
    }

    /// Records what Cloud's identity read advertises, and points provider
    /// setup's Claude status at [`Core::claude_attestation_verified`].
    pub(super) fn record_native_capability(
        &self,
        account: &CloudAccount,
        generation: u64,
        response: &Value,
    ) {
        let changed = self
            .state
            .record_native_capability(account, generation, response);
        if let (true, Some(account)) = (changed, &self.account) {
            account.native_capability_changed(&self.core);
        }
    }
}

/// The sync body with the installation's attestation key, when Cloud
/// verifies Claude proofs.
pub(super) fn with_attestation_key(mut body: Value, key: Option<&Value>) -> Value {
    if let Some(key) = key {
        body["attestation_key"] = key.clone();
    }
    body
}

pub(super) fn attach(identity: &mut Value, target: &Target, support: Support) {
    // These native identifiers come from verified runtime records, never tool
    // arguments. OpenClaw agents and Hermes profiles are not chat identities.
    let kind = match target.kind {
        SessionKind::Codex if support.codex => Some("codex"),
        SessionKind::ClaudeTerminal if support.claude => Some("claude"),
        SessionKind::Copilot if support.copilot => Some("copilot"),
        SessionKind::Pi if support.providers => Some("pi"),
        SessionKind::Grok | SessionKind::GrokOpen if support.providers => Some("grok"),
        SessionKind::OpenCode | SessionKind::OpenCodeOpen if support.providers => Some("opencode"),
        _ => None,
    };
    if let Some(kind) = kind.filter(|_| target.is_valid()) {
        let id = if kind == "opencode" {
            target.id.clone()
        } else {
            target.id.to_ascii_lowercase()
        };
        identity["native_session"] = json!({"kind":kind, "id":id});
    }
    // A claude.ai/code session's hosted MCP names its container's session
    // id, so Cloud links it to this identity without a `join_relay`.
    if let Some(id) = agent_relay_node::relay::claude_web::web_session_id(target) {
        identity["web_session_id"] = json!(id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn support(codex: bool, providers: bool) -> Support {
        Support {
            codex,
            providers,
            claude: false,
            copilot: false,
        }
    }
    #[test]
    fn capability_never_crosses_account_origin_workspace_or_generation() {
        let sync = DesktopIdentitySync::default();
        let account = CloudAccount {
            site_url: "https://cloud.test".into(),
            workspace_id: "workspace".into(),
            user_id: Some("alice".into()),
        };
        sync.record_native_capability(
            &account,
            7,
            &json!({"capabilities":{"native_session_identity":1,"provider_native_session_identity":1,"copilot_session_attestation":1}}),
        );
        assert!(sync.native_supported(&account, 7));
        assert!(sync.native_providers_supported(&account, 7));
        assert!(sync.copilot_attestation_supported(&account, 7));
        assert!(!sync.native_supported(&account, 8));
        assert!(!sync.native_providers_supported(&account, 8));
        assert!(!sync.copilot_attestation_supported(&account, 8));
        for other in [
            CloudAccount {
                user_id: Some("bob".into()),
                ..account.clone()
            },
            CloudAccount {
                site_url: "https://other.test".into(),
                ..account.clone()
            },
            CloudAccount {
                workspace_id: "other".into(),
                ..account.clone()
            },
        ] {
            assert!(!sync.native_supported(&other, 7));
            assert!(!sync.native_providers_supported(&other, 7));
            assert!(!sync.copilot_attestation_supported(&other, 7));
        }
        assert!(!sync.claude_attestation_supported(&account, 7));
        sync.record_native_capability(
            &account,
            7,
            &json!({"capabilities":{"claude_session_attestation":1}}),
        );
        assert!(sync.claude_attestation_supported(&account, 7));
        assert!(!sync.copilot_attestation_supported(&account, 7));
        assert!(!sync.claude_attestation_supported(&account, 8));
        assert!(!sync.native_supported(&account, 7));
        sync.record_native_capability(&account, 7, &json!({"identities":[]}));
        assert!(!sync.native_supported(&account, 7));
        assert!(!sync.native_providers_supported(&account, 7));
        assert!(!sync.claude_attestation_supported(&account, 7));
    }

    #[test]
    fn claude_chats_name_their_session_only_with_attestation_support() {
        let id = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA";
        let claude = Target::new(SessionKind::ClaudeTerminal, id);
        let mut value = json!({});
        attach(&mut value, &claude, support(true, true));
        assert!(value.get("native_session").is_none());
        let with_claude = Support {
            claude: true,
            ..Support::default()
        };
        attach(&mut value, &claude, with_claude);
        assert_eq!(
            value["native_session"],
            json!({"kind":"claude","id":id.to_ascii_lowercase()})
        );
        for target in [
            Target::new(SessionKind::ClaudeTerminal, "not-a-session"),
            Target::new(SessionKind::ClaudeCloud, "session_01abc"),
            Target::new(SessionKind::Codex, id),
        ] {
            let mut value = json!({});
            attach(&mut value, &target, with_claude);
            assert!(value.get("native_session").is_none(), "{target:?}");
        }
    }
    #[test]
    fn only_verified_supported_native_targets_are_advertised_after_negotiation() {
        let target = Target::new(SessionKind::Codex, "00000000-0000-4000-8000-000000000001");
        let mut old = json!({"agent_id":"immutable"});
        attach(&mut old, &target, support(false, false));
        assert!(old.get("native_session").is_none());
        attach(&mut old, &target, support(true, false));
        assert_eq!(
            old["native_session"],
            json!({"kind":"codex","id":target.id.to_ascii_lowercase()})
        );
        for target in [
            Target::new(SessionKind::Codex, "unverified"),
            Target::new(SessionKind::Pi, &target.id),
            Target::new(SessionKind::ClaudeTerminal, &target.id),
        ] {
            let mut value = json!({});
            attach(&mut value, &target, support(true, false));
            assert!(value.get("native_session").is_none());
        }
    }

    #[test]
    fn copilot_native_identity_requires_its_own_attestation_capability() {
        let target = Target::new(SessionKind::Copilot, "1069773c-0824-4fad-b7f9-d3dc3c4ed148");
        let mut identity = json!({});
        attach(&mut identity, &target, support(true, true));
        assert!(identity.get("native_session").is_none());
        attach(
            &mut identity,
            &target,
            Support {
                copilot: true,
                ..support(false, false)
            },
        );
        assert_eq!(
            identity["native_session"],
            json!({"kind":"copilot","id":target.id})
        );
    }

    #[test]
    fn provider_native_ids_need_separate_negotiation_and_verified_target_shapes() {
        let id = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA";
        for (target_kind, native_kind) in [
            (SessionKind::Pi, "pi"),
            (SessionKind::Grok, "grok"),
            (SessionKind::GrokOpen, "grok"),
        ] {
            let target = Target::new(target_kind, id);
            let mut value = json!({});
            attach(&mut value, &target, support(true, false));
            assert!(value.get("native_session").is_none());
            attach(&mut value, &target, support(true, true));
            assert_eq!(
                value["native_session"],
                json!({"kind":native_kind,"id":id.to_ascii_lowercase()})
            );
            let mut invalid = json!({});
            attach(
                &mut invalid,
                &Target::new(target_kind, "model-supplied-name"),
                support(true, true),
            );
            assert!(invalid.get("native_session").is_none());
        }
        for kind in [SessionKind::OpenCode, SessionKind::OpenCodeOpen] {
            let target = Target::new(kind, "ses_AbCd1234");
            let mut value = json!({});
            attach(&mut value, &target, support(true, false));
            assert!(value.get("native_session").is_none());
            attach(&mut value, &target, support(true, true));
            assert_eq!(
                value["native_session"],
                json!({"kind":"opencode","id":"ses_AbCd1234"})
            );
        }
        for target in [
            Target::new(SessionKind::OpenClaw, "main"),
            Target::new(SessionKind::Hermes, "default"),
            Target::new(SessionKind::ClaudeTerminal, id),
        ] {
            let mut value = json!({});
            attach(&mut value, &target, support(true, true));
            assert!(value.get("native_session").is_none());
        }
    }
}
