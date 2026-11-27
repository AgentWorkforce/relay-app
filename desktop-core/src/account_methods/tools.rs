use super::*;
use crate::account::Account;
use crate::contract::tool_integrations::ToolsReport;
use agent_relay_node::relay::contract::answer;
use crate::mcp_settings::{Binding, Installer};

fn requested_providers(
    installer: &Installer,
    action: &str,
    ids: &[String],
    tool: &str,
) -> Vec<String> {
    match action {
        "select" => installer.expand_selection(ids.to_vec()),
        "enable" => installer.expand_selection(vec![tool.into()]),
        "check" => installer.provider_ids(),
        "install" | "repair" | "reinstall" => vec![tool.into()],
        _ => Vec::new(),
    }
}

/// Run local setup work together on the blocking installer thread.
fn apply_installer_action(
    installer: &Installer,
    action: &str,
    ids: Vec<String>,
    tool: &str,
    binding: Option<&Binding>,
    health: Option<String>,
) -> Result<ToolsReport, RpcError> {
    match action {
        "select" => {
            installer.select(ids, binding, health.clone())?;
            installer.refresh(binding);
            Ok(installer.report(binding, health))
        }
        "check" => {
            if let Some(binding) = binding {
                installer.recheck_runtime(binding);
            }
            Ok(installer.report(binding, health))
        }
        "status" => Ok(installer.report(binding, health)),
        "enable" => {
            installer.manage(tool, action, binding, health.clone())?;
            if matches!(tool, "conductor" | "paseo") {
                installer.refresh(binding);
            }
            Ok(installer.report(binding, health))
        }
        _ => installer.manage(tool, action, binding, health),
    }
}

pub(super) async fn run(
    account: &Arc<Account>,
    action: &str,
    ids: Vec<String>,
    tool: String,
) -> Result<ToolsReport, RpcError> {
    let Some(installer) = account.mcp_config.get().cloned() else {
        return Ok(ToolsReport {
            onboarding_complete: true,
            account_health: Some("unavailable".into()),
            workspace_error: None,
            tools: Vec::new(),
        });
    };
    if action == "check" {
        account.recheck_auth_health().await;
    }
    if !matches!(action, "status") {
        {
            let requested = requested_providers(&installer, action, &ids, &tool);
            account.refresh_provider_credentials(requested, false).await;
        }
    }
    let snapshot = account.status();
    let preparation = (action == "status")
        .then(|| preparing_report_binding(&snapshot))
        .flatten();
    let status = Some(snapshot);
    let health = status.as_ref().and_then(|s| s.auth_health.clone());
    let workspace_error = status
        .as_ref()
        .filter(|s| s.auth_health.as_deref() == Some("workspace_unavailable"))
        .and_then(|s| s.auth_error.clone());
    let binding = status.and_then(|s| {
        if s.state != agent_relay_node::relay::state::SignInState::SignedIn {
            return None;
        }
        Some(Binding {
            site_url: s.site_url?,
            workspace_id: s.workspace_id?,
            account_id: s.person?.id?,
        })
    });
    let binding_for_external = binding.clone();
    let force_external = action == "check";
    let retry_removals = matches!(action, "uninstall" | "select");
    let changed_selection = matches!(action, "select" | "enable" | "uninstall" | "disable");
    let action = action.to_string();
    let final_installer = installer.clone();
    let final_binding = binding.clone();
    let final_health = health.clone();
    let result = tokio::task::spawn_blocking(move || {
        apply_installer_action(&installer, &action, ids, &tool, binding.as_ref(), health)
    })
    .await
    .map_err(|_| RpcError::invalid_params("Provider integration check could not complete."))?;
    // Selection changes can remove credentials. Revoke them in the same action,
    // including after a failed install, instead of waiting for maintenance.
    if changed_selection && result.is_ok() {
        account
            .refresh_provider_credentials(Vec::new(), retry_removals)
            .await;
    }
    let refreshed_installer = final_installer.clone();
    let mut report = if changed_selection && result.is_ok() {
        tokio::task::spawn_blocking(move || {
            refreshed_installer.report(final_binding.as_ref(), final_health)
        })
        .await
        .map_err(|_| RpcError::invalid_params("Provider integration check could not complete."))?
    } else { result? };
    // A short account-scoped cache keeps ordinary provider polling cheap.
    // Check connection explicitly bypasses it after external setup.
    account.verify_external_connections(&mut report, binding_for_external.as_ref(), force_external).await;
    account.verify_native_connections(&mut report, binding_for_external.as_ref(), force_external).await;
    crate::mcp_settings::derive_host_connections(&mut report.tools, report.account_health.as_deref());
    show_preparing_status(account, &final_installer, preparation, &mut report);
    report.workspace_error = workspace_error;
    Ok(report)
}

pub(super) async fn call(
    account: &Arc<Account>,
    method: &str,
    raw: &Value,
) -> Option<Result<Value, RpcError>> {
    Some(match method {
        crate::contract::method::ToolsStatus::NAME => {
            answer::<crate::contract::method::ToolsStatus, _>(raw.clone(), |_| {
                run(account, "status", Vec::new(), String::new())
            })
            .await
        }
        crate::contract::method::ToolsCheck::NAME => {
            answer::<crate::contract::method::ToolsCheck, _>(raw.clone(), |_| {
                run(account, "check", Vec::new(), String::new())
            })
            .await
        }
        crate::contract::method::ToolsSelect::NAME => {
            answer::<crate::contract::method::ToolsSelect, _>(raw.clone(), |r| async move {
                run(account, "select", r.tool_ids, String::new()).await
            })
            .await
        }
        crate::contract::method::ToolsManage::NAME => {
            answer::<crate::contract::method::ToolsManage, _>(raw.clone(), |r| async move {
                run(account, &r.action, Vec::new(), r.tool_id).await
            })
            .await
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_a_host_requests_its_detected_provider_grants() {
        let root = tempfile::tempdir().unwrap();
        let executable = root
            .path()
            .join(if cfg!(windows) { "grok.exe" } else { "grok" });
        std::fs::write(&executable, "fixture executable").unwrap();
        agent_relay_node::platform::fs::set_mode(&executable, 0o700).unwrap();
        let env = |key: &str| match key {
            "RELAY_DESKTOP_GROK" => Some(executable.to_string_lossy().into_owned()),
            "RELAY_DESKTOP_MUSE" => {
                Some(root.path().join("missing").to_string_lossy().into_owned())
            }
            _ => None,
        };
        let mut local = agent_relay_node::relay::local::LocalConfig::from_env(&env, root.path());
        local.codex.executable = None;
        local.claude.executable = None;
        let installer = Installer::new(
            root.path().into(),
            local.codex,
            local.claude,
            local.opencode,
        )
        .with_environment(&env);
        assert_eq!(
            requested_providers(&installer, "enable", &[], "paseo"),
            vec!["grok", "paseo"]
        );
        assert!(requested_providers(&installer, "disable", &[], "paseo").is_empty());
    }
}

fn show_preparing_status(
    account: &Account,
    installer: &Installer,
    preparation: Option<Binding>,
    report: &mut ToolsReport,
) {
    // This changes a read-only status report, never an installer binding,
    // credential refresh, connection probe or action authorization.
    if let Some(preparation) = preparation {
        let still_preparing = preparing_report_binding(&account.status()).is_some_and(|current| {
            current.site_url == preparation.site_url
                && current.workspace_id == preparation.workspace_id
                && current.account_id == preparation.account_id
        });
        if still_preparing && !installer.provider_login_required(&preparation) {
            for tool in &mut report.tools {
                if tool.supported && tool.state == "signed_out" {
                    tool.state = "preparing".into();
                    tool.status_label = "Preparing sessions".into();
                    tool.detail = "You’re signed in. Agent Relay is preparing your local sessions. Provider setup will continue when that finishes.".into();
                    tool.can_install = false;
                    tool.can_repair = false;
                }
            }
        }
    }
}

/// A report-only witness: this never authorizes provider setup while the
/// account is still preparing. This phase is entered only after Cloud has
/// verified the person and attached their workspace.
fn preparing_report_binding(
    status: &agent_relay_node::relay::state::AccountStatus,
) -> Option<Binding> {
    use agent_relay_node::relay::state::SignInState;
    if status.state != SignInState::Preparing
        || status.phase.as_deref() != Some("preparing_local_sessions")
        || status.error.is_some()
        || status.action.is_some()
        || status.auth_error.is_some()
        || !matches!(status.auth_health.as_deref(), None | Some("ready"))
    {
        return None;
    }
    let site_url =
        agent_relay_node::relay::workspace::validated_base_url(status.site_url.as_deref()?)?;
    let workspace_id = status.workspace_id.as_ref()?;
    let account_id = status.person.as_ref()?.id.as_ref()?;
    if !crate::cloud_api::is_workspace_uuid(workspace_id) || account_id.trim().is_empty() {
        return None;
    }
    Some(Binding {
        site_url,
        workspace_id: workspace_id.clone(),
        account_id: account_id.clone(),
    })
}

#[cfg(test)]
mod preparing_tests;
