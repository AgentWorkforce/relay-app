//! Settings rows for the agent hooks Agent Relay adds: Claude Code's
//! SessionStart hook, question escalation, and the Codex hook that delivers
//! into a running turn.
use super::*;
use crate::models::{
    question_escalation_row, CLAUDE_SESSION_HOOK_OFF, CLAUDE_SESSION_HOOK_ON,
    CLAUDE_SESSION_HOOK_TITLE, CLAUDE_SESSION_HOOK_TURN_OFF_HELP,
    CLAUDE_SESSION_HOOK_TURN_ON_HELP, CODEX_HOOK_OFF, CODEX_HOOK_ON, CODEX_HOOK_TITLE,
    CODEX_HOOK_TURN_OFF_HELP, CODEX_HOOK_TURN_ON_HELP, QUESTION_ESCALATION_DETAIL,
    QUESTION_ESCALATION_TITLE, QUESTION_ESCALATION_TURN_OFF_HELP,
    QUESTION_ESCALATION_TURN_ON_HELP, QUESTION_ESCALATION_UNAVAILABLE,
};

/// The question escalation row's widget name, for tests.
pub const QUESTION_ESCALATION_ROW_NAME: &str = "question-escalation";
/// The core's availability line in that row, for tests.
pub const QUESTION_ESCALATION_STATUS_NAME: &str = "question-escalation-status";

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
