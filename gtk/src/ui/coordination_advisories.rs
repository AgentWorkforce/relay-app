//! Whether sessions are shown teammates' overlapping work as their turn
//! ends. desktop-core keeps it (`coordination_advisories.*`), off by
//! default. The Mac's row of the same name matches it.

use super::app::App;
use super::settings_page::{pending_toggle_row, pending_toggle_row_with};
use super::widgets::{settings_divider, text, LabelStyle};
use crate::models::{
    COORDINATION_ADVISORIES_DETAIL, COORDINATION_ADVISORIES_TITLE,
    COORDINATION_NOTIFICATIONS_DETAIL, COORDINATION_NOTIFICATIONS_TITLE,
};
use agent_relay_desktop_core::contract::coordination::{
    CoordinationAdvisoriesParams, CoordinationAdvisoriesState,
    CoordinationNotificationsDisplayParams, NotificationPermission,
};
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
                    app.update_state(|s| {
                        s.coordination_advisories = Some(state.enabled);
                        s.coordination_notifications = Some(state.notifications_enabled);
                        s.coordination_notification_status = Some(state.notification_status);
                    });
                    app.settings_changed();
                    app.report_notification_display();
                }
            },
        );
    }

    /// Tells the core what the OS lets this app show, so it requests an
    /// urgent system notification only while an app can show it. Sent on
    /// connect and on the poll timer, well inside the core's 150 s window.
    pub(super) fn report_notification_display(&self) {
        if self.with_state(|s| s.coordination_advisories.is_none()) {
            return;
        }
        self.call::<method::CoordinationNotificationsDisplay>(
            CoordinationNotificationsDisplayParams {
                permission: notification_permission(),
            },
            |app, result| {
                if let Ok(state) = result {
                    app.refresh_coordination_state(state);
                }
            },
        );
    }

    /// Takes the core's answer when it differs from what the row shows.
    fn refresh_coordination_state(&self, state: CoordinationAdvisoriesState) {
        let changed = self.update_state(|s| {
            let fresh = (
                Some(state.enabled),
                Some(state.notifications_enabled),
                Some(state.notification_status),
            );
            let shown = (
                s.coordination_advisories,
                s.coordination_notifications,
                s.coordination_notification_status.clone(),
            );
            if fresh == shown {
                return false;
            }
            (
                s.coordination_advisories,
                s.coordination_notifications,
                s.coordination_notification_status,
            ) = fresh;
            true
        });
        if changed {
            self.settings_changed();
        }
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
            !pending && !self.with_state(|s| s.coordination_notifications_wanted.is_pending()),
            pending,
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.change_coordination_advisories(enabled);
                }
            },
        ));
        self.append_coordination_notifications_row(group);
    }

    fn append_coordination_notifications_row(&self, group: &gtk::Box) {
        let Some((enabled, pending, available, status)) = self.with_state(|s| {
            Some((
                s.coordination_notifications_wanted
                    .shown(s.coordination_notifications?),
                s.coordination_notifications_wanted.is_pending(),
                s.coordination_advisories == Some(true)
                    && !s.coordination_advisories_wanted.is_pending(),
                s.coordination_notification_status.clone(),
            ))
        }) else {
            return;
        };
        // The status is its own line under the detail, as on the Mac.
        let extra: Vec<gtk::Widget> = status
            .filter(|s| !s.is_empty())
            .map(|s| text(&s, &["secondary", "caption"]).wrapped().upcast())
            .into_iter()
            .collect();
        let weak = self.downgrade();
        settings_divider(group, 62);
        group.append(&pending_toggle_row_with(
            COORDINATION_NOTIFICATIONS_TITLE,
            COORDINATION_NOTIFICATIONS_DETAIL,
            "notifications-symbolic",
            enabled,
            available && !pending,
            pending,
            &extra,
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.change_coordination_notifications(enabled);
                }
            },
        ));
    }

    fn change_coordination_notifications(&self, enabled: bool) {
        let token = self.update_state(|s| {
            s.settings_error = None;
            s.coordination_notifications_wanted.begin(enabled)
        });
        self.render_settings();
        self.call::<method::CoordinationNotificationsSet>(
            CoordinationAdvisoriesParams { enabled },
            move |app, result| {
                let latest = app.update_state(|s| {
                    if let Ok(saved) = &result {
                        s.coordination_advisories = Some(saved.enabled);
                        s.coordination_notifications = Some(saved.notifications_enabled);
                        s.coordination_notification_status =
                            Some(saved.notification_status.clone());
                    }
                    let latest = s.coordination_notifications_wanted.finish(token);
                    if latest {
                        if let Err(error) = &result {
                            s.settings_error = Some(format!(
                                "Agent Relay could not change urgent system notifications: {}",
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
                        state.coordination_notifications = Some(saved.notifications_enabled);
                        state.coordination_notification_status =
                            Some(saved.notification_status.clone());
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

/// What the OS lets this app show. Linux: the desktop decides and there is
/// nothing to read, so `unknown`.
#[cfg(not(windows))]
fn notification_permission() -> NotificationPermission {
    NotificationPermission::Unknown
}

/// Windows: the toast notifier in use (tauri-winrt-notification) exposes
/// no read of the toast setting, and reading it otherwise needs a new
/// dependency, so `unknown`.
#[cfg(windows)]
fn notification_permission() -> NotificationPermission {
    NotificationPermission::Unknown
}
