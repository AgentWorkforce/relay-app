//! Read-only verification with the exact scoped credentials installed in native
//! providers. This proves the configured connection, not every running chat.
use super::*;
use crate::mcp_settings::Binding;
use crate::contract::tool_integrations::ToolsReport;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Instant;

pub(super) struct NativeCheck {
    binding: Binding,
    epoch: u64,
    generation: u64,
    fingerprints: BTreeMap<String, [u8; 32]>,
    checked: Instant,
    proof: BTreeMap<String, Proof>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Proof {
    Ready,
    Rejected,
    /// The site has no hosted MCP route (`mcp_auth::hosted_mcp_absent`), so
    /// no saved connection on it can be verified; said as such, not as an
    /// outage to check again later.
    Absent,
    Unavailable,
}

fn fingerprints(tokens: &BTreeMap<String, String>) -> BTreeMap<String, [u8; 32]> {
    tokens
        .iter()
        .map(|(id, token)| (id.clone(), Sha256::digest(token.as_bytes()).into()))
        .collect()
}

fn apply(report: &mut ToolsReport, site: &str, proofs: &BTreeMap<String, Proof>) {
    for tool in &mut report.tools {
        if !tool.selected || !tool.supported || tool.state != "installed" {
            continue;
        }
        let Some(proof) = proofs.get(&tool.id) else {
            continue;
        };
        if *proof == Proof::Ready {
            tool.state = "configured".into();
            tool.status_label = "Connected".into();
            tool.detail = format!("Agent Relay authenticated with {}’s saved MCP configuration and verified the required search and messaging tools. Existing chats may need a reload or a new session to load them.", tool.name);
        } else {
            // The label the apps pass through with the core's detail (macOS
            // ProviderConnectionPresentation, GTK ui/tools/presentation);
            // the detail says what to do, or that nothing can be done here.
            tool.status_label = "Connection not verified".into();
            tool.detail = match proof {
                Proof::Rejected => "This provider’s configured Agent Relay credential was rejected. Check again or repair its connection; your saved provider choice is unchanged.".into(),
                Proof::Absent => format!(
                    "{} Your saved setup is unchanged.",
                    mcp_auth::hosted_mcp_absent_message(site)
                ),
                _ => "Agent Relay could not verify this provider’s configured MCP connection. Check again when the connection is working; your saved setup is unchanged.".into(),
            };
        }
        if cfg!(windows) && tool.id == "cursor" {
            tool.detail.push_str(" Cursor MCP setup is available; local Cursor session messaging hooks are unavailable on Windows.");
        }
    }
}

impl Account {
    pub(super) fn provider_check_current(
        &self,
        binding: &Binding,
        epoch: u64,
        generation: u64,
    ) -> bool {
        let status = self.status();
        self.epoch.load(Ordering::SeqCst) == epoch
            && self.auth_generation.load(Ordering::SeqCst) == generation
            && status.state == SignInState::SignedIn
            && status.site_url.as_deref() == Some(&binding.site_url)
            && status.workspace_id.as_deref() == Some(&binding.workspace_id)
            && status
                .person
                .as_ref()
                .and_then(|person| person.id.as_deref())
                == Some(&binding.account_id)
    }

    pub(crate) async fn verify_native_connections(
        &self,
        report: &mut ToolsReport,
        binding: Option<&Binding>,
        force: bool,
    ) {
        let Some(binding) = binding else {
            return;
        };
        if report.account_health.as_deref() != Some("ready") {
            return;
        }
        let Some(installer) = self.mcp_config.get() else {
            return;
        };
        let ids = report
            .tools
            .iter()
            .filter(|tool| tool.selected && tool.state == "installed")
            .map(|tool| tool.id.clone())
            .collect::<Vec<_>>();
        let tokens = installer.provider_probe_tokens(binding, &ids);
        if tokens.is_empty() {
            return;
        }
        let stamp = fingerprints(&tokens);
        let epoch = self.epoch.load(Ordering::SeqCst);
        let generation = self.auth_generation.load(Ordering::SeqCst);
        let mut cache = self.connection_checks.native.lock().await;
        if let Some(cached) = cache.as_ref().filter(|cached| {
            !force
                && cached.binding == *binding
                && cached.epoch == epoch
                && cached.generation == generation
                && cached.fingerprints == stamp
                && cached.checked.elapsed() < Duration::from_secs(300)
        }) {
            if self.provider_check_current(binding, epoch, generation)
                && installer.provider_probe_tokens(binding, &ids) == tokens
            {
                apply(report, &binding.site_url, &cached.proof);
            }
            return;
        }
        let proof = futures_util::future::join_all(tokens.iter().map(|(id, token)| async move {
            let credential = Secret::new(token.clone());
            let result = tokio::time::timeout(
                Duration::from_secs(8),
                mcp_auth::probe_tools(&binding.site_url, &credential),
            )
            .await;
            let proof = match result {
                Ok(Ok(())) => Proof::Ready,
                Ok(Err(CloudError::Rejected)) => Proof::Rejected,
                Ok(Err(error)) if mcp_auth::hosted_mcp_absent(&binding.site_url, &error) => {
                    Proof::Absent
                }
                _ => Proof::Unavailable,
            };
            (id.clone(), proof)
        }))
        .await
        .into_iter()
        .collect();
        // Neither an identity change nor a config/token/selection change while
        // the request was pending can make the new state look connected.
        if !self.provider_check_current(binding, epoch, generation)
            || installer.provider_probe_tokens(binding, &ids) != tokens
        {
            return;
        }
        apply(report, &binding.site_url, &proof);
        *cache = Some(NativeCheck {
            binding: binding.clone(),
            epoch,
            generation,
            fingerprints: stamp,
            checked: Instant::now(),
            proof,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_connections_do_not_claim_running_chat_readiness_or_change_choices() {
        let tool = |id: &str, selected, state: &str| json!({"id":id,"name":id,"detected":true,"selected":selected,"supported":true,"state":state,"status_label":"Setup saved","detail":"installed","can_install":false,"can_repair":true,"can_uninstall":true});
        let mut report: ToolsReport = serde_json::from_value(json!({"onboarding_complete":true,"account_health":"ready","tools":[tool("grok",true,"installed"),tool("cursor",true,"installed"),tool("devin",false,"installed"),tool("muse",true,"action_required")]})).unwrap();
        apply(
            &mut report,
            "https://gcp-proof.example",
            &BTreeMap::from([
                ("grok".into(), Proof::Ready),
                ("cursor".into(), Proof::Rejected),
                ("devin".into(), Proof::Ready),
                ("muse".into(), Proof::Ready),
            ]),
        );
        assert_eq!(report.tools[0].state, "configured");
        assert!(report.tools[0]
            .detail
            .contains("Existing chats may need a reload"));
        assert_eq!(report.tools[1].state, "installed");
        assert_eq!(report.tools[1].status_label, "Connection not verified");
        assert!(!report.tools[2].selected);
        assert_eq!(report.tools[2].state, "installed");
        assert_eq!(report.tools[3].state, "action_required");
    }

    #[test]
    fn a_site_without_hosted_mcp_is_named_instead_of_asking_to_check_again() {
        let tool = json!({"id":"grok","name":"grok","detected":true,"selected":true,"supported":true,"state":"installed","status_label":"Setup saved","detail":"installed","can_install":false,"can_repair":true,"can_uninstall":true});
        let mut report: ToolsReport = serde_json::from_value(
            json!({"onboarding_complete":true,"account_health":"ready","tools":[tool]}),
        )
        .unwrap();
        apply(
            &mut report,
            "https://gcp-proof.example",
            &BTreeMap::from([("grok".into(), Proof::Absent)]),
        );
        assert_eq!(report.tools[0].state, "installed");
        // The apps show the core's detail only under this label.
        assert_eq!(report.tools[0].status_label, "Connection not verified");
        assert!(report.tools[0]
            .detail
            .starts_with("Hosted Agent Relay MCP is not available on https://gcp-proof.example"));
        assert!(!report.tools[0].detail.contains("Check again"));
    }

    #[test]
    fn credential_rotation_invalidates_the_fingerprint() {
        let first = BTreeMap::from([("grok".into(), "fixture-one".into())]);
        let rotated = BTreeMap::from([("grok".into(), "fixture-two".into())]);
        assert_ne!(fingerprints(&first), fingerprints(&rotated));
        assert_ne!(fingerprints(&first), fingerprints(&BTreeMap::new()));
    }
}
