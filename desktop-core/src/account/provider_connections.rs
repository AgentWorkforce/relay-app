//! Read-only proof of the person's external provider connection in this workspace.
use super::*;
use crate::mcp_settings::Binding;
use agent_relay_node::relay::cloud_session::CloudAccess;
use crate::contract::tool_integrations::ToolsReport;
use std::time::{Duration, Instant};

mod native;

#[derive(Default)]
pub(super) struct Checks {
    external: tokio::sync::Mutex<Option<ExternalCheck>>,
    native: tokio::sync::Mutex<Option<native::NativeCheck>>,
}

pub(super) struct ExternalCheck {
    binding: Binding,
    epoch: u64,
    generation: u64,
    checked: Instant,
    proof: ExternalProof,
    capy_api: Option<(Instant, CapyProof)>,
}

#[derive(Clone, Copy)]
struct ExternalProof {
    capy: CapyProof,
    grok_bot: GrokBotProof,
    dot: DotProof,
    amp: AmpProof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AmpProof {
    Connected,
    NotBound,
    Unavailable,
}

/// Cloud lists a bound Amp thread among the workspace's agents with session
/// kind amp, online while its relay status is active, and the owner its
/// binding verifies to. Only a live thread of this person's proves Amp is on:
/// a bound but idle thread, a teammate's thread, or an agent merely named Amp
/// does not. Amp is never among the connectors.
fn amp_proof(value: &Value, account_id: &str) -> AmpProof {
    if value["unavailable"].as_bool() != Some(false) {
        return AmpProof::Unavailable;
    }
    let Some(agents) = value["agents"].as_array() else {
        return AmpProof::Unavailable;
    };
    if agents.iter().any(|agent| {
        agent["sessionKind"].as_str() == Some("amp")
            && agent["online"].as_bool() == Some(true)
            && agent["owner"]["id"].as_str() == Some(account_id)
    }) {
        AmpProof::Connected
    } else {
        AmpProof::NotBound
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DotProof {
    Connected,
    NotConnected,
    Unavailable,
}

/// Cloud returns only this person's connectors for the requested workspace.
/// A live OAuth connection proves plugin sign-in even before chat subscriptions
/// are configured. Installed apps and self-declared agent names do not.
fn dot_proof(value: &Value) -> DotProof {
    if value["unavailable"].as_bool() != Some(false) {
        return DotProof::Unavailable;
    }
    let Some(connectors) = value["connectors"].as_array() else {
        return DotProof::Unavailable;
    };
    if connectors.iter().any(|connector| {
        connector["kind"].as_str() == Some("chatgpt")
            && connector["signedIn"].as_bool() == Some(true)
    }) {
        DotProof::Connected
    } else {
        DotProof::NotConnected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapyProof {
    Connected,
    WorkspaceConnected,
    WorkspaceMissing,
    NoActiveThread,
    Unavailable,
}

/// Ownership comes from Cloud's verified binding lookup. A teammate's thread,
/// a name containing Capy, or an unavailable/partial roster is not proof.
fn capy_proof(value: &Value, account_id: &str) -> CapyProof {
    if value["unavailable"].as_bool() != Some(false) {
        return CapyProof::Unavailable;
    }
    let Some(agents) = value["agents"].as_array() else {
        return CapyProof::Unavailable;
    };
    if agents.iter().any(|agent| {
        agent["sessionKind"].as_str() == Some("capy")
            && agent["sessionId"].as_str().is_some_and(|id| {
                id.strip_prefix("jam_").is_some_and(|suffix| {
                    (6..=64).contains(&suffix.len())
                        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
            })
            && agent["online"].as_bool() == Some(true)
            && agent["owner"]["id"].as_str() == Some(account_id)
    }) {
        CapyProof::Connected
    } else {
        CapyProof::NoActiveThread
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GrokBotProof {
    Active(usize),
    Inactive,
    Unavailable,
}

/// This endpoint scopes connectors to the authenticated person and workspace.
/// Cloud derives their kind from OAuth and subscriptions, and ownsAgent from
/// the current relay binding. A matching name or installed app proves nothing.
fn grok_bot_proof(value: &Value) -> GrokBotProof {
    if value["unavailable"].as_bool() != Some(false) {
        return GrokBotProof::Unavailable;
    }
    let Some(connectors) = value["connectors"].as_array() else {
        return GrokBotProof::Unavailable;
    };
    let active = connectors
        .iter()
        .filter(|connector| {
            connector["kind"].as_str() == Some("grok-bot")
                && connector["ownsAgent"].as_bool() == Some(true)
                && connector["status"]["tone"].as_str() == Some("online")
        })
        .filter_map(|connector| connector["agentName"].as_str())
        .filter(|name| !name.trim().is_empty())
        .collect::<std::collections::HashSet<_>>()
        .len();
    if active > 0 {
        GrokBotProof::Active(active)
    } else {
        GrokBotProof::Inactive
    }
}

fn capy_api_proof(response: Result<Value, CloudError>) -> CapyProof {
    match response {
        Ok(value) if value["projects"].is_array() => CapyProof::WorkspaceConnected,
        Err(CloudError::Status { status: 409, .. }) => CapyProof::WorkspaceMissing,
        _ => CapyProof::Unavailable,
    }
}

impl ExternalProof {
    fn apply(self, report: &mut ToolsReport) {
        if let Some(tool) = report.tools.iter_mut().find(|tool| tool.id == "capy") {
            let (state, label, detail) = match self.capy {
            CapyProof::Connected => ("configured", "Connected", "Your Capy thread is connected to Agent Relay in this workspace. New threads use the Agent Relay plugin to connect too."),
            CapyProof::WorkspaceConnected => ("configured", "Connected", "This workspace’s saved Capy connection was verified through Capy’s API. No active thread is currently verified; your existing Capy setup remains available."),
            CapyProof::WorkspaceMissing => ("external_setup", "Workspace connection not configured", "No active Capy thread is verified and this workspace has no saved Capy API connection. Existing per-thread setup may still be in place. Connect Capy in Agent Relay Cloud or start a connected thread, then check again."),
            CapyProof::NoActiveThread => ("external_setup", "No active Capy thread", "No active Capy thread was verified for your account in this workspace. Your saved Capy setup may still be in place. Start a thread with the Agent Relay plugin, then check again."),
            CapyProof::Unavailable => ("external_setup", "Connection not verified", "Agent Relay could not check your Capy connection. Check again when the connection is working; your saved Capy setup is unchanged."),
        };
            tool.state = state.into();
            tool.status_label = label.into();
            tool.detail = detail.into();
        }
        if let Some(tool) = report.tools.iter_mut().find(|tool| tool.id == "grok-bot") {
            let (state, label, detail) = match self.grok_bot {
                GrokBotProof::Active(count) => ("configured", "Connected", if count == 1 {
                    "1 Grok Bot is active on Agent Relay in this workspace.".into()
                } else {
                    format!("{count} Grok Bots are active on Agent Relay in this workspace.")
                }),
                GrokBotProof::Inactive => ("external_setup", "No active Grok Bot", "No active Grok Bot was verified for your account in this workspace. Your saved setup may still be in place. Open Grok Bot to connect or resume a bot, then check again.".into()),
                GrokBotProof::Unavailable => ("external_setup", "Connection not verified", "Agent Relay could not check your Grok Bot connection. Check again when the connection is working; your saved setup is unchanged.".into()),
            };
            tool.state = state.into();
            tool.status_label = label.into();
            tool.detail = detail;
        }
        if let Some(tool) = report.tools.iter_mut().find(|tool| tool.id == "dot") {
            let (state, label, detail) = match self.dot {
                DotProof::Connected => ("configured", "Connected", "Your Agent Relay plugin is connected in this workspace."),
                DotProof::NotConnected => ("external_setup", "Plugin not connected", "No signed-in Agent Relay plugin connection was verified for your account in this workspace. Open the plugin page to connect, then check again."),
                DotProof::Unavailable => ("external_setup", "Connection not verified", "Agent Relay could not check your plugin connection. Check again when the connection is working; your saved setup is unchanged."),
            };
            tool.state = state.into();
            tool.status_label = label.into();
            tool.detail = detail.into();
        }
        // Amp has no Check action: without a live thread of this person's, or
        // when Cloud cannot be asked, its row keeps the core's wording
        // (Connects through Agent Relay Cloud) and stays external_setup.
        if let Some(tool) = report.tools.iter_mut().find(|tool| tool.id == "amp") {
            if self.amp == AmpProof::Connected {
                tool.state = "configured".into();
                tool.status_label = "Connected in Agent Relay Cloud".into();
                tool.detail = "Agent Relay Cloud reports one of your bound Amp threads as live in this workspace. Amp's token stays in Agent Relay Cloud; Agent Relay Desktop never asks for or stores it.".into();
            }
        }
    }
}

// The optional proof deadline must not cancel an account refresh after Cloud
// consumes its rotating token. Dropping this owned task's handle detaches it;
// renewal and credential persistence finish even if the caller times out.
async fn capy_workspace_proof(account: Arc<Account>, workspace_id: &str) -> CapyProof {
    let path = format!("/workspaces/{}/capy/projects",
        agent_relay_node::relay::cloud_session::segment(workspace_id));
    let task = tokio::spawn(async move {
        CloudAccess::request(account.as_ref(), "GET", path, None).await
    });
    match tokio::time::timeout(Duration::from_secs(4), task).await {
        Ok(Ok(response)) => capy_api_proof(response),
        _ => CapyProof::Unavailable,
    }
}

impl Account {
    pub(crate) async fn verify_external_connections(
        self: &Arc<Self>,
        report: &mut ToolsReport,
        binding: Option<&Binding>,
        force: bool,
    ) {
        if !report
            .tools
            .iter()
            .any(|tool| matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot" | "amp"))
        {
            return;
        }
        let Some(binding) = binding else { return };
        if report.account_health.as_deref() != Some("ready") {
            return;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let generation = self.auth_generation.load(Ordering::SeqCst);
        let mut cache = self.connection_checks.external.lock().await;
        let mut capy_api = cache
            .as_ref()
            .filter(|cached| {
                cached.binding == *binding
                    && cached.epoch == epoch
                    && cached.generation == generation
            })
            .and_then(|cached| cached.capy_api);
        let cached_proof = cache
            .as_ref()
            .filter(|cached| {
                !force
                    && cached.binding == *binding
                    && cached.epoch == epoch
                    && cached.generation == generation
                    && cached.checked.elapsed() < Duration::from_secs(10)
            })
            .map(|cached| cached.proof);
        let proof = if let Some(proof) = cached_proof {
            proof
        } else {
            let response = CloudAccess::request(
                self.as_ref(),
                "GET",
                format!(
                    "/workspaces/{}/sessions/agents",
                    agent_relay_node::relay::cloud_session::segment(&binding.workspace_id)
                ),
                None,
            )
            .await;
            let current = self.status();
            if self.epoch.load(Ordering::SeqCst) != epoch
                || self.auth_generation.load(Ordering::SeqCst) != generation
                || current.state != SignInState::SignedIn
                || current.site_url.as_deref() != Some(&binding.site_url)
                || current.workspace_id.as_deref() != Some(&binding.workspace_id)
                || current
                    .person
                    .as_ref()
                    .and_then(|person| person.id.as_deref())
                    != Some(&binding.account_id)
            {
                return;
            }
            let proof = response
                .as_ref()
                .map(|value| ExternalProof {
                    capy: capy_proof(value, &binding.account_id),
                    grok_bot: grok_bot_proof(value),
                    dot: dot_proof(value),
                    amp: amp_proof(value, &binding.account_id),
                })
                .unwrap_or(ExternalProof {
                    capy: CapyProof::Unavailable,
                    grok_bot: GrokBotProof::Unavailable,
                    dot: DotProof::Unavailable,
                    amp: AmpProof::Unavailable,
                });
            let mut proof = proof;
            if proof.capy == CapyProof::NoActiveThread {
                let checked =
                    capy_api.filter(|(at, _)| !force && at.elapsed() < Duration::from_secs(300));
                proof.capy = if let Some((_, proof)) = checked {
                    proof
                } else {
                    let outcome = capy_workspace_proof(self.clone(), &binding.workspace_id).await;
                    capy_api = Some((Instant::now(), outcome));
                    outcome
                };
            }
            if !self.provider_check_current(binding, epoch, generation) {
                return;
            }
            *cache = Some(ExternalCheck {
                binding: binding.clone(),
                epoch,
                generation,
                checked: Instant::now(),
                proof,
                capy_api,
            });
            proof
        };
        if !self.provider_check_current(binding, epoch, generation) {
            return;
        }
        proof.apply(report);
        // Remote evidence never changes local installer selection or eligibility.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_proof_updates_cloud_rows_without_changing_local_installer_choices() {
        let tools = ["capy", "grok-bot", "dot"].map(|id| json!({"id":id,"name":id,"detected":false,"selected":false,"supported":false,"state":"external_setup","status_label":"Cloud integration","detail":"setup","can_install":false,"can_repair":false,"can_uninstall":false}));
        let mut report: ToolsReport = serde_json::from_value(
            json!({"onboarding_complete":true,"account_health":"ready","tools":tools}),
        )
        .unwrap();
        ExternalProof {
            capy: CapyProof::Connected,
            grok_bot: GrokBotProof::Active(2),
            dot: DotProof::Connected,
            amp: AmpProof::Unavailable,
        }
        .apply(&mut report);
        for tool in &report.tools {
            assert_eq!(tool.state, "configured");
            assert_eq!(tool.status_label, "Connected");
            assert!(!tool.selected && !tool.detected && !tool.supported && !tool.can_install);
        }
        ExternalProof {
            capy: CapyProof::Unavailable,
            grok_bot: GrokBotProof::Inactive,
            dot: DotProof::NotConnected,
            amp: AmpProof::Unavailable,
        }
        .apply(&mut report);
        assert_eq!(report.tools[0].status_label, "Connection not verified");
        assert_eq!(report.tools[1].status_label, "No active Grok Bot");
        assert_eq!(report.tools[2].status_label, "Plugin not connected");
        assert!(report
            .tools
            .iter()
            .all(|tool| tool.state == "external_setup" && !tool.selected));
    }

    #[test]
    fn remote_proof_turns_amp_on_only_for_a_live_thread_of_yours() {
        let amp = json!({"id":"amp","name":"Amp","detected":false,"selected":false,"supported":false,"state":"external_setup","status_label":"Connects through Agent Relay Cloud","detail":"setup","can_install":false,"can_repair":false,"can_uninstall":false});
        let mut report: ToolsReport = serde_json::from_value(
            json!({"onboarding_complete":true,"account_health":"ready","tools":[amp]}),
        )
        .unwrap();
        let proof = |amp| ExternalProof {
            capy: CapyProof::Unavailable,
            grok_bot: GrokBotProof::Unavailable,
            dot: DotProof::Unavailable,
            amp,
        };
        for amp in [AmpProof::NotBound, AmpProof::Unavailable] {
            proof(amp).apply(&mut report);
            let row = &report.tools[0];
            assert_eq!(row.state, "external_setup", "{amp:?}");
            assert_eq!(
                row.status_label, "Connects through Agent Relay Cloud",
                "{amp:?}"
            );
            assert_eq!(row.detail, "setup", "{amp:?}");
        }
        proof(AmpProof::Connected).apply(&mut report);
        let row = &report.tools[0];
        assert_eq!(row.state, "configured");
        assert_eq!(row.status_label, "Connected in Agent Relay Cloud");
        assert!(row.detail.contains("bound Amp threads as live"));
        assert!(row.detail.contains("never asks for or stores it"));
        assert!(!row.selected && !row.detected && !row.supported && !row.can_install);
    }

    #[test]
    fn amp_requires_own_live_bound_thread() {
        let body = json!({"unavailable":false,"connectors":[],"agents":[{"name":"amp-thread","host":"Amp","sessionKind":"amp","displaySessionId":"T-1234","sessionId":null,"online":true,"owner":{"id":"me","name":"Me","avatarUrl":null}}]});
        assert_eq!(amp_proof(&body, "me"), AmpProof::Connected);
        assert_eq!(amp_proof(&body, "teammate"), AmpProof::NotBound);
        let mut wrong = body.clone();
        wrong["agents"][0]["sessionKind"] = json!("codex");
        assert_eq!(
            amp_proof(&wrong, "me"),
            AmpProof::NotBound,
            "named Amp, another kind"
        );
        wrong = body.clone();
        wrong["agents"][0].as_object_mut().unwrap().remove("owner");
        assert_eq!(amp_proof(&wrong, "me"), AmpProof::NotBound);
        wrong = body.clone();
        wrong["agents"][0]["owner"] = json!(null);
        assert_eq!(amp_proof(&wrong, "me"), AmpProof::NotBound);
        wrong = body.clone();
        wrong["agents"][0]["online"] = json!(false);
        assert_eq!(
            amp_proof(&wrong, "me"),
            AmpProof::NotBound,
            "bound but not live"
        );
        wrong = body.clone();
        wrong["agents"][0].as_object_mut().unwrap().remove("online");
        assert_eq!(amp_proof(&wrong, "me"), AmpProof::NotBound);
        // Amp is never a connector; a connector of that kind proves nothing.
        assert_eq!(
            amp_proof(
                &json!({"unavailable":false,"agents":[],"connectors":[{"kind":"amp","signedIn":true}]}),
                "me"
            ),
            AmpProof::NotBound
        );
        wrong = body;
        wrong["unavailable"] = json!(true);
        assert_eq!(amp_proof(&wrong, "me"), AmpProof::Unavailable);
        for unavailable in [
            json!({"agents":[]}),
            json!({"unavailable":false}),
            json!({"unavailable":false,"agents":{}}),
        ] {
            assert_eq!(amp_proof(&unavailable, "me"), AmpProof::Unavailable);
        }
    }

    #[test]
    fn dot_uses_live_plugin_sign_in_without_claiming_message_delivery() {
        let mut body = json!({"unavailable":false,"connectors":[{"kind":"chatgpt", "signedIn":true, "ownsAgent":false, "status":{"tone":"waiting"}}]});
        assert_eq!(dot_proof(&body), DotProof::Connected);
        body["connectors"][0]["signedIn"] = json!(false);
        assert_eq!(dot_proof(&body), DotProof::NotConnected);
        body["connectors"][0]["signedIn"] = json!(true);
        body["connectors"][0]["kind"] = json!("grok-bot");
        assert_eq!(dot_proof(&body), DotProof::NotConnected);
        assert_eq!(
            dot_proof(&json!({"unavailable":false,"connectors":[]})),
            DotProof::NotConnected
        );
        assert_eq!(
            dot_proof(&json!({"unavailable":false})),
            DotProof::Unavailable
        );
        body["unavailable"] = json!(true);
        assert_eq!(dot_proof(&body), DotProof::Unavailable);
    }

    #[test]
    fn grok_bot_requires_owned_waking_connectors_on_the_current_relay() {
        let connected = json!({"unavailable":false,"connectors":[{
            "kind":"grok-bot", "agentName":"my-bot", "ownsAgent":true,
            "status":{"tone":"online"}
        }]});
        assert_eq!(grok_bot_proof(&connected), GrokBotProof::Active(1));
        for (key, value) in [
            ("kind", json!("mcp")),
            ("ownsAgent", json!(false)),
            ("agentName", json!("")),
            ("status", json!({"tone":"waiting"})),
            ("status", json!({"tone":"attention"})),
        ] {
            let mut wrong = connected.clone();
            wrong["connectors"][0][key] = value;
            assert_eq!(grok_bot_proof(&wrong), GrokBotProof::Inactive, "{key}");
        }
        let mut missing_owner = connected.clone();
        missing_owner["connectors"][0]
            .as_object_mut()
            .unwrap()
            .remove("ownsAgent");
        assert_eq!(grok_bot_proof(&missing_owner), GrokBotProof::Inactive);
        let mut duplicate = connected.clone();
        duplicate["connectors"]
            .as_array_mut()
            .unwrap()
            .push(connected["connectors"][0].clone());
        assert_eq!(grok_bot_proof(&duplicate), GrokBotProof::Active(1));
        duplicate["connectors"][1]["agentName"] = json!("other-bot");
        assert_eq!(grok_bot_proof(&duplicate), GrokBotProof::Active(2));
        assert_eq!(
            grok_bot_proof(&json!({"unavailable":false,"connectors":[]})),
            GrokBotProof::Inactive
        );
        for unavailable in [
            json!({}),
            json!({"unavailable":false}),
            json!({"unavailable":false,"connectors":{}}),
        ] {
            assert_eq!(grok_bot_proof(&unavailable), GrokBotProof::Unavailable);
        }
        let mut unavailable = connected;
        unavailable["unavailable"] = json!(true);
        assert_eq!(grok_bot_proof(&unavailable), GrokBotProof::Unavailable);
    }

    #[test]
    fn capy_workspace_setup_is_verified_without_an_active_thread() {
        assert_eq!(
            capy_api_proof(Ok(json!({"projects":[]}))),
            CapyProof::WorkspaceConnected
        );
        assert_eq!(
            capy_api_proof(Ok(json!({"unexpected":[]}))),
            CapyProof::Unavailable
        );
        assert_eq!(
            capy_api_proof(Err(CloudError::Status {
                status: 409,
                message: None
            })),
            CapyProof::WorkspaceMissing
        );
        assert_eq!(
            capy_api_proof(Err(CloudError::Status {
                status: 403,
                message: None
            })),
            CapyProof::Unavailable
        );
    }

    #[test]
    fn capy_requires_own_verified_active_thread() {
        let body = json!({"unavailable":false,"agents":[{"name":"capy-thread","sessionKind":"capy","sessionId":"jam_thread","online":true,"owner":{"id":"me"}}]});
        assert_eq!(capy_proof(&body, "me"), CapyProof::Connected);
        assert_eq!(capy_proof(&body, "teammate"), CapyProof::NoActiveThread);
        let mut wrong = body.clone();
        wrong["agents"][0]["sessionKind"] = json!("codex");
        assert_eq!(capy_proof(&wrong, "me"), CapyProof::NoActiveThread);
        wrong = body.clone();
        wrong["agents"][0].as_object_mut().unwrap().remove("owner");
        assert_eq!(capy_proof(&wrong, "me"), CapyProof::NoActiveThread);
        wrong = body.clone();
        wrong["agents"][0]["online"] = json!(false);
        assert_eq!(capy_proof(&wrong, "me"), CapyProof::NoActiveThread);
        wrong = body;
        wrong["unavailable"] = json!(true);
        assert_eq!(capy_proof(&wrong, "me"), CapyProof::Unavailable);
        assert_eq!(
            capy_proof(&json!({"agents":[]}), "me"),
            CapyProof::Unavailable
        );
    }
}
