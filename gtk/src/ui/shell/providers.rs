//! Settings navigation with the core's real provider catalog.
use super::*;
use crate::ui::tools::provider_status::ProviderStatus;
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;

/// The top bar's subtitle, hidden when empty: each provider page names
/// itself, so Providers has none.
pub(super) fn page_subtitle(subtitle: &str) -> gtk::Label {
    let label = text(subtitle, &["page-subtitle"]).ellipsized();
    label.set_visible(!subtitle.is_empty());
    label
}

impl App {
    pub fn render_sidebar(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let rows = &shell.sidebar_rows;
        clear(rows);
        let page = self.page();
        if page.section() != Section::Settings {
            return;
        }
        self.start_tools_poll();
        let update = self.with_state(|state| state.update_available());
        for (index, (title, panes)) in Pane::GROUPS.iter().enumerate() {
            if index == 1 {
                self.providers_sidebar(rows);
            }
            let heading = section_title(title);
            heading.set_margin_top(if index == 0 { 0 } else { 14 });
            rows.append(&heading);
            for pane in panes.iter().copied() {
                rows.append(&sidebar_row(
                    pane.title(),
                    RowExtras {
                        icon: Some(pane.icon()),
                        selected: page == Page::Settings(pane),
                        badge: (pane == Pane::About && update).then_some("Update"),
                        ..Default::default()
                    },
                    self.on(move |app| app.go(Page::Settings(pane))),
                ));
            }
        }
    }

    fn providers_sidebar(&self, rows: &gtk::Box) {
        let ui = self.with_state(|state| state.tools.clone());
        let health = ui
            .report
            .as_ref()
            .and_then(|report| report.account_health.as_deref());
        let count = ui.report.as_ref().map_or(0, |report| {
            report
                .tools
                .iter()
                .filter(|tool| ProviderStatus::for_tool(tool, health) == ProviderStatus::On)
                .count()
        });
        let heading = hbox(8, &[]);
        heading.set_margin_top(14);
        let title = section_title("Providers");
        let overview = gtk::Button::new();
        overview.add_css_class("flat");
        overview.set_hexpand(true);
        overview.set_child(Some(&title));
        widgets::action_cursor(&overview);
        widgets::name(&overview, "Providers");
        let open = self.on(App::open_providers);
        overview.connect_clicked(move |_| open());
        heading.append(&overview);
        let on = text(&format!("{count} on"), &["section-title"]);
        on.set_margin_end(10);
        heading.append(&on);
        rows.append(&heading);
        if let Some(report) = &ui.report {
            for tool in crate::ui::tools::visible_providers(report) {
                let selected = self.page() == Page::Settings(Pane::Tools)
                    && ui.provider_id.as_deref() == Some(tool.id.as_str());
                rows.append(&self.provider_sidebar_row(tool, health, selected));
            }
        } else {
            let note = if ui.loading {
                "Checking providers…"
            } else {
                "Check provider setup"
            };
            rows.append(&sidebar_row(
                note,
                RowExtras::default(),
                self.on(App::open_providers),
            ));
        }
        rows.append(&sidebar_row(
            "Add provider",
            RowExtras {
                icon: Some("list-add-symbolic"),
                selected: self.page() == Page::Settings(Pane::Tools)
                    && (ui.provider_overview || ui.provider_id.is_none()),
                ..Default::default()
            },
            self.on(App::open_providers),
        ));
    }

    fn provider_sidebar_row(
        &self,
        tool: &ToolIntegration,
        health: Option<&str>,
        selected: bool,
    ) -> gtk::Button {
        let state = ProviderStatus::for_tool(tool, health);
        let row = gtk::Button::new();
        widgets::add_classes(
            &row,
            &["sidebar-row", if selected { "selected" } else { "" }],
        );
        widgets::action_cursor(&row);
        row.set_widget_name(&format!("provider-row-{}", tool.id));
        let content = hbox(9, &[]);
        let mark = widgets::agent_badge(&tool.id, 22);
        mark.remove_css_class("agent-icon");
        let label = text(&tool.name, &["row-title"]).ellipsized();
        label.set_hexpand(true);
        // As on the Mac, an off provider's icon stays muted when selected;
        // only its label takes the selected row's contrast.
        if state == ProviderStatus::Off {
            mark.set_opacity(0.55);
            if !selected {
                label.set_opacity(0.62);
            }
        }
        content.append(&mark);
        content.append(&label);
        let dot = Dot::colored(6, state.color()).widget;
        if state == ProviderStatus::Off {
            dot.set_opacity(0.0);
        }
        dot.set_widget_name(&format!("provider-status-{}-{}", tool.id, state.key()));
        content.append(&dot);
        row.set_child(Some(&content));
        let detail = format!("{}: {}", tool.name, state.detail());
        widgets::name(&row, &detail);
        row.set_tooltip_text(Some(&format!(
            "{}\n{}\n{}",
            tool.status_label,
            tool.detail,
            state.detail()
        )));
        let id = tool.id.clone();
        let open = self.on(move |app| app.open_provider(&id));
        row.connect_clicked(move |_| open());
        row
    }
}
