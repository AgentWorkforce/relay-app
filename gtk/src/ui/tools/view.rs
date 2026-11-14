//! Tools settings and the optional post-sign-in chooser.

use super::*;
use crate::models::{CloudProviderSetup, provider_onboarding_ready, provider_onboarding_detail};
use crate::ui::widgets::{self, button, hbox, text, vbox, LabelStyle};
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;
use gtk::{prelude::*, Align};
use gtk4 as gtk;

pub const CHOOSER_TITLE: &str = "Choose your providers";
pub const CHOOSER_DETAIL: &str =
    "Choose where to enable Agent Relay. Running chats may need a reload or a new session.";
pub const OVERVIEW_TITLE: &str = "Add provider";
pub const OVERVIEW_DETAIL: &str =
    "Installed providers appear in the sidebar automatically. Add others here, including GitHub Copilot and Amp.";
pub const OVERVIEW_EMPTY: &str = "All providers are already in the sidebar.";

/// The chooser's widget name: a digest of everything it shows, so an
/// unchanged state is recognized without drawing it again.
fn chooser_name(ui: &ToolsUi) -> String {
    use std::hash::{Hash, Hasher};
    let mut digest = std::collections::hash_map::DefaultHasher::new();
    if let Some(report) = &ui.report {
        for tool in onboarding_tools(report) {
            (&tool.id, &tool.name, &tool.detail, tool.supported).hash(&mut digest);
        }
    }
    (
        &ui.selected,
        ui.busy,
        ui.loading,
        &ui.error,
        &ui.setup_started,
        ui.report.is_some(),
    )
        .hash(&mut digest);
    format!("tools-chooser-{:x}", digest.finish())
}

impl App {
    pub(in crate::ui) fn render_tools_onboarding(&self) -> bool {
        let show = self.with_state(|state| {
            state.core_connected
                && provider_onboarding_ready(&state.account())
                && state.tools.needs_onboarding()
        });
        if !show || !self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsSelect::NAME) {
            return false;
        }
        self.update_state(|state| state.view = None);
        // A status update that changes nothing the chooser shows, such as a
        // background account check, keeps it mounted with its focus and
        // scroll position.
        let shown = self.with_state(|state| chooser_name(&state.tools));
        let mounted = self
            .window()
            .child()
            .is_some_and(|child| child.widget_name() == shown.as_str());
        if mounted {
            return true;
        }
        let root = vbox(0, &["welcome"]);
        root.set_widget_name(&shown);
        let (column, holder) = widgets::column(560);
        holder.set_valign(Align::Fill);
        holder.set_vexpand(true);
        widgets::pad(&column, 36, None, None);
        crate::ui::zoom::spacing(&column, 20);
        column.set_vexpand(true);
        column.append(&text(CHOOSER_TITLE, &["settings-profile-name"]).wrapped());
        column.append(&text(CHOOSER_DETAIL, &["secondary"]).wrapped());
        let ui = self.with_state(|state| state.tools.clone());
        if let Some(report) = &ui.report {
            let choices = vbox(16, &[]);
            for tool in onboarding_tools(report) {
                choices.append(&self.tool_choice(tool, &ui));
            }
            let scrolling = widgets::scrolled(&choices);
            scrolling.set_vexpand(true);
            scrolling.set_min_content_height(100);
            column.append(&scrolling);
            let supported = report.tools.iter().any(chooser_eligible);
            let footer = if supported {
                "You can change this later in Settings › Providers."
            } else {
                "No supported providers were detected. You can continue and add providers later in Settings › Providers."
            };
            column.append(&text(footer, &["settings-row-detail"]).wrapped());
        }
        if let Some(error) = &ui.error {
            column.append(&text(error, &["warn"]).wrapped());
        }
        let actions = hbox(12, &[]);
        let later = button(
            "Not now",
            &[],
            self.on(|app| app.finish_tools_selection(true)),
        );
        later.set_sensitive(!ui.busy && !ui.loading);
        actions.append(&later);
        let next = button(
            if ui.busy { "Saving…" } else { "Continue" },
            &["primary"],
            self.on(|app| app.finish_tools_selection(false)),
        );
        next.set_sensitive(!ui.busy && !ui.loading);
        actions.append(&next);
        column.append(&actions);
        if ui.error.is_some() {
            let escape = button("Continue to app", &[], self.on(App::continue_without_tools));
            escape.set_sensitive(!ui.busy && !ui.loading);
            column.append(&escape);
        }
        root.append(&holder);
        self.replace(&root);
        true
    }

    /// One provider in the chooser, as on the Mac: its checkbox, its mark,
    /// then its name over what it does.
    fn tool_choice(&self, tool: &ToolIntegration, ui: &ToolsUi) -> gtk::Box {
        let row = hbox(12, &[]);
        row.set_margin_top(14);
        row.set_margin_bottom(14);
        let check = gtk::CheckButton::new();
        check.set_size_request(-1, 28);
        check.set_valign(gtk::Align::Start);
        // Tests and assistive technology find the box by the provider's name.
        check.set_widget_name(&tool.name);
        widgets::name(&check, &tool.name);
        check.set_active(ui.selected.contains(&tool.id));
        check.set_sensitive(chooser_eligible(tool) && !ui.busy && !ui.loading);
        let id = tool.id.clone();
        let weak = self.downgrade();
        check.connect_toggled(move |check| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| state.tools.choose(&id, check.is_active()));
                app.sync_chooser_choices();
            }
        });
        let action = onboarding_action(tool);
        if action == OnboardingAction::Checkbox {
            row.append(&check);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_size_request(14, 28);
            row.append(&spacer);
        }
        let mark = widgets::agent_badge(&tool.id, 28);
        mark.set_valign(gtk::Align::Start);
        row.append(&mark);
        let words = vbox(5, &[]);
        words.set_hexpand(true);
        let title = text(&tool.name, &["settings-row-title"]);
        title.set_size_request(-1, 28);
        // Native checkbox themes can request more than 28px. Keep all three
        // elements in the first line aligned at the same measured height.
        let first_line = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
        first_line.add_widget(&check);
        first_line.add_widget(&mark);
        first_line.add_widget(&title);
        words.append(&title);
        let account = self.with_state(|state| state.account());
        words.append(&text(provider_onboarding_detail(tool, &account), &["settings-row-detail"]).wrapped());
        if let Some(setup) = self.cloud_setup_button(&tool.id) {
            setup.set_halign(gtk::Align::Start);
            setup.set_sensitive(!ui.busy && !ui.loading);
            words.append(&setup);
            if let Some(snippet) = self.cloud_setup_snippet(&tool.id) {
                words.append(&snippet);
            } else if let Some(link) = self.cloud_setup_skill_link(&tool.id) {
                link.set_halign(gtk::Align::Start);
                words.append(&link);
            }
            if ui.setup_started.contains(&tool.id) {
                if let Some(copied) = CloudProviderSetup::for_id(&tool.id).and_then(|setup| setup.copied()) {
                    words.append(&text(copied, &["caption", "secondary"]).wrapped());
                }
            }
        } else if action == OnboardingAction::OpenCloudConnections {
            if let Some(connections) = self.amp_connections_button(&tool.id) {
                connections.set_halign(gtk::Align::Start);
                connections.set_sensitive(!ui.busy && !ui.loading);
                words.append(&connections);
            }
        }
        row.append(&words);
        row
    }

    fn sync_chooser_choices(&self) {
        let Some(root) = self
            .window()
            .child()
            .filter(|root| root.widget_name().starts_with("tools-chooser-"))
        else {
            return;
        };
        let choices = self.with_state(|state| {
            state
                .tools
                .report
                .as_ref()
                .map(|report| {
                    report
                        .tools
                        .iter()
                        .map(|tool| (tool.name.clone(), state.tools.selected.contains(&tool.id)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        widgets::walk(&root, &mut |widget| {
            let Some(check) = widget.downcast_ref::<gtk::CheckButton>() else {
                return;
            };
            let Some((_, active)) = choices
                .iter()
                .find(|(name, _)| name == check.widget_name().as_str())
            else {
                return;
            };
            if check.is_active() != *active {
                check.set_active(*active);
            }
        });
        // Retain this chooser through subsequent unchanged status updates.
        root.set_widget_name(&self.with_state(|state| chooser_name(&state.tools)));
    }

    pub(in crate::ui) fn tools_pane(&self, column: &gtk::Box) {
        self.start_tools_poll();
        let ui = self.with_state(|state| state.tools.clone());
        let chosen = ui.report.as_ref().and_then(|report| {
            if ui.provider_overview {
                return None;
            }
            let visible = visible_providers(report);
            visible
                .iter()
                .copied()
                .find(|tool| Some(&tool.id) == ui.provider_id.as_ref())
                .or_else(|| visible.first().copied())
        });
        if let Some(tool) = chosen {
            // Identity and next step, settings, then collapsed diagnostics.
            self.provider_connection_detail(column, tool, &ui);
            self.tools_error(column, &ui);
            self.external_provider_setup(column, &tool.id);
            self.provider_spawn_settings(column, &tool.id);
            self.provider_preferences(column, &tool.id);
            let diagnostics = vbox(10, &[]);
            let health = ui.report.as_ref().and_then(|r| r.account_health.as_deref());
            self.provider_connection_facts(&diagnostics, tool, &ui, health);
            self.provider_opening_requirements(&diagnostics, Some(&tool.id));
            column.append(&diagnostics);
        } else {
            column.append(&text(OVERVIEW_TITLE, &["settings-profile-name"]));
            column.append(&text(OVERVIEW_DETAIL, &["settings-row-detail"]).wrapped());
            if ui.report.is_none() {
                column.append(&text(if ui.loading { "Loading providers…" } else { "Provider setup is unavailable. Update or restart Agent Relay and try again." }, &["secondary"]).wrapped());
            }
            if let Some(report) = &ui.report {
                self.available_providers_section(column, report, &ui);
            }
            self.tools_error(column, &ui);
            let choose = button("Choose providers…", &[], self.on(App::reopen_tools_onboarding));
            choose.set_sensitive(!ui.busy && ui.report.is_some());
            column.append(&choose);
        }
    }

    /// Providers not installed here, each added with a secondary button so
    /// the list doesn't compete as a column of primary actions.
    fn available_providers_section(&self, column: &gtk::Box, report: &ToolsReport, ui: &ToolsUi) {
        let available = available_providers(report);
        if available.is_empty() {
            column.append(&text(OVERVIEW_EMPTY, &["secondary"]).wrapped());
            return;
        }
        let (section, group) = widgets::settings_section("Available providers", None);
        for (index, tool) in available.into_iter().enumerate() {
            if index > 0 {
                widgets::settings_divider(&group, 50);
            }
            let row = hbox(12, &[]);
            widgets::pad(&row, 10, Some(14), None);
            row.append(&widgets::agent_badge(&tool.id, 24));
            let name = text(&tool.name, &["row-title"]);
            name.set_hexpand(true);
            row.append(&name);
            let id = tool.id.clone();
            let add = button(
                "Add",
                &[],
                self.on(move |app| app.manage_tool(&id, "enable")),
            );
            widgets::name(&add, &format!("Add {}", tool.name));
            add.set_sensitive(!ui.busy && self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsManage::NAME));
            row.append(&add);
            group.append(&row);
        }
        column.append(&section);
    }

    fn tools_error(&self, column: &gtk::Box, ui: &ToolsUi) {
        if let Some(error) = &ui.error {
            column.append(&text(error, &["warn"]).wrapped());
            let retry = button("Try again", &[], self.on(App::retry_tools_selection));
            retry.set_sensitive(!ui.busy && !ui.loading);
            column.append(&retry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chooser_explains_that_running_chats_may_need_reload() {
        assert_eq!(CHOOSER_TITLE, "Choose your providers");
        assert!(CHOOSER_DETAIL.contains("Running chats may need a reload or a new session."));
    }
}
