//! Provider-specific local session preferences.
use super::*;

const OPENING_TITLE: &str = "Local sessions";
const OPENING_ROW: &str = "Open with";
const OPENING_HELP: &str =
    "Automatic opens the desktop app when installed, otherwise Terminal. Applies only to this computer.";
const OPENING_REQUIREMENTS: &str = "Opening requirements";

impl App {
    pub(in crate::ui) fn refresh_settings_delivery(&self, page: Page) {
        match page {
            Page::Settings(Pane::Tools)
                if self.with_state(|state| {
                    !state.tools.provider_overview
                        && state.tools.provider_id.as_deref() == Some("claude")
                }) =>
            {
                self.refresh_claude_delivery()
            }
            _ => {}
        }
    }

    pub(in crate::ui) fn refresh_provider_preferences(&self) {
        self.refresh_tools();
        self.load_settings();
        self.refresh_claude_delivery();
        self.refresh_local();
    }

    /// Where local sessions open, on this computer only. A provider page shows
    /// its own row; the standalone pane lists every provider with its
    /// requirements under it.
    pub(in crate::ui) fn provider_opening_preferences(
        &self,
        column: &gtk::Box,
        provider: Option<&str>,
    ) {
        let (section, group) = settings_section(OPENING_TITLE, None);
        for (source, name) in [("claude", "Claude Code"), ("codex", "Codex")] {
            if provider.is_some_and(|id| id != source) {
                continue;
            }
            let title = if provider.is_some() {
                OPENING_ROW
            } else {
                name
            };
            group.append(&self.provider_opening_row(source, name, title));
        }
        if provider.is_none_or(|id| id == "grok") {
            let title = if provider.is_some() {
                OPENING_ROW
            } else {
                "Grok"
            };
            let terminal = text("Terminal", &["secondary"]);
            let row = settings_row(
                title,
                "",
                "utilities-terminal-symbolic",
                Some(terminal.upcast_ref()),
                &[],
            );
            group.append(&row.0);
        }
        column.append(&section);
        if provider.is_none() {
            self.provider_opening_requirements(column, None);
        }
    }

    /// What opening a local session needs, collapsed with the other diagnostics.
    pub(in crate::ui) fn provider_opening_requirements(
        &self,
        column: &gtk::Box,
        provider: Option<&str>,
    ) {
        if provider.is_some_and(|id| !matches!(id, "claude" | "codex" | "grok")) {
            return;
        }
        let requirements = gtk::Expander::new(Some(OPENING_REQUIREMENTS));
        let detail = vbox(10, &[]);
        detail.set_margin_top(10);
        // The shared planner returns to a verified live host before considering resume.
        detail.append(&text("Open returns to the original session when the host exposes an exact navigation API. Closed sessions resume in the recorded application with the matching CLI and original folder. Unsupported hosts and runtimes show an explanation.", &["settings-row-detail"]).wrapped());
        let actions = hbox(12, &[]);
        if provider != Some("grok") {
            actions.append(&button(
                "Check installed apps",
                &[],
                self.on(|app| app.render_settings()),
            ));
        }
        if provider.is_none_or(|id| id == "claude") {
            actions.append(&button(
                "Set up Claude Code",
                &[],
                self.on(|app| app.open_url("https://code.claude.com/docs/en/quickstart")),
            ));
        }
        detail.append(&actions);
        let key = format!("provider-opening-{}", provider.unwrap_or("all"));
        requirements
            .set_expanded(self.with_state(|state| state.settings_details_open.contains(&key)));
        let weak = self.downgrade();
        requirements.connect_expanded_notify(move |expander| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| {
                    if expander.is_expanded() {
                        state.settings_details_open.insert(key.clone());
                    } else {
                        state.settings_details_open.remove(&key);
                    }
                });
            }
        });
        requirements.set_child(Some(&detail));
        column.append(&requirements);
    }

    fn provider_opening_row(&self, source: &'static str, name: &str, title: &str) -> gtk::Box {
        let choice = gtk::DropDown::from_strings(&["Automatic", "Terminal"]);
        choice.set_selected(
            if self.prefs().session_opening(source) == SessionOpenPreference::Terminal {
                1
            } else {
                0
            },
        );
        widgets::name(&choice, &format!("Open {name} sessions"));
        choice.set_widget_name(&format!("session-opening-{source}"));
        choice.set_tooltip_text(Some(OPENING_HELP));
        widgets::action_cursor(&choice);
        let weak = self.downgrade();
        choice.connect_selected_notify(move |choice| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                let preference = if choice.selected() == 1 {
                    SessionOpenPreference::Terminal
                } else {
                    SessionOpenPreference::Automatic
                };
                let result = app
                    .inner
                    .prefs
                    .borrow_mut()
                    .set_session_opening(source, preference);
                if let Err(error) = result {
                    app.update_state(|state| state.settings_error = Some(error.to_string()));
                    app.render_settings();
                }
            }
        });
        let detail = if source == "claude" {
            "Claude Desktop opens only with a verified provider session link. Terminal sessions return to their recorded host."
        } else if self.inner.desktop.has_uri_handler("codex") {
            "Codex is installed"
        } else {
            "Codex isn’t installed. Recorded terminal sessions can still resume in their original terminal."
        };
        settings_row(
            title,
            detail,
            "utilities-terminal-symbolic",
            Some(choice.upcast_ref()),
            &[],
        )
        .0
    }

    pub(in crate::ui) fn provider_preferences(&self, column: &gtk::Box, id: &str) {
        self.provider_message_preferences(column, id);
        if matches!(id, "codex" | "claude" | "grok") {
            self.provider_opening_preferences(column, Some(id));
        }
        if crate::ui::tools::teammate::TEAMMATE_CONTEXT_PROVIDERS.contains(&id) {
            if let Some(row) = self.teammate_context_row() {
                let (section, group) = settings_section("Teammates", None);
                group.append(&row);
                column.append(&section);
            }
        }
        if id == "claude" {
            self.claude_provider_preferences(column);
        }
    }

    fn provider_message_preferences(&self, column: &gtk::Box, id: &str) {
        let settings = self.with_state(|state| state.settings.clone());
        if let Some(settings) = settings {
            let row = match id {
                "grok" if settings.grok_leader.is_some() => Some(self.grok_leader_row()),
                "opencode" if settings.opencode_plugin.is_some() => {
                    Some(self.opencode_plugin_row())
                }
                "devin" if settings.devin_direct_delivery.is_some() => {
                    Some(self.devin_delivery_row())
                }
                "cursor" if settings.cursor_direct_delivery.is_some() => {
                    Some(self.cursor_delivery_row())
                }
                "codex" if settings.codex_stop_hook.is_some() => Some(self.codex_hook_row()),
                _ => None,
            };
            if let Some(row) = row {
                let (section, group) = settings_section("Messages", None);
                group.append(&row);
                column.append(&section);
            } else if id == "cursor" {
                let (section, group) = settings_section("Messages", None);
                let detail = if cfg!(windows) {
                    "Cursor session messaging is not available on Windows."
                } else {
                    "Start a Cursor Agent chat once, then return here to connect its messages."
                };
                group.append(
                    &settings_row(
                        "Cursor messages",
                        detail,
                        "dialog-information-symbolic",
                        None,
                        &[],
                    )
                    .0,
                );
                column.append(&section);
            }
            if matches!(id, "openclaw" | "hermes") {
                let (section, group) = settings_section("Messages", None);
                self.append_gateway_rows(&group, id);
                column.append(&section);
            }
        }
    }

    fn claude_provider_preferences(&self, column: &gtk::Box) {
        if let Some(section) = self.claude_folders_section() {
            column.append(&section);
        }
        // As on the Mac, delivery shows only while a Claude terminal
        // session exists, and the section only with a row to show.
        let (delivery, hook) = self.with_state(|state| {
            (
                state.local_sessions.iter().any(|session| {
                    session.target.kind
                        == agent_relay_node::relay::store::SessionKind::ClaudeTerminal
                }),
                state.sharing_mode() == Some(SharingMode::Selected)
                    && state
                        .settings
                        .as_ref()
                        .is_some_and(|settings| settings.claude_session_hook.is_some()),
            )
        });
        if delivery || hook {
            let (section, group) = settings_section("Messages", None);
            if delivery {
                group.append(&self.claude_delivery_row());
            }
            if hook {
                if delivery {
                    settings_divider(&group, 62);
                }
                let row = self.claude_session_hook_row();
                row.set_widget_name(super::general::CLAUDE_SESSION_PROMPT_NAME);
                group.append(&row);
            }
            column.append(&section);
        }
        if self.with_state(|state| state.connected()) {
            column.append(&self.claude_delivery_section());
        }
    }
}
