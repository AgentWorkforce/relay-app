//! Settings rows for the agent hooks Agent Relay adds: Claude Code's
//! SessionStart hook, question escalation, and the Codex hook that delivers
//! into a running turn.
use super::*;
use crate::models::{
    question_escalation_row, QuestionCoverageGroup, CLAUDE_SESSION_HOOK_OFF,
    CLAUDE_SESSION_HOOK_ON, CLAUDE_SESSION_HOOK_TITLE, CLAUDE_SESSION_HOOK_TURN_OFF_HELP,
    CLAUDE_SESSION_HOOK_TURN_ON_HELP, CODEX_HOOK_OFF, CODEX_HOOK_ON, CODEX_HOOK_TITLE,
    CODEX_HOOK_TURN_OFF_HELP, CODEX_HOOK_TURN_ON_HELP, QUESTION_COVERAGE_CLOSING,
    QUESTION_COVERAGE_TITLE, QUESTION_ESCALATION_DETAIL, QUESTION_ESCALATION_TITLE,
    QUESTION_ESCALATION_TURN_OFF_HELP, QUESTION_ESCALATION_TURN_ON_HELP,
    QUESTION_ESCALATION_UNAVAILABLE,
};

/// The question escalation row's widget name, for tests.
pub const QUESTION_ESCALATION_ROW_NAME: &str = "question-escalation";
/// The core's availability line in that row, for tests.
pub const QUESTION_ESCALATION_STATUS_NAME: &str = "question-escalation-status";
/// The disclosure holding the list of kinds of session, for tests.
pub const QUESTION_COVERAGE_NAME: &str = "question-escalation-coverage";
/// Its open state in `settings_details_open`, kept across redraws.
const QUESTION_COVERAGE_KEY: &str = "question-escalation-coverage";

impl App {
    /// Claude Code's SessionStart hook. On by default after sign-in. It never
    /// registers a session; it tells one that is not on the relay how to
    /// POST /register.
    pub(super) fn claude_session_hook_row(&self) -> gtk::Box {
        let (enabled, error) = self.with_state(|state| {
            (
                state
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.claude_session_hook)
                    == Some(true),
                state.hook_errors.claude_session.clone(),
            )
        });
        let set = |enabled: bool| {
            self.on(move |app| {
                app.set_setting(
                    SettingsChange {
                        claude_session_hook: Some(enabled),
                        ..SettingsChange::default()
                    },
                    SettingRow::ClaudeSessionHook,
                )
            })
        };
        let (detail, accessory) = if enabled {
            let off = button("Turn Off", &[], set(false));
            off.set_tooltip_text(Some(CLAUDE_SESSION_HOOK_TURN_OFF_HELP));
            (CLAUDE_SESSION_HOOK_ON, off)
        } else {
            let on = button("Turn On", &[], set(true));
            on.set_tooltip_text(Some(CLAUDE_SESSION_HOOK_TURN_ON_HELP));
            (CLAUDE_SESSION_HOOK_OFF, on)
        };
        let extra: Vec<gtk::Widget> = error
            .iter()
            .map(|error| notice(error, Tone::Error))
            .collect();
        settings_row(
            CLAUDE_SESSION_HOOK_TITLE,
            detail,
            if enabled {
                "emblem-ok-symbolic"
            } else {
                "dialog-information-symbolic"
            },
            Some(accessory.upcast_ref()),
            &extra,
        )
        .0
    }

    /// Question escalation (`question_escalation`). It shows what the core
    /// reports, with the core's own availability line; a core that does not
    /// report it gets an unavailable row, not a hidden one.
    pub(super) fn question_escalation_row(&self) -> gtk::Box {
        let (row, error) = self.with_state(|state| {
            (
                state.settings.as_ref().map(question_escalation_row),
                state.hook_errors.question_escalation.clone(),
            )
        });
        let row = row.unwrap_or(crate::models::QuestionEscalationRow {
            enabled: None,
            status: None,
            inbox: None,
            coverage: Vec::new(),
        });
        let set = |enabled: bool| {
            self.on(move |app| {
                app.set_setting(
                    SettingsChange {
                        question_escalation: Some(if enabled { "on" } else { "off" }.into()),
                        ..SettingsChange::default()
                    },
                    SettingRow::QuestionEscalation,
                )
            })
        };
        let accessory = match row.enabled {
            Some(true) => {
                let off = button("Turn Off", &[], set(false));
                off.set_tooltip_text(Some(QUESTION_ESCALATION_TURN_OFF_HELP));
                Some(off)
            }
            Some(false) => {
                let on = button("Turn On", &[], set(true));
                on.set_tooltip_text(Some(QUESTION_ESCALATION_TURN_ON_HELP));
                Some(on)
            }
            None => None,
        };
        let mut extra: Vec<gtk::Widget> = Vec::new();
        match row.enabled {
            None => extra.push(notice(QUESTION_ESCALATION_UNAVAILABLE, Tone::Neutral)),
            Some(_) => {
                // The core's own text: capped at four lines, as on the Mac.
                if let Some(status) = &row.status {
                    let line = text(status, &["settings-row-detail"]).wrapped();
                    line.set_lines(4);
                    line.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    line.set_widget_name(QUESTION_ESCALATION_STATUS_NAME);
                    extra.push(line.upcast());
                }
                if !row.coverage.is_empty() {
                    extra.push(self.question_coverage_disclosure(&row.coverage).upcast());
                    extra.push(
                        text(QUESTION_COVERAGE_CLOSING, &["settings-row-detail"])
                            .wrapped()
                            .upcast(),
                    );
                }
                if let Some(inbox) = &row.inbox {
                    extra.push(text(inbox, &["settings-row-detail"]).wrapped().upcast());
                }
            }
        }
        extra.extend(error.iter().map(|error| notice(error, Tone::Error)));
        let row = settings_row(
            QUESTION_ESCALATION_TITLE,
            QUESTION_ESCALATION_DETAIL,
            match row.enabled {
                Some(true) => "emblem-ok-symbolic",
                _ => "dialog-information-symbolic",
            },
            accessory.as_ref().map(|button| button.upcast_ref()),
            &extra,
        )
        .0;
        row.set_widget_name(QUESTION_ESCALATION_ROW_NAME);
        row
    }

    /// The kinds of session question escalation reaches, collapsed under
    /// the row; read-only. The open state survives redraws.
    fn question_coverage_disclosure(&self, groups: &[QuestionCoverageGroup]) -> gtk::Expander {
        // The title wraps, as the row's own lines do, so it never sets the
        // page's width at a large zoom.
        let title = text(QUESTION_COVERAGE_TITLE, &["settings-row-detail"]).wrapped();
        let disclosure = gtk::Expander::builder().label_widget(&title).build();
        disclosure.set_widget_name(QUESTION_COVERAGE_NAME);
        disclosure.add_css_class("settings-row-detail");
        disclosure.set_expanded(
            self.with_state(|state| state.settings_details_open.contains(QUESTION_COVERAGE_KEY)),
        );
        let weak = self.downgrade();
        disclosure.connect_expanded_notify(move |disclosure| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| {
                    if disclosure.is_expanded() {
                        state
                            .settings_details_open
                            .insert(QUESTION_COVERAGE_KEY.into());
                    } else {
                        state.settings_details_open.remove(QUESTION_COVERAGE_KEY);
                    }
                });
            }
        });
        let list = vbox(10, &[]);
        list.set_margin_top(6);
        for group in groups {
            let block = vbox(4, &[]);
            let heading = text(group.heading, &["settings-row-detail"]).wrapped();
            heading.add_css_class("settings-row-title");
            block.append(&heading);
            for entry in &group.entries {
                block.append(&text(&entry.line(), &["settings-row-detail"]).wrapped());
            }
            list.append(&block);
        }
        disclosure.set_child(Some(&list));
        disclosure
    }

    pub(in crate::ui) fn codex_hook_row(&self) -> gtk::Box {
        let (enabled, error) = self.with_state(|state| {
            (
                state
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.codex_stop_hook)
                    == Some(true),
                state.hook_errors.codex.clone(),
            )
        });
        let set = |enabled: bool| {
            self.on(move |app| {
                app.set_setting(
                    SettingsChange {
                        codex_stop_hook: Some(enabled),
                        ..SettingsChange::default()
                    },
                    SettingRow::CodexHook,
                )
            })
        };
        let (detail, accessory) = if enabled {
            let off = button("Turn Off", &[], set(false));
            off.set_tooltip_text(Some(CODEX_HOOK_TURN_OFF_HELP));
            (CODEX_HOOK_ON, off)
        } else {
            let on = button("Turn On", &[], set(true));
            on.set_tooltip_text(Some(CODEX_HOOK_TURN_ON_HELP));
            (CODEX_HOOK_OFF, on)
        };
        let extra: Vec<gtk::Widget> = error
            .iter()
            .map(|error| notice(error, Tone::Error))
            .collect();
        settings_row(
            CODEX_HOOK_TITLE,
            detail,
            if enabled {
                "emblem-ok-symbolic"
            } else {
                "dialog-information-symbolic"
            },
            Some(accessory.upcast_ref()),
            &extra,
        )
        .0
    }
}
