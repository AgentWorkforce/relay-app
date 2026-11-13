//! The per-computer teammate native input preference. The Mac's row of the
//! same name under Messages matches it.

use super::app::App;
use super::settings_page::{pending_toggle_row, SettingRow};
use super::widgets::settings_divider;
use crate::models::{TEAMMATE_NATIVE_INPUT_DETAIL, TEAMMATE_NATIVE_INPUT_TITLE};
use agent_relay_node::relay::contract::agents::SettingsChange;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    pub(super) fn append_teammate_native_input_row(&self, group: &gtk::Box) {
        let Some(enabled) = self.with_state(|state| {
            state
                .settings
                .as_ref()
                .and_then(|settings| settings.teammate_native_input)
        }) else {
            return;
        };
        let weak = self.downgrade();
        settings_divider(group, 62);
        group.append(&pending_toggle_row(
            TEAMMATE_NATIVE_INPUT_TITLE,
            TEAMMATE_NATIVE_INPUT_DETAIL,
            "mail-unread-symbolic",
            enabled,
            true,
            self.with_state(|state| state.setting_pending("teammate_native_input")),
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.set_setting(
                        SettingsChange {
                            teammate_native_input: Some(enabled),
                            ..SettingsChange::default()
                        },
                        SettingRow::Pane,
                    );
                }
            },
        ));
    }
}
