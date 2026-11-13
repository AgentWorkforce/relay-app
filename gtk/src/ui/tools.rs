//! Explicit hosted tool selection and per-tool management for Linux and Windows.

mod detail;
mod external;
mod presentation;
pub(super) mod provider_status;
mod status_reads;
pub(crate) mod teammate;
mod view;

use gtk4::prelude::WidgetExt;

use super::app::App;
use super::state::{Page, Pane, Section};
use agent_relay_desktop_core::contract::tool_integrations::{
    ToolsManageParams, ToolsReport, ToolsSelectParams,
};
use agent_relay_node::relay::contract::NoParams;
use agent_relay_node::relay::state::SignInState;
use std::collections::BTreeSet;

pub(super) fn visible_providers(
    report: &ToolsReport,
) -> Vec<&agent_relay_desktop_core::contract::tool_integrations::ToolIntegration> {
    sorted_providers(report, true)
}

pub(super) fn available_providers(
    report: &ToolsReport,
) -> Vec<&agent_relay_desktop_core::contract::tool_integrations::ToolIntegration> {
    sorted_providers(report, false)
}

/// Providers that are added explicitly and never auto-shown. Detection on
/// this computer (VS Code for GitHub Copilot) does not put them in the
/// sidebar: they stay under Available providers until the person adds them
/// there or chooses them during onboarding.
/// The Mac applies the same list in `ToolsModel.addedExplicitly`.
const ADDED_EXPLICITLY: &[&str] = &["copilot"];

/// Cloud providers the sidebar shows once Agent Relay Cloud reports their
/// connection configured. Amp is one of them: the core never reports Amp as
/// detected, so no local Amp install or folder can show it; it appears once
/// it is added or once Cloud reports its connection.
/// The Mac applies the same list in `ToolsModel.cloudConfigured`.
const CLOUD_CONFIGURED: &[&str] = &["capy", "grok-bot", "dot", "amp"];

/// The sidebar shows a provider when it is selected, when Agent Relay Cloud
/// reports a Cloud provider's connection configured, or when it is detected
/// here and is not one of the providers that are added explicitly.
fn is_visible(
    tool: &agent_relay_desktop_core::contract::tool_integrations::ToolIntegration,
) -> bool {
    (tool.detected && !ADDED_EXPLICITLY.contains(&tool.id.as_str()))
        || tool.selected
        || (CLOUD_CONFIGURED.contains(&tool.id.as_str()) && tool.state == "configured")
}

fn sorted_providers(
    report: &ToolsReport,
    visible: bool,
) -> Vec<&agent_relay_desktop_core::contract::tool_integrations::ToolIntegration> {
    let mut tools: Vec<_> = report
        .tools
        .iter()
        .filter(|tool| is_visible(tool) == visible)
        .collect();
    tools.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    tools
}

type Identity = (u64, Option<String>, Option<String>, Option<String>);

fn chooser_eligible(
    tool: &agent_relay_desktop_core::contract::tool_integrations::ToolIntegration,
) -> bool {
    // These are Cloud handoffs, never local MCP checkbox defaults.
    !matches!(tool.id.as_str(), "amp" | "capy")
        && tool.detected
        && (tool.supported || matches!(tool.id.as_str(), "conductor" | "paseo"))
}

/// What one provider's row in the onboarding chooser offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OnboardingAction {
    /// A checkbox: the provider is set up on this computer when chosen.
    Checkbox,
    /// The Cloud provider's own setup action (Capy, Grok Bot, Dot, Muse).
    CloudSetup(crate::models::CloudProviderSetup),
    /// Open Cloud connections on the signed-in site (Amp).
    OpenCloudConnections,
    /// Nothing to choose: listed only because it was detected here.
    NoChoice,
}

/// What a provider's onboarding chooser row offers: a checkbox for a local
/// choice, the Cloud provider's setup, or, for Amp, Open Cloud connections.
/// Amp has no checkbox: its token lives on the Cloud workspace, never here.
/// The Mac decides the same in `ToolsModel.onboardingAction(for:)`.
pub(super) fn onboarding_action(
    tool: &agent_relay_desktop_core::contract::tool_integrations::ToolIntegration,
) -> OnboardingAction {
    if let Some(setup) = crate::models::CloudProviderSetup::for_id(&tool.id) {
        return OnboardingAction::CloudSetup(setup);
    }
    if tool.id == "amp" {
        return OnboardingAction::OpenCloudConnections;
    }
    if chooser_eligible(tool) {
        OnboardingAction::Checkbox
    } else {
        OnboardingAction::NoChoice
    }
}

/// The onboarding chooser lists detected providers and Cloud setup actions.
/// Amp/Capy require the core's local-install witness, regardless of saved
/// choice/connection. The Mac uses the same `ToolsModel.onboardingTools`.
pub(super) fn onboarding_tools(
    report: &ToolsReport,
) -> impl Iterator<Item = &agent_relay_desktop_core::contract::tool_integrations::ToolIntegration> {
    report
        .tools
        .iter()
        .filter(|tool| crate::models::provider_onboarding_present(tool)
            && (tool.detected || onboarding_action(tool) != OnboardingAction::NoChoice))
}

#[derive(Clone, Default)]
pub struct ToolsUi {
    pub setup_started: BTreeSet<String>,
    pub report: Option<ToolsReport>,
    pub selected: BTreeSet<String>,
    pub busy: bool,
    pub checking: bool,
    pub last_checked: Option<String>,
    operation_revision: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub dismissed: bool,
    pub onboarding_requested: bool,
    retained_onboarding: BTreeSet<String>,
    pub provider_id: Option<String>,
    pub provider_overview: bool,
    sidebar_report: Option<serde_json::Value>,
    polling: bool,
    identity: Option<Identity>,
    draft_initialized: bool,
    /// The person's tick or untick of the chooser's teammate-context line
    /// since it opened; `None` follows the saved state the report gives.
    pub teammate_draft: Option<bool>,
}

impl ToolsUi {
    fn read(&mut self, report: ToolsReport) {
        if !self.onboarding_requested { self.retained_onboarding.clear(); }
        if !self.draft_initialized || (report.onboarding_complete && !self.onboarding_requested) {
            self.selected = report
                .tools
                .iter()
                .filter(|tool| tool.selected && chooser_eligible(tool))
                .map(|tool| tool.id.clone())
                .collect();
            self.draft_initialized = true;
        }
        if self.onboarding_requested {
            for tool in &report.tools {
                if chooser_eligible(tool) {
                    if self.retained_onboarding.remove(&tool.id) { self.selected.insert(tool.id.clone()); }
                } else if tool.selected && self.selected.contains(&tool.id) {
                    self.retained_onboarding.insert(tool.id.clone());
                }
            }
        }
        self.selected.retain(|id| {
            report
                .tools
                .iter()
                .any(|tool| &tool.id == id && chooser_eligible(tool))
        });
        if !self.provider_overview
            && !self
                .provider_id
                .as_ref()
                .is_some_and(|id| visible_providers(&report).iter().any(|tool| &tool.id == id))
        {
            self.provider_id = visible_providers(&report)
                .first()
                .map(|tool| tool.id.clone());
        }
        self.report = Some(report);
        self.loading = false;
        self.busy = false;
        self.checking = false;
        self.error = None;
    }

    fn selection_to_save(&self) -> Vec<String> {
        let mut selected = self.selected.union(&self.retained_onboarding)
            .cloned().collect::<BTreeSet<_>>();
        if let Some(report) = &self.report {
            // Retain only already-saved hidden choices, never a catalog default.
            selected.extend(report.tools.iter().filter(|tool| {
                matches!(tool.id.as_str(), "amp" | "capy") && tool.selected
            }).map(|tool| tool.id.clone()));
        }
        selected.into_iter().collect()
    }

    pub fn needs_onboarding(&self) -> bool {
        !self.dismissed
            && (self.onboarding_requested || self
                .report
                .as_ref()
                .is_some_and(|report| !report.onboarding_complete))
    }

    pub fn choose(&mut self, id: &str, selected: bool) {
        let allowed = self.report.as_ref().is_some_and(|report| {
            report
                .tools
                .iter()
                .any(|tool| tool.id == id && chooser_eligible(tool))
        });
        if !allowed {
            return;
        }
        if selected {
            self.selected.insert(id.into());
            if let Some(report) = &self.report {
                for &(_, provider) in external::agent_providers(id) {
                    if report
                        .tools
                        .iter()
                        .any(|tool| tool.id == provider && chooser_eligible(tool))
                    {
                        self.selected.insert(provider.into());
                    }
                }
            }
        } else {
            self.selected.remove(id);
            self.retained_onboarding.remove(id);
            for choices in [&mut self.selected, &mut self.retained_onboarding] {
                choices.retain(|host| {
                    !external::agent_providers(host).iter().any(|(_, provider)| *provider == id)
                });
            }
        }
    }
}

pub(super) fn account_health_label(health: Option<&str>) -> &'static str {
    match health {
        Some("ready") => "Verified",
        Some("sign_in_required") => "Sign in required",
        Some("offline") => "Cannot verify sign-in",
        Some("checking") => "Checking sign-in",
        _ => "Not checked",
    }
}

impl App {
    pub(super) fn sync_tools_account(&self) {
        let identity = self.with_state(|state| {
            let account = state.account();
            (
                state.generation,
                account.site_url,
                account.workspace_id,
                account.person.and_then(|person| person.id),
            )
        });
        let changed = self.update_state(|state| {
            if state.tools.identity.as_ref() == Some(&identity) {
                return false;
            }
            state.tools = ToolsUi {
                identity: Some(identity),
                ..Default::default()
            };
            true
        });
        if self.with_state(|state| {
            (changed || state.tools.report.is_none()) && crate::models::provider_onboarding_ready(&state.account())
        }) {
            self.refresh_tools();
        }
    }

    pub fn refresh_tools(&self) {
        // Navigation, like polling, must retain a failed change until the
        // person explicitly retries, checks health or chooses another action.
        self.read_tools_status(true);
    }

    pub(super) fn start_tools_poll(&self) {
        let identity = self.with_state(|state| state.tools.identity.clone());
        let start = self.update_state(|state| {
            if state.tools.polling {
                return false;
            }
            state.tools.polling = true;
            true
        });
        if !start {
            return;
        }
        let weak = self.downgrade();
        gtk4::glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            let Some(app) = weak.upgrade() else {
                return gtk4::glib::ControlFlow::Break;
            };
            if app.with_state(|state| state.tools.identity.clone()) != identity {
                return gtk4::glib::ControlFlow::Break;
            }
            if app.page().section() != Section::Settings {
                app.update_state(|state| state.tools.polling = false);
                return gtk4::glib::ControlFlow::Break;
            }
            let ready = app.with_state(|state| {
                state.core_connected && state.connected() && !state.tools.needs_onboarding()
            });
            if app.window().is_visible() && ready {
                app.read_tools_status(true);
            }
            gtk4::glib::ControlFlow::Continue
        });
    }

    fn tools_changed(&self) {
        self.render();
        let changed = self.update_state(|state| {
            let report = serde_json::to_value(&state.tools.report).ok();
            if state.tools.sidebar_report == report {
                return false;
            }
            state.tools.sidebar_report = report;
            true
        });
        if changed {
            self.render_sidebar();
        }
        if self.page() == Page::Settings(Pane::Tools) {
            self.render_settings();
        }
    }

    pub fn open_providers(&self) {
        self.update_state(|state| {
            state.tools.provider_overview = true;
            state.tools.provider_id = None;
        });
        self.go(Page::Settings(Pane::Tools));
    }

    pub fn open_provider(&self, id: &str) {
        let known = self.update_state(|state| {
            let known =
                state.tools.report.as_ref().is_some_and(|report| {
                    visible_providers(report).iter().any(|tool| tool.id == id)
                });
            if known {
                state.tools.provider_overview = false;
                state.tools.provider_id = Some(id.into());
            }
            known
        });
        if known {
            self.go(Page::Settings(Pane::Tools));
        } else {
            self.open_providers();
        }
    }

    pub fn check_tools(&self) {
        if !self.start_tools_action(agent_relay_desktop_core::contract::method::ToolsCheck::NAME) {
            return;
        }
        self.update_state(|state| state.tools.checking = true);
        self.tools_changed();
        let identity = self.with_state(|state| state.request_identity_revision);
        self.view_call::<agent_relay_desktop_core::contract::method::ToolsCheck>(NoParams {}, move |app, result| {
            app.finish_tools_action(result, false, identity);
        });
    }

    fn start_tools_action(&self, method: &str) -> bool {
        if !self.bridge().offers(method) {
            return false;
        }
        let start = self.update_state(|state| {
            if !state.core_connected || state.tools.busy {
                return false;
            }
            state.tools.operation_revision = state.tools.operation_revision.wrapping_add(1);
            state.tools.loading = false;
            state.tools.busy = true;
            state.tools.error = None;
            true
        });
        if start {
            self.tools_changed();
        }
        start
    }

    pub fn manage_tool(&self, id: &str, action: &str) {
        if !self.start_tools_action(agent_relay_desktop_core::contract::method::ToolsManage::NAME) {
            return;
        }
        let identity = self.with_state(|state| state.request_identity_revision);
        let enabled_id = (action == "enable").then(|| id.to_owned());
        self.view_call::<agent_relay_desktop_core::contract::method::ToolsManage>(
            ToolsManageParams {
                tool_id: id.into(),
                action: action.into(),
            },
            move |app, result| {
                let enabled = result.is_ok()
                    && app.with_state(|state| state.request_identity_revision) == identity;
                app.finish_tools_action(result, false, identity);
                if enabled {
                    if let Some(id) = enabled_id {
                        app.open_provider(&id);
                    }
                }
            },
        );
    }

    pub fn finish_tools_selection(&self, skip: bool) {
        if skip && self.with_state(|state| state.tools.onboarding_requested && !state.tools.busy) {
            self.continue_without_tools();
            return;
        }
        if !self.start_tools_action(agent_relay_desktop_core::contract::method::ToolsSelect::NAME) {
            return;
        }
        let (tool_ids, teammate_context) = self.with_state(|state| {
            if skip {
                // Not now leaves the teammate-context default as it is.
                (Vec::new(), None)
            } else {
                (
                    state.tools.selection_to_save(),
                    state.tools.teammate_to_save(),
                )
            }
        });
        let identity = self.with_state(|state| state.request_identity_revision);
        self.view_call::<agent_relay_desktop_core::contract::method::ToolsSelect>(
            ToolsSelectParams {
                tool_ids,
                teammate_context,
            },
            move |app, result| {
                app.finish_tools_action(result, true, identity);
            },
        );
    }

    fn finish_tools_action(
        &self,
        result: Result<ToolsReport, crate::bridge::CoreError>,
        selection: bool,
        identity: u64,
    ) {
        if self.with_state(|state| state.request_identity_revision) != identity {
            return;
        }
        self.update_state(|state| match result {
            Ok(report) => {
                if selection && report.onboarding_complete {
                    state.tools.onboarding_requested = false;
                    state.tools.teammate_draft = None;
                    state.tools.dismissed = true;
                }
                if state.tools.checking {
                    state.tools.last_checked =
                        Some(chrono::Local::now().format("%H:%M").to_string());
                }
                state.tools.read(report);
            }
            Err(_) => {
                state.tools.busy = false;
                state.tools.checking = false;
                state.tools.error = Some(if selection {
                    "Your provider choices could not be saved. Try again in Settings › Providers."
                        .into()
                } else {
                    "Couldn’t update this connection. Try again or open Connection details.".into()
                });
            }
        });
        self.tools_changed();
    }

    fn retry_tools_selection(&self) {
        let cleared = self.update_state(|state| {
            state.tools.dismissed = false;
            state.tools.error.take().is_some()
        });
        if self.with_state(|state| state.tools.needs_onboarding()) {
            self.render();
        } else {
            // The read that follows may change nothing else, so the cleared
            // error is drawn away now rather than left until a change.
            if cleared {
                self.tools_changed();
            }
            self.refresh_tools();
        }
    }

    pub fn continue_without_tools(&self) {
        self.update_state(|state| { state.tools.dismissed = true; state.tools.onboarding_requested = false; });
        self.tools_changed();
    }

    pub fn reopen_tools_onboarding(&self) {
        self.update_state(|state| {
            if state.tools.busy { return; }
            let Some(report) = &state.tools.report else { return; };
            state.tools.selected = report.tools.iter().filter(|tool| tool.selected && chooser_eligible(tool)).map(|tool| tool.id.clone()).collect();
            state.tools.retained_onboarding = report.tools.iter().filter(|tool| tool.selected && !chooser_eligible(tool)).map(|tool| tool.id.clone()).collect();
            state.tools.onboarding_requested = true;
            state.tools.teammate_draft = None;
            state.tools.dismissed = false;
            state.tools.error = None;
        });
        self.render();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;

    fn tool(id: &str, detected: bool, supported: bool, selected: bool) -> ToolIntegration {
        ToolIntegration {
            id: id.into(),
            name: id.into(),
            detected,
            onboarding_available: None,
            supported,
            selected,
            state: "not_installed".into(),
            status_label: "Not installed".into(),
            detail: "Configuration is not running-chat verification.".into(),
            can_install: supported && detected,
            can_repair: false,
            can_uninstall: false,
        }
    }

    #[test]
    fn continue_preserves_saved_hidden_choices_without_enrolling_absent_defaults() {
        let mut hidden = tool("capy", true, false, true);
        hidden.onboarding_available = Some(false);
        let mut absent = tool("amp", true, true, false);
        absent.onboarding_available = Some(false);
        absent.state = "configured".into();
        let mut ui = ToolsUi::default();
        ui.read(ToolsReport { onboarding_complete: false, account_health: None, teammate_context: None,
            tools: vec![hidden, absent, tool("codex", true, true, true)] });
        assert_eq!(ui.selected, BTreeSet::from(["codex".into()]));
        ui.choose("amp", true);
        assert_eq!(ui.selection_to_save(), vec!["capy", "codex"]);
        assert_eq!(onboarding_tools(ui.report.as_ref().unwrap()).map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["codex"]);
        ui.report.as_mut().unwrap().tools[0].onboarding_available = Some(true);
        assert_eq!(ui.selection_to_save(), vec!["capy", "codex"], "installed Cloud setup is not deselection");
    }

    #[test]
    fn selection_admits_only_detected_supported_tools_and_keeps_unsaved_choices() {
        let mut ui = ToolsUi::default();
        ui.read(ToolsReport {
            teammate_context: None,
            onboarding_complete: false,
            account_health: None,
            tools: vec![
                tool("codex", true, true, true),
                tool("claude", true, false, true),
                tool("absent", false, true, true),
            ],
        });
        assert_eq!(
            ui.selected.iter().cloned().collect::<Vec<_>>(),
            vec!["codex"]
        );
        ui.choose("claude", true);
        ui.choose("absent", true);
        ui.choose("unknown", true);
        ui.choose("codex", false);
        assert!(ui.selected.is_empty());
        let report = ui.report.clone().unwrap();
        ui.read(report);
        assert!(ui.selected.is_empty());
        assert!(ui.needs_onboarding());
    }

    #[test]
    fn host_choices_are_selectable_and_survive_draft_refresh() {
        let mut ui = ToolsUi::default();
        ui.read(ToolsReport {
            tools: vec![
                tool("codex", true, true, false),
                tool("conductor", true, false, false),
                tool("paseo", true, false, false),
                tool("capy", true, false, false),
            ],
            ..Default::default()
        });
        ui.choose("conductor", true);
        ui.choose("paseo", true);
        ui.choose("capy", true);
        ui.read(ui.report.clone().unwrap());
        assert_eq!(
            ui.selected,
            BTreeSet::from(["codex".into(), "conductor".into(), "paseo".into()])
        );
        ui.choose("conductor", false);
        assert_eq!(
            ui.selected,
            BTreeSet::from(["codex".into(), "paseo".into()])
        );
        ui.choose("codex", false);
        assert!(ui.selected.is_empty());
        ui.choose("paseo", true);
        let mut report = ui.report.clone().unwrap();
        report
            .tools
            .iter_mut()
            .for_each(|tool| tool.detected = false);
        ui.read(report);
        assert!(ui.selected.is_empty());
    }

    #[test]
    fn the_teammate_line_follows_the_saved_state_until_the_person_changes_it() {
        let mut ui = ToolsUi::default();
        let report = |saved: Option<bool>| ToolsReport {
            onboarding_complete: true,
            account_health: None,
            tools: vec![tool("claude", true, true, true)],
            teammate_context: saved,
        };
        // Turned off in Settings: a reopened chooser shows it off and saves
        // nothing unless the person ticks it.
        ui.read(report(Some(false)));
        assert!(!ui.teammate_choice());
        assert_eq!(ui.teammate_to_save(), None);
        ui.read(report(Some(true)));
        assert!(ui.teammate_choice());
        assert_eq!(ui.teammate_to_save(), None);
        // The person's untick survives polls.
        ui.teammate_draft = Some(false);
        ui.read(report(Some(true)));
        assert!(!ui.teammate_choice());
        assert_eq!(ui.teammate_to_save(), Some(false));
        // Nothing saved yet (Claude Code just chosen): off by default, and an
        // untouched line saves nothing; ticking it turns it on.
        let mut fresh = ToolsUi::default();
        fresh.read(report(None));
        fresh.selected.insert("claude".into());
        assert!(!fresh.teammate_choice());
        assert_eq!(fresh.teammate_to_save(), None);
        fresh.teammate_draft = Some(true);
        assert!(fresh.teammate_choice());
        assert_eq!(fresh.teammate_to_save(), Some(true));
    }

    #[test]
    fn provider_navigation_survives_status_updates_and_falls_back_only_if_missing() {
        let mut ui = ToolsUi::default();
        let report = ToolsReport {
            teammate_context: None,
            onboarding_complete: true,
            account_health: Some("ready".into()),
            tools: vec![
                tool("codex", true, true, true),
                tool("claude", true, false, false),
            ],
        };
        ui.read(report.clone());
        ui.provider_id = Some("claude".into());
        ui.read(report);
        assert_eq!(ui.provider_id.as_deref(), Some("claude"));
        ui.provider_overview = true;
        ui.provider_id = None;
        ui.read(ui.report.clone().unwrap());
        assert!(ui.provider_id.is_none());
        ui.provider_overview = false;
        ui.read(ToolsReport {
            onboarding_complete: true,
            tools: vec![tool("codex", true, true, true)],
            ..Default::default()
        });
        assert_eq!(ui.provider_id.as_deref(), Some("codex"));
    }

    #[test]
    fn provider_visibility_and_default_navigation_use_alphabetical_visible_catalog() {
        let mut ui = ToolsUi::default();
        let report = ToolsReport {
            onboarding_complete: true,
            tools: vec![
                tool("zeta", true, true, false),
                tool("hidden", false, false, false),
                tool("Absent", false, true, false),
                tool("alpha", false, false, true),
            ],
            ..Default::default()
        };
        assert_eq!(
            visible_providers(&report)
                .iter()
                .map(|tool| tool.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(
            available_providers(&report)
                .iter()
                .map(|tool| tool.id.as_str())
                .collect::<Vec<_>>(),
            vec!["Absent", "hidden"]
        );
        ui.read(report);
        assert_eq!(ui.provider_id.as_deref(), Some("alpha"));
        ui.read(ToolsReport {
            tools: vec![tool("hidden", false, true, false)],
            ..Default::default()
        });
        assert!(ui.provider_id.is_none());
    }

    #[test]
    fn newly_detected_retained_choice_can_be_deselected() {
        let mut ui = ToolsUi { onboarding_requested: true, ..Default::default() };
        ui.retained_onboarding.insert("opencode".into());
        ui.read(ToolsReport { onboarding_complete: true, tools: vec![tool("opencode", true, true, true)], ..Default::default() });
        assert!(ui.selected.contains("opencode"));
        assert!(ui.retained_onboarding.is_empty());
        ui.choose("opencode", false);
        assert!(ui.selected.is_empty());
        assert!(ui.retained_onboarding.is_empty());
    }

    #[test]
    fn active_cloud_provider_is_visible_without_local_installation() {
        let mut cloud = tool("grok-bot", false, false, false);
        cloud.state = "configured".into();
        let mut report = ToolsReport { tools: vec![cloud], ..Default::default() };
        assert_eq!(visible_providers(&report).len(), 1);
        assert!(available_providers(&report).is_empty());
        assert!(!chooser_eligible(&report.tools[0]));
        report.tools[0].state = "external_setup".into();
        assert!(visible_providers(&report).is_empty());
        assert_eq!(available_providers(&report).len(), 1);
    }

    #[test]
    fn absent_amp_and_capy_are_not_offered_by_their_catalog_actions() {
        use crate::models::CloudProviderSetup;
        let mut amp = tool("amp", false, false, false);
        amp.state = "external_setup".into();
        let mut capy = tool("capy", false, false, false);
        capy.state = "external_setup".into();
        let report = ToolsReport {
            tools: vec![
                amp,
                capy,
                tool("copilot", true, true, false),
                tool("claude", true, false, false),
                tool("opencode", false, false, false),
            ],
            ..Default::default()
        };
        let action =
            |id: &str| onboarding_action(report.tools.iter().find(|tool| tool.id == id).unwrap());
        assert_eq!(action("amp"), OnboardingAction::OpenCloudConnections);
        assert_eq!(action("copilot"), OnboardingAction::Checkbox);
        assert_eq!(
            action("capy"),
            OnboardingAction::CloudSetup(CloudProviderSetup::Capy)
        );
        assert_eq!(action("claude"), OnboardingAction::NoChoice);
        assert_eq!(
            onboarding_tools(&report).map(|tool| tool.id.as_str()).collect::<Vec<_>>(),
            vec!["copilot", "claude"]
        );
    }

    #[test]
    fn onboarding_requires_installation_despite_detected_selected_or_configured() {
        for id in ["amp", "capy"] {
            for evidence in ["detected", "selected", "configured"] {
                let mut candidate = tool(id, evidence == "detected", false, evidence == "selected");
                candidate.state = if evidence == "configured" { "configured" } else { "external_setup" }.into();
                let mut report = ToolsReport { tools: vec![candidate], ..Default::default() };
                assert_eq!(onboarding_tools(&report).count(), 0);
                report.tools[0].onboarding_available = Some(false);
                assert_eq!(onboarding_tools(&report).count(), 0);
                report.tools[0].onboarding_available = Some(true);
                assert_eq!(onboarding_tools(&report).map(|tool| tool.id.as_str()).collect::<Vec<_>>(), vec![id]);
                assert!(!chooser_eligible(&report.tools[0]), "Cloud setup never becomes a local MCP choice");
            }
        }
    }

    #[test]
    fn copilot_is_added_explicitly_and_amp_follows_its_cloud_connection() {
        let ids = |tools: Vec<&ToolIntegration>| {
            tools.iter().map(|tool| tool.id.clone()).collect::<Vec<_>>()
        };
        let mut amp = tool("amp", false, false, false);
        amp.state = "external_setup".into();
        let mut ui = ToolsUi::default();
        let mut report = ToolsReport {
            onboarding_complete: true,
            tools: vec![
                amp,
                tool("codex", true, true, false),
                tool("copilot", true, true, false),
            ],
            ..Default::default()
        };
        assert_eq!(ids(visible_providers(&report)), vec!["codex"]);
        assert_eq!(ids(available_providers(&report)), vec!["amp", "copilot"]);
        ui.read(report.clone());
        assert_eq!(ui.provider_id.as_deref(), Some("codex"));
        // Cloud reporting Amp's connection shows it, like Capy, Grok and Dot.
        report.tools[0].state = "configured".into();
        assert_eq!(ids(visible_providers(&report)), vec!["amp", "codex"]);
        assert_eq!(ids(available_providers(&report)), vec!["copilot"]);
        // Copilot appears only once it is added.
        report.tools[2].selected = true;
        assert_eq!(
            ids(visible_providers(&report)),
            vec!["amp", "codex", "copilot"]
        );
        assert!(available_providers(&report).is_empty());
    }

    #[test]
    fn explicit_escape_is_not_persisted_completion() {
        let mut ui = ToolsUi::default();
        assert!(!ui.needs_onboarding());
        ui.read(ToolsReport {
            onboarding_complete: false,
            ..Default::default()
        });
        ui.dismissed = true;
        assert!(!ui.needs_onboarding());
        assert!(!ui.report.as_ref().unwrap().onboarding_complete);
        ui.dismissed = false;
        assert!(ui.needs_onboarding());
        ui.read(ToolsReport {
            onboarding_complete: true,
            ..Default::default()
        });
        assert!(!ui.needs_onboarding());
    }
}
