//! Whether sessions are shown teammates' overlapping work as their turn
//! ends. desktop-core keeps it (`coordination_advisories.*`), off by
//! default. The Mac's row of the same name matches it.

use super::app::App;
use super::settings_page::pending_toggle_row;
use super::widgets::settings_divider;
use crate::models::{COORDINATION_ADVISORIES_DETAIL, COORDINATION_ADVISORIES_TITLE};
use agent_relay_desktop_core::contract::coordination::CoordinationAdvisoriesParams;
use agent_relay_desktop_core::contract::method;
use agent_relay_node::relay::contract::NoParams;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    /// Reads the setting; a core built without an account never answers,
    /// and the row stays hidden.
    pub(super) fn load_coordination_advisories(&self) {
        self.once::<method::CoordinationAdvisoriesStatus>(
            "coordination_advisories",
            NoParams {},
            App::load_coordination_advisories,
            |app, result| {
                if let Ok(state) = result {
                    app.update_state(|s| s.coordination_advisories = Some(state.enabled));
                    app.settings_changed();
                }
            },
        );
    }

    pub(super) fn append_coordination_advisories_row(&self, group: &gtk::Box) {
        let Some((enabled, pending)) = self.with_state(|state| {
            let confirmed = state.coordination_advisories?;
            Some((
                state.coordination_advisories_wanted.shown(confirmed),
                state.coordination_advisories_wanted.is_pending(),
            ))
        }) else {
            return;
        };
        let weak = self.downgrade();
        settings_divider(group, 62);
        group.append(&pending_toggle_row(
            COORDINATION_ADVISORIES_TITLE,
            COORDINATION_ADVISORIES_DETAIL,
            "system-users-symbolic",
            enabled,
            true,
            pending,
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.change_coordination_advisories(enabled);
                }
            },
        ));
    }

    /// The choice shows at once; the last click wins, and only its failure
    /// reverts and shows in the pane's banner.
    fn change_coordination_advisories(&self, enabled: bool) {
        let token = self.update_state(|state| {
            state.settings_error = None;
            state.coordination_advisories_wanted.begin(enabled)
        });
        self.render_settings();
        self.call::<method::CoordinationAdvisoriesSet>(
            CoordinationAdvisoriesParams { enabled },
            move |app, result| {
                let latest = app.update_state(|state| {
                    if let Ok(saved) = &result {
                        state.coordination_advisories = Some(saved.enabled);
                    }
                    let latest = state.coordination_advisories_wanted.finish(token);
                    if latest {
                        if let Err(error) = &result {
                            state.settings_error = Some(format!(
                                "Agent Relay could not change the turn-end notes setting: {}",
                                error.message
                            ));
                        }
                    }
                    latest
                });
                if latest {
                    app.settings_changed();
                }
            },
        );
    }
}
