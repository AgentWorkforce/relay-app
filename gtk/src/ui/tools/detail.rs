//! The provider's identity and status with its one next step beside it, and
//! deliberately secondary maintenance and evidence.
use super::presentation::{NextAction, Presentation};
use super::provider_status::ProviderStatus;
use super::*;
use crate::models::CAPY_SETUP_COPIED;
use crate::models::CloudProviderSetup;
use crate::ui::widgets::{self, button, hbox, text, vbox, Dot, LabelStyle};
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    pub(super) fn provider_connection_detail(
        &self,
        column: &gtk::Box,
        tool: &ToolIntegration,
        ui: &ToolsUi,
    ) {
        let health = ui
            .report
            .as_ref()
            .and_then(|report| report.account_health.as_deref());
        let presentation = Presentation::new(tool, health);
        let connection = ProviderStatus::for_tool(tool, health);
        let heading = hbox(12, &[]);
        heading.append(&widgets::agent_badge(&tool.id, 32));
        let title = vbox(3, &[]);
        title.set_hexpand(true);
        title.set_valign(gtk::Align::Center);
        title.append(&text(&tool.name, &["settings-profile-name"]));
        let status = hbox(6, &[]);
        let dot = Dot::colored(6, connection.color()).widget;
        dot.set_valign(gtk::Align::Center);
        status.append(&dot);
        let tone = if connection == ProviderStatus::Attention {
            "warn"
        } else {
            "secondary"
        };
        status.append(&text(&presentation.status, &["caption", tone]));
        title.append(&status);
        heading.append(&title);
        if let Some(primary) = self.provider_primary_action(tool, &presentation, ui) {
            primary.set_valign(gtk::Align::Center);
            heading.append(&primary);
        }
        self.provider_maintenance(&heading, tool, ui);
        column.append(&heading);
        if let Some(summary) = &presentation.summary {
            column.append(&text(summary, &["settings-row-detail"]).wrapped());
        }
        if let Some(snippet) = self.cloud_setup_snippet(&tool.id) {
            column.append(&snippet);
        } else if let Some(link) = self.cloud_setup_skill_link(&tool.id) {
            link.set_halign(gtk::Align::Start);
            column.append(&link);
        }
        if tool.id == "capy" && tool.state != "configured" && self.with_state(|state| state.capy_prompt_copied) {
            column.append(&text(CAPY_SETUP_COPIED, &["caption", "secondary"]));
        }
        if tool.id != "capy" && ui.setup_started.contains(&tool.id) {
            if let Some(copied) = CloudProviderSetup::for_id(&tool.id).and_then(|setup| setup.copied()) {
                column.append(&text(copied, &["caption", "secondary"]).wrapped());
            }
        }
    }

    /// The one blue action: what to connect or fix next, or Capy's own setup.
    fn provider_primary_action(
        &self,
        tool: &ToolIntegration,
        presentation: &Presentation,
        ui: &ToolsUi,
    ) -> Option<gtk::Button> {
        let Some(action) = presentation.action else {
            return self
                .cloud_setup_button(&tool.id)
                .or_else(|| self.amp_connections_button(&tool.id));
        };
        let id = tool.id.clone();
        let needs_sign_in = tool.state == "signed_out";
        let primary = button(
            if ui.busy {
                "Working…"
            } else {
                presentation.action_label().unwrap_or_default()
            },
            &["primary"],
            self.on(move |app| match action {
                NextAction::Account => {
                    app.go(Page::Settings(Pane::Account));
                    if needs_sign_in {
                        app.renew_sign_in();
                    }
                }
                NextAction::Check => app.check_tools(),
                NextAction::Install => app.manage_tool(&id, "install"),
                NextAction::Enable => app.manage_tool(&id, "enable"),
                NextAction::Repair => app.manage_tool(&id, "repair"),
            }),
        );
        let offered = match action {
            NextAction::Account => true,
            NextAction::Check => self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsCheck::NAME),
            NextAction::Install | NextAction::Repair | NextAction::Enable => {
                self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsManage::NAME)
            }
        };
        primary.set_sensitive(!ui.busy && offered);
        Some(primary)
    }

    fn provider_maintenance(&self, actions: &gtk::Box, tool: &ToolIntegration, ui: &ToolsUi) {
        let removable = tool.selected && !tool.supported;
        if !(tool.supported && tool.can_repair) && !tool.can_uninstall && !removable {
            return;
        }
        let menu = gtk::MenuButton::new();
        menu.set_icon_name("view-more-symbolic");
        menu.set_valign(gtk::Align::Center);
        menu.add_css_class("flat");
        widgets::action_cursor(&menu);
        menu.set_widget_name("provider-more-options");
        widgets::name(&menu, "More options");
        menu.set_tooltip_text(Some("More options"));
        let popover = gtk::Popover::new();
        let list = vbox(4, &[]);
        widgets::pad(&list, 8, None, None);
        for (label, action, allowed) in [
            ("Reinstall", "reinstall", tool.supported && tool.can_repair),
            ("Disconnect", "uninstall", tool.can_uninstall),
            (
                if tool.detected {
                    "Deselect provider"
                } else {
                    "Remove from sidebar"
                },
                "disable",
                removable,
            ),
        ] {
            if !allowed {
                continue;
            }
            let id = tool.id.clone();
            let weak = popover.downgrade();
            list.append(&button(
                label,
                &["flat"],
                self.on(move |app| {
                    if let Some(popover) = weak.upgrade() {
                        popover.popdown();
                    }
                    app.manage_tool(&id, action);
                }),
            ));
        }
        popover.set_child(Some(&list));
        menu.set_popover(Some(&popover));
        menu.set_sensitive(!ui.busy && self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsManage::NAME));
        actions.append(&menu);
    }

    pub(super) fn provider_connection_facts(
        &self,
        column: &gtk::Box,
        tool: &ToolIntegration,
        ui: &ToolsUi,
        health: Option<&str>,
    ) {
        let key = format!("provider.connection.{}", tool.id);
        let details = gtk::Expander::new(Some("Connection details"));
        details.set_expanded(self.with_state(|state| state.settings_details_open.contains(&key)));
        let weak = self.downgrade();
        details.connect_expanded_notify(move |details| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| {
                    if details.is_expanded() {
                        state.settings_details_open.insert(key.clone());
                    } else {
                        state.settings_details_open.remove(&key);
                    }
                });
            }
        });
        let facts = vbox(8, &[]);
        widgets::pad(&facts, 12, None, None);
        // Amp's on state needs a ready account, so it shows the account line;
        // it has no Check again, its one action is Open Cloud connections.
        if (tool.supported && !super::external::is_external(&tool.id))
            || matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot" | "amp")
        {
            facts.append(&text(
                &format!("Agent Relay account: {}", account_health_label(health)),
                &["settings-row-detail"],
            ));
        }
        facts.append(&text(&tool.status_label, &["caption", "secondary"]));
        facts.append(&text(&tool.detail, &["settings-row-detail"]).wrapped());
        if (tool.supported && !super::external::is_external(&tool.id)) || matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot") {
            let check = button(
                if ui.checking {
                    "Checking…"
                } else {
                    "Check again"
                },
                &[],
                self.on(App::check_tools),
            );
            check.set_sensitive(!ui.busy && self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsCheck::NAME));
            let checked = hbox(8, &[]);
            checked.append(&check);
            if let Some(time) = &ui.last_checked {
                checked.append(&text(
                    &format!("Checked at {time}"),
                    &["caption", "secondary"],
                ));
            }
            facts.append(&checked);
        }
        details.set_child(Some(&facts));
        column.append(&details);
    }
}
