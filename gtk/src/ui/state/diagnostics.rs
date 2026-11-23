//! Account and provider failures shared by the app status and Diagnostics.
use super::*;
use crate::models::{diagnostics_sections, CoreCrates, DiagnosticsInput, DiagnosticsSection};

impl State {
    pub fn account(&self) -> AccountStatus {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.account.clone())
            .unwrap_or_default()
    }

    pub fn status_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for problem in [
            self.session_error.clone(),
            self.core_error.clone(),
            self.page_error.clone(),
            self.relay_problem(),
            self.upload_action_error.clone(),
            self.status
                .as_ref()
                .filter(|status| {
                    status.stage == agent_relay_desktop_core::contract::account::Stage::NeedsAttention
                })
                .map(|status| {
                    status
                        .detail
                        .clone()
                        .unwrap_or_else(|| status.headline.clone())
                }),
            self.relay_load_error.clone(),
        ]
        .into_iter()
        .flatten()
        .chain(self.upload_errors.values().cloned())
        .chain(self.account_provider_problems())
        {
            if !problem.is_empty() && !problems.contains(&problem) {
                problems.push(problem);
            }
        }
        problems
    }

    /// About's allowlisted diagnostics, with why the last connection link
    /// failed (`DesktopDiagnostics` on macOS adds the same row).
    pub fn diagnostics(&self, platform: &str) -> Vec<DiagnosticsSection> {
        let mut sections = diagnostics_sections(DiagnosticsInput {
            snapshot: self.snapshot.as_ref(),
            connected: self.core_connected,
            app_version: agent_relay_node::VERSION,
            crates: CoreCrates::built().as_ref(),
            platform,
            service: self
                .service_state
                .map_or("Unknown", |service| service.label()),
            upload_headline: self.status.as_ref().map(|status| status.headline.as_str()),
            zone: &chrono::Local,
        });
        if let Some(connections) = sections
            .iter_mut()
            .find(|section| section.title == "Connections")
        {
            connections.rows.push((
                "Last link failure",
                self.link_error.clone().unwrap_or_else(|| "None".into()),
            ));
        }
        sections
    }

    pub fn account_provider_problems(&self) -> Vec<String> {
        [
            self.account_health_error.clone(),
            self.account().auth_error,
            self.tools.error.clone(),
        ]
        .into_iter()
        .flatten()
        .chain(
            self.tools
                .report
                .iter()
                .flat_map(|report| &report.tools)
                .filter(|tool| tool.selected && tool.state == "action_required")
                .map(|tool| format!("{}: {}", tool.name, tool.detail)),
        )
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_desktop_core::contract::tool_integrations::ToolsReport;

    #[test]
    fn account_and_selected_provider_failures_reach_diagnostics() {
        let mut state = State::new(InstallKind::Source);
        state.snapshot = Some(
            serde_json::from_value(crate::fake_core::snapshot(
                serde_json::json!({"auth_error": "Account verification failed"}),
            ))
            .unwrap(),
        );
        state.account_health_error = Some("Health check failed".into());
        state.tools.error = Some("Repair failed".into());
        state.tools.report = Some(
            serde_json::from_value::<ToolsReport>(serde_json::json!({
                "onboarding_complete": true,
                "account_health": "ready",
                "tools": [
                    provider("Codex", true, "action_required"),
                    provider("Claude", false, "action_required"),
                    provider("Other", true, "configured")
                ]
            }))
            .unwrap(),
        );
        assert_eq!(
            state.status_problems(),
            [
                "Health check failed",
                "Account verification failed",
                "Repair failed",
                "Codex: Needs repair"
            ]
        );
    }

    #[test]
    fn a_failed_link_reaches_diagnostics_only() {
        let mut state = State::new(InstallKind::Source);
        let report = |state: &State| crate::models::diagnostics_report(&state.diagnostics("Linux"));
        assert!(report(&state).contains("Last link failure: None"));
        state.link_error = Some("Agent Relay is starting. Try again in a moment.".into());
        // The page error says what the person sees; the cause is for
        // Diagnostics, as on macOS.
        assert!(state.status_problems().is_empty());
        assert!(report(&state)
            .contains("Last link failure: Agent Relay is starting. Try again in a moment."));
    }

    fn provider(name: &str, selected: bool, state: &str) -> serde_json::Value {
        serde_json::json!({"id": name, "name": name, "detected": true, "supported": true,
            "selected": selected, "state": state, "status_label": "Needs attention",
            "detail": "Needs repair", "can_install": false, "can_repair": true, "can_uninstall": true})
    }
}
