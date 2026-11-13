//! [`Account`] as the core's account plugin.

use super::*;
use crate::contract::method;
use crate::contract::platform::LoginItemState;
use agent_relay_node::relay::client::WorkspaceInfo;
use agent_relay_node::relay::contract::{answer, Done, MethodSpec};
use agent_relay_node::relay::daemon::{AccountPlugin, CallContext, MethodExtension};
use agent_relay_node::relay::outbound::{Outbound, OutboundError, Request};
use agent_relay_node::relay::protocol::RpcError;
use agent_relay_node::relay::workspace::Workspace;
use futures_util::future::BoxFuture;
use serde_json::Value;

/// This computer's MCP setup, with a development build's private profiles,
/// the teammate-context setting kept beside it, and the guidance it adds to
/// every session's start through the core.
pub(super) fn mcp_installer(
    config: &agent_relay_node::relay::daemon::Config,
    plugins: &mut agent_relay_node::relay::daemon::Plugins,
) -> Arc<crate::mcp_settings::Installer> {
    let installer = provider_installer(config, &plugins.sessions);
    plugins
        .settings
        .push(Arc::new(crate::teammate_context::Setting::new(installer.clone())));
    plugins.session_start_context = Some(Arc::new(crate::teammate_context::Context::new(
        installer.clone(),
    )));
    installer
}

fn provider_installer(
    config: &agent_relay_node::relay::daemon::Config,
    sessions: &Arc<dyn agent_relay_node::relay::sessions::SessionDirectory>,
) -> Arc<crate::mcp_settings::Installer> {
    use agent_relay_node::relay::local::{is_development, local_config};
    let development = is_development(config);
    let clients = development.then(|| crate::mcp_settings::development::Clients::new(config));
    let client_home = clients
        .as_ref()
        .map_or(&config.home, |clients| &clients.home);
    let client_env = clients.as_ref().map_or(&config.env, |clients| &clients.env);
    let env = |key: &str| client_env.get(key).cloned();
    let local = local_config(config, client_home, &env);
    let installer = crate::mcp_settings::Installer::new(
        client_home.clone(),
        local.codex.clone(),
        local.claude.clone(),
        local.opencode.clone(),
    )
    .with_environment(&env)
    .with_choices_path(config.app_data_dir.join("tool-integrations.json"))
    .with_live_reload();
    let installer = Arc::new(if development {
        installer.with_development(
            &config.app_data_dir,
            &config.paths.socket,
            &std::env::current_exe().unwrap_or_default(),
        )
    } else {
        installer
            .with_copilot_executable(std::env::current_exe().ok())
            .with_core_sockets(config)
    });
    sessions.configure_copilot_helper(installer.copilot_helper());
    installer
}

/// Where this computer's agents keep what proves a session's owner, as
/// discovery reads it; `None` where a development build's client profiles are
/// not private, and nothing reads them.
pub(super) fn session_location_config(
    config: &agent_relay_node::relay::daemon::Config,
) -> Option<(agent_relay_node::relay::local::LocalConfig, PathBuf)> {
    let profile = client_profile(config);
    if profile.as_ref().is_some_and(|profile| !profile.safe) {
        return None;
    }
    let client_home = profile
        .as_ref()
        .map_or(&config.home, |profile| &profile.home);
    let client_env = profile.as_ref().map_or(&config.env, |profile| &profile.env);
    let env = |key: &str| client_env.get(key).cloned();
    let local = agent_relay_node::relay::local::local_config(config, client_home, &env);
    Some((local, crate::history::sources::probe_root(config)))
}

struct Plugin(Arc<Account>);

impl Account {
    /// Cloud's answer to an identity sync changed what it supports.
    pub(crate) fn native_capability_changed(&self, core: &Arc<Core>) {
        if let Some(installer) = self.mcp_config.get() {
            let core = Arc::downgrade(core);
            let copilot = core.clone();
            installer.set_copilot_attestation_source(Arc::new(move || {
                copilot.upgrade().is_some_and(|core| {
                    crate::desktop_identity::Identities::new(core).copilot_attestation_verified()
                })
            }));
            installer.set_claude_attestation_source(Arc::new(move || {
                core.upgrade().is_some_and(|core| {
                    crate::desktop_identity::Identities::new(core).claude_attestation_verified()
                })
            }));
        }
    }

    /// This account as the core carries it.
    pub fn into_plugin(self: Arc<Self>) -> Arc<dyn AccountPlugin> {
        Arc::new(Plugin(self))
    }
}

/// The methods this account answers, as a build registers them.
pub struct AccountMethods(pub Arc<Account>);

impl MethodExtension for AccountMethods {
    fn specs(&self) -> Vec<MethodSpec> {
        crate::contract::account_specs()
    }

    fn call<'a>(
        &'a self,
        core: &'a Arc<Core>,
        // The bridge admits only this user's processes; the service-caller
        // methods also check who the peer is (`docs/service-callers.md` section 6).
        context: &'a CallContext,
        method: &'a str,
        raw: Value,
    ) -> BoxFuture<'a, Option<Result<Value, RpcError>>> {
        Box::pin(async move {
            if method.starts_with("spawn_callers.") {
                return self
                    .0
                    .service_callers
                    .callers
                    .call(context, method, raw)
                    .await;
            }
            if let Some(answer) = crate::provider_methods::call(core, method, &raw).await {
                return Some(answer);
            }
            if let Some(answer) = crate::cloud_chat::call(core, method, raw.clone()).await {
                return Some(answer);
            }
            let contractors = self.0.contractors(core);
            let contractor_answer =
                crate::contractor_access::call(core, &contractors, method, raw.clone()).await;
            if let Some(answer) = contractor_answer {
                return Some(answer);
            }
            if let Some(answer) = login_item_call(core, method, raw.clone()).await {
                return Some(answer);
            }
            crate::account_methods::account_call(core, &self.0, method, raw).await
        })
    }

    /// A bridge connection ended: the service callers release what it held
    /// (`docs/service-callers-auth.md`, `connection_closed`).
    fn connection_closed(&self, context: &CallContext) {
        self.0.service_callers.callers.connection_closed(context);
    }
}

async fn login_item_call(
    core: &Arc<Core>,
    method: &str,
    raw: Value,
) -> Option<Result<Value, RpcError>> {
    let config = core.config();
    match method {
        method::LoginItemSet::NAME => Some(
            answer::<method::LoginItemSet, _>(raw, |request| async move {
                crate::login_item::set(&config.home, &config.env, request.enabled).map(|_| Done {})
            })
            .await,
        ),
        method::LoginItemStatus::NAME => Some(
            answer::<method::LoginItemStatus, _>(raw, |_| async {
                crate::login_item::status(&config.home, &config.env)
                    .map(|enabled| LoginItemState { enabled })
            })
            .await,
        ),
        _ => None,
    }
}

impl AccountPlugin for Plugin {
    fn attach(&self, core: &Arc<Core>) {
        Account::attach(&self.0, core);
    }

    fn run(&self) -> BoxFuture<'static, ()> {
        Box::pin(Account::run(self.0.clone()))
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(Account::shutdown(&self.0))
    }

    fn report(&self) -> Value {
        serde_json::to_value(Account::report(&self.0)).unwrap_or_default()
    }

    fn maintain(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(core) = self.0.core() {
                crate::desktop_identity::Identities::new(core)
                    .refresh_mcp_links()
                    .await;
            }
            self.0.refresh_provider_credentials(Vec::new(), false).await;
            // At startup (the first pass) and whenever an install attaches,
            // the registered names reach session history without waiting
            // for the agents to change. Unchanged names write nothing.
            if let Some(core) = self.0.core() {
                let uploader = self.0.uploader.clone();
                tokio::spawn(async move {
                    crate::history::registered_names::publish(&core, &uploader).await
                });
            }
        })
    }

    fn recover_workspace_key<'a>(
        &'a self,
        refused: &'a Workspace,
        workspace_id: &'a str,
    ) -> BoxFuture<'a, Option<(Workspace, WorkspaceInfo)>> {
        Box::pin(Account::recover_workspace_key(
            &self.0,
            refused,
            workspace_id,
        ))
    }

    fn agents_changed(&self, core: &Arc<Core>) {
        let identities = crate::desktop_identity::Identities::new(core.clone());
        tokio::spawn(async move { identities.sync_desktop_identities().await });
        // The registered names reach session history, for "@agent" search.
        let (core, uploader) = (core.clone(), self.0.uploader.clone());
        tokio::spawn(
            async move { crate::history::registered_names::publish(&core, &uploader).await },
        );
    }

    fn mcp_linked_at(&self, agent_id: &str) -> Option<String> {
        crate::desktop_identity::Identities::new(self.0.core()?).mcp_linked_at(agent_id)
    }

    fn cloud_identity_sync(&self, core: &Core) -> Value {
        serde_json::to_value(super::cloud_identity_sync(core)).unwrap_or_default()
    }

    fn refresh_tools(&self, core: &Core) -> Vec<String> {
        let Some(installer) = self.0.mcp_config.get() else {
            return Vec::new();
        };
        let status = core.status().account;
        let binding = (|| {
            if status.state != agent_relay_node::relay::state::SignInState::SignedIn {
                return None;
            }
            Some(crate::mcp_settings::Binding {
                site_url: status.site_url.clone()?,
                workspace_id: status.workspace_id.clone()?,
                account_id: status.person.as_ref()?.id.clone()?,
            })
        })();
        installer.refresh(binding.as_ref())
    }

    fn session_route<'a>(
        &'a self,
        outbound: &'a Outbound,
        pid: i32,
        uid: u32,
        request: &'a Request,
    ) -> Option<BoxFuture<'a, Result<Value, OutboundError>>> {
        if request.path == "/copilot/hook" {
            let input: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
            if input["relayEvent"] == "before" {
                return Some(Box::pin(crate::copilot_attestation::route(
                    outbound,
                    self.0.clone(),
                    pid,
                    uid,
                    input,
                )));
            }
        }
        if request.path == crate::copilot_dispatch::ROUTE && request.method == "POST" {
            return Some(Box::pin(crate::copilot_dispatch::route(
                self.0.clone(),
                outbound,
                pid,
                uid,
                request,
            )));
        }
        if request.path != crate::claude_attestation::ROUTE {
            return None;
        }
        let key = self
            .0
            .attestation_key
            .get(&outbound.core.config.app_data_dir);
        // The hook this build installed names its own server's tools.
        let server = self.0.mcp_config.get().map_or_else(
            || crate::claude_attestation::SERVER.to_string(),
            |installer| installer.server().to_string(),
        );
        Some(Box::pin(crate::attestation_route::route(
            outbound, key, server, pid, uid, request,
        )))
    }

    fn site_url(&self) -> String {
        Account::site_url(&self.0).to_string()
    }

    /// Turning Claude Code cloud sessions on here, beside the text that
    /// explains macOS's prompt, is the one moment the app asks for Keychain
    /// access.
    fn person_changed_settings(&self, changes: &serde_json::Map<String, Value>) {
        use agent_relay_node::relay::claude_dirs::{cloud_sessions_on_here, CLOUD_SETTING};
        if changes.get(CLOUD_SETTING) == Some(&Value::Bool(true)) {
            crate::claude_keychain::ask_access(cloud_sessions_on_here);
        }
    }

    fn record_usage(&self, event: agent_relay_node::relay::usage::UsageEvent) {
        if let Some(telemetry) = &self.0.telemetry {
            telemetry.record(event.into());
        }
    }

    /// The desktop's sessions join the relay by themselves until the person
    /// turns that off (`auto_activate`), as the desktop's own core always
    /// treated an unset setting. Pi, Oh My Pi and OpenCode still need their
    /// own opt-in, which the desktop's core never gave them either.
    fn auto_activate_by_default(&self) -> bool {
        true
    }

    fn as_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::{daemon::Config, paths::CorePaths, sessions::NoSessions};

    /// The wiring the core runs: a site profile build's helper and hook both
    /// name its own sockets, under its own entry name.
    #[test]
    fn a_profile_builds_installer_names_its_own_sockets() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("person");
        let short = if cfg!(windows) {
            std::env::temp_dir()
        } else {
            PathBuf::from("/tmp")
        };
        let socket = short
            .join(format!("ar-p0-plugin-{}", std::process::id()))
            .join("core.devaws.sock");
        let config = Config {
            home: home.clone(),
            app_data_dir: home.join("com.agentrelay.desktop.devaws"),
            paths: CorePaths::for_socket(socket.clone()).unwrap(),
            env: [("XDG_RUNTIME_DIR".to_string(), "/run/user/4242".to_string())].into(),
            version: "test".into(),
            host_name: "test".into(),
            rediscover_every: std::time::Duration::from_secs(3600),
            push_timing: Default::default(),
            delivery_timing: Default::default(),
        };
        let sessions: Arc<dyn agent_relay_node::relay::sessions::SessionDirectory> =
            Arc::new(NoSessions);
        let installer = provider_installer(&config, &sessions);
        let binding = crate::mcp_settings::Binding {
            site_url: "https://agentrelay.com".into(),
            workspace_id: "workspace".into(),
            account_id: "account".into(),
        };
        let (helper, hook, server) = installer.commands(&binding);
        let helper = helper.unwrap();
        assert!(helper.contains(" --socket "), "{helper}");
        assert!(helper.ends_with("core.devaws.sock") || helper.ends_with("core.devaws.sock'"));
        let hook = hook.unwrap();
        assert!(
            hook.contains(" claude-attestation-hook --socket "),
            "{hook}"
        );
        assert_eq!(server, "agent-relay-devaws");
    }
}
