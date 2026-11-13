//! Settings rows for the agent hooks Agent Relay adds: Claude Code's
//! SessionStart hook and the Codex hook that delivers into a running turn.
use super::*;
use crate::models::{
    CLAUDE_SESSION_HOOK_OFF, CLAUDE_SESSION_HOOK_ON, CLAUDE_SESSION_HOOK_TITLE,
    CLAUDE_SESSION_HOOK_TURN_OFF_HELP, CLAUDE_SESSION_HOOK_TURN_ON_HELP, CODEX_HOOK_OFF,
    CODEX_HOOK_ON, CODEX_HOOK_TITLE, CODEX_HOOK_TURN_OFF_HELP, CODEX_HOOK_TURN_ON_HELP,
};

impl App {
    /// The teammate-context hook, where Claude Code is here or chosen. As on
    /// the Mac, it is read and changed through the provider methods.
    pub(super) fn teammate_context_row(&self) -> Option<gtk::Box> {
        use crate::models::{
            TEAMMATE_CONTEXT_DETAIL, TEAMMATE_CONTEXT_TITLE, TEAMMATE_CONTEXT_TURN_OFF_HELP,
            TEAMMATE_CONTEXT_TURN_ON_HELP,
        };
        let (enabled, busy) = self.with_state(|state| {
            (state.tools.teammate_context(), state.tools.busy)
        });
        let enabled = enabled?;
        let set = |enabled: bool| self.on(move |app| app.set_teammate_context(enabled));
        let accessory = if enabled {
            let off = button("Turn Off", &[], set(false));
            off.set_tooltip_text(Some(TEAMMATE_CONTEXT_TURN_OFF_HELP));
            off
        } else {
            let on = button("Turn On", &[], set(true));
            on.set_tooltip_text(Some(TEAMMATE_CONTEXT_TURN_ON_HELP));
            on
        };
        accessory.set_sensitive(!busy);
        Some(
            settings_row(
                TEAMMATE_CONTEXT_TITLE,
                TEAMMATE_CONTEXT_DETAIL,
                if enabled {
                    "emblem-ok-symbolic"
                } else {
                    "system-users-symbolic"
                },
                Some(accessory.upcast_ref()),
                &[],
            )
            .0,
        )
    }

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
