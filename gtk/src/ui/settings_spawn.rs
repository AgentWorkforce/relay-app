//! Settings › Relay: spawning, Agent Relay starting agents on this
//! computer (`relay::spawn`).

use super::app::App;
use super::settings_page::{pending_toggle_row, switch_accessory, SettingRow};
use super::state::{Page, Pane};
use super::widgets::{self, notice, settings_divider, settings_row, settings_section, Tone};
use crate::models::{
    spawn_claim_can_retry, spawn_claim_line, spawn_full_access, spawn_full_access_change,
    spawn_full_access_detail, spawn_full_access_harnesses, spawn_full_access_title,
    spawn_max_agents_input, spawn_status_is_healthy, spawn_status_line, SPAWN_ACCEPT_DETAIL,
    SPAWN_ACCEPT_TITLE, SPAWN_CLAIM_RETRY, SPAWN_DESCRIPTION, SPAWN_MAX_AGENTS_DEFAULT,
    SPAWN_MAX_AGENTS_DETAIL, SPAWN_MAX_AGENTS_TITLE, SPAWN_NATIVE_INPUT_DETAIL,
    SPAWN_NATIVE_INPUT_TITLE, SPAWN_NO_LIMIT, SPAWN_REQUESTER_APPROVALS_DETAIL,
    SPAWN_REQUESTER_APPROVALS_TITLE, SPAWN_TEAMMATES_DETAIL, SPAWN_TEAMMATES_TITLE, SPAWN_TITLE,
};
use agent_relay_node::relay::contract::agents::{SettingsChange, SettingsValues};
use agent_relay_node::relay::contract::{method, NoParams};
use agent_relay_node::relay::state::{SpawnClaimState, SpawnStatus};
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    /// Agent Relay starting agents here, headless. Whether teammates may
    /// spawn here and the maximum show only while spawns are accepted.
    /// Provider-specific permissions live on each provider’s page. `None` with a
    /// core that does not spawn.
    pub(super) fn spawn_section(&self) -> Option<gtk::Box> {
        let (accepts, settings, spawn) = self.with_state(|state| {
            (
                state
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.accept_spawns),
                state.settings.clone(),
                state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.spawn.clone()),
            )
        });
        let accepts = accepts?;
        let (section, group) = settings_section(SPAWN_TITLE, Some(SPAWN_DESCRIPTION));
        let switch = gtk::Switch::new();
        crate::ui::widgets::action_cursor(&switch);
        switch.set_active(accepts);
        switch.set_valign(gtk::Align::Center);
        widgets::name(&switch, SPAWN_ACCEPT_TITLE);
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                app.set_setting(
                    SettingsChange {
                        accept_spawns: Some(switch.is_active()),
                        ..SettingsChange::default()
                    },
                    SettingRow::Pane,
                );
            }
        });
        let mut extra: Vec<gtk::Widget> = Vec::new();
        if accepts {
            if let Some(line) = spawn_status_line(spawn.as_ref()) {
                let healthy = spawn_status_is_healthy(spawn.as_ref());
                extra.push(notice(&line, if healthy { Tone::Ok } else { Tone::Warn }));
            }
        }
        group.append(
            &settings_row(
                SPAWN_ACCEPT_TITLE,
                SPAWN_ACCEPT_DETAIL,
                "system-run-symbolic",
                Some(&switch_accessory(
                    &switch,
                    self.with_state(|state| state.setting_pending("accept_spawns")),
                )),
                &extra,
            )
            .0,
        );
        if !accepts {
            return Some(section);
        }
        settings_divider(&group, 62);
        group.append(&self.spawn_teammates_row(settings.as_ref(), spawn.as_ref()));
        settings_divider(&group, 62);
        let weak = self.downgrade();
        group.append(&pending_toggle_row(
            SPAWN_NATIVE_INPUT_TITLE,
            SPAWN_NATIVE_INPUT_DETAIL,
            "mail-unread-symbolic",
            settings
                .as_ref()
                .and_then(|settings| settings.spawn_native_input)
                .unwrap_or(true),
            true,
            self.with_state(|state| state.setting_pending("spawn_native_input")),
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.set_setting(
                        SettingsChange {
                            spawn_native_input: Some(enabled),
                            ..SettingsChange::default()
                        },
                        SettingRow::Pane,
                    );
                }
            },
        ));
        let native_input = settings
            .as_ref()
            .and_then(|settings| settings.spawn_native_input)
            .unwrap_or(true);
        if native_input {
            settings_divider(&group, 62);
            let weak = self.downgrade();
            group.append(&pending_toggle_row(
                SPAWN_REQUESTER_APPROVALS_TITLE,
                SPAWN_REQUESTER_APPROVALS_DETAIL,
                "security-high-symbolic",
                settings
                    .as_ref()
                    .and_then(|settings| settings.spawn_requester_approvals)
                    .unwrap_or(false),
                true,
                self.with_state(|state| state.setting_pending("spawn_requester_approvals")),
                move |enabled| {
                    if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                        app.set_setting(
                            SettingsChange {
                                spawn_requester_approvals: Some(enabled),
                                ..SettingsChange::default()
                            },
                            SettingRow::Pane,
                        );
                    }
                },
            ));
        }
        settings_divider(&group, 62);
        group.append(
            &self.spawn_max_agents_row(
                settings
                    .as_ref()
                    .and_then(|settings| settings.spawn_max_agents)
                    .unwrap_or(SPAWN_MAX_AGENTS_DEFAULT),
            ),
        );

        Some(section)
    }

    /// Provider-specific spawn permissions, alongside that provider's setup.
    pub(in crate::ui) fn provider_spawn_settings(&self, column: &gtk::Box, id: &str) {
        let (settings, spawn) = self.with_state(|state| {
            (
                state.settings.clone(),
                state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.spawn.clone()),
            )
        });
        if !settings
            .as_ref()
            .and_then(|settings| settings.accept_spawns)
            .unwrap_or(false)
        {
            return;
        }
        for harness in spawn_full_access_harnesses(spawn.as_ref())
            .into_iter()
            .filter(|harness| harness.id == id)
        {
            let weak = self.downgrade();
            let id = harness.id.clone();
            let (section, group) = settings_section("Spawned agents", None);
            group.append(&pending_toggle_row(
                &spawn_full_access_title(&harness),
                &spawn_full_access_detail(&harness),
                "changes-allow-symbolic",
                spawn_full_access(settings.as_ref(), &harness),
                true,
                self.with_state(|state| state.setting_pending("spawn_harnesses")),
                move |enabled| {
                    if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                        app.set_setting(spawn_full_access_change(&id, enabled), SettingRow::Pane);
                    }
                },
            ));
            column.append(&section);
        }
    }

    /// Whether teammates may spawn here, and under it whether Agent Relay
    /// Cloud records this computer's spawn node as the person's.
    fn spawn_teammates_row(
        &self,
        settings: Option<&SettingsValues>,
        spawn: Option<&SpawnStatus>,
    ) -> gtk::Box {
        let switch = gtk::Switch::new();
        widgets::action_cursor(&switch);
        switch.set_active(
            settings
                .and_then(|settings| settings.spawn_allow_teammates)
                .unwrap_or(false),
        );
        switch.set_valign(gtk::Align::Center);
        widgets::name(&switch, SPAWN_TEAMMATES_TITLE);
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                let change = SettingsChange {
                    spawn_allow_teammates: Some(switch.is_active()),
                    ..SettingsChange::default()
                };
                app.set_setting(change, SettingRow::Pane);
            }
        });
        let claim = spawn.and_then(|spawn| spawn.claim.as_ref());
        let mut extra: Vec<gtk::Widget> = spawn_claim_line(claim)
            .map(|line| {
                let claimed = claim.map(|claim| claim.state) == Some(SpawnClaimState::Claimed);
                notice(&line, if claimed { Tone::Ok } else { Tone::Warn })
            })
            .into_iter()
            .collect();
        if spawn_claim_can_retry(claim) {
            let retry = widgets::button(
                SPAWN_CLAIM_RETRY,
                &["primary"],
                self.on(App::retry_spawn_claim),
            );
            retry.set_halign(gtk::Align::Start);
            extra.push(retry.upcast());
        }
        settings_row(
            SPAWN_TEAMMATES_TITLE,
            SPAWN_TEAMMATES_DETAIL,
            "system-users-symbolic",
            Some(&switch_accessory(
                &switch,
                self.with_state(|state| state.setting_pending("spawn_allow_teammates")),
            )),
            &extra,
        )
        .0
    }

    /// Asks Agent Relay Cloud again to record this computer's spawn node as
    /// the person's (`spawn.claim.retry`); the status shows the answer.
    pub fn retry_spawn_claim(&self) {
        self.call::<method::SpawnClaimRetry>(NoParams {}, |app, result| {
            if let Err(error) = result {
                app.show_error(&error.message);
            }
        });
    }

    /// Tracks whether the cap field has keyboard focus, and gives it back
    /// after a status redraw rebuilt Settings while someone was typing.
    fn keep_spawn_cap_focus(&self, entry: &gtk::Entry, restore: bool) {
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_enter(move |_| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                app.update_state(|state| state.spawn_cap.focused = true);
            }
        });
        let weak = self.downgrade();
        let field = entry.downgrade();
        focus.connect_leave(move |_| {
            let (weak, field) = (weak.clone(), field.clone());
            // A status redraw destroys the field and so takes its focus,
            // with Relay still shown: that is not the person leaving. A field
            // still shown, or Settings on another page, is.
            gtk::glib::idle_add_local_once(move || {
                let Some(app) = weak.upgrade() else { return };
                let shown = field.upgrade().is_some_and(|field| field.is_mapped());
                let redrawn = !shown
                    && app.with_state(|state| matches!(state.page, Page::Settings(Pane::Relay)));
                if !redrawn {
                    app.update_state(|state| state.spawn_cap.focused = false);
                }
            });
        });
        entry.add_controller(focus);
        if restore {
            let entry = entry.clone();
            gtk::glib::idle_add_local_once(move || {
                entry.grab_focus();
                entry.set_position(-1);
            });
        }
    }

    /// How many spawned agents may run here at once.
    fn spawn_max_agents_row(&self, value: u32) -> gtk::Box {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(SPAWN_NO_LIMIT));
        entry.set_width_chars(8);
        let edit = self.with_state(|state| state.spawn_cap.clone());
        let draft = edit.draft.clone().unwrap_or_else(|| {
            if value == 0 {
                String::new()
            } else {
                value.to_string()
            }
        });
        entry.set_text(&draft);
        let weak = self.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                app.update_state(|state| state.spawn_cap.draft = Some(entry.text().to_string()));
            }
        });
        entry.set_valign(gtk::Align::Center);
        entry.set_sensitive(!edit.saving);
        self.keep_spawn_cap_focus(&entry, edit.focused && !edit.saving);
        widgets::name(&entry, SPAWN_MAX_AGENTS_TITLE);
        let weak = self.downgrade();
        let commit = move |entry: &gtk::Entry| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                // One save at a time: a second could land first and its
                // reply be overwritten by the earlier value's.
                if app.with_state(|state| state.spawn_cap.saving) {
                    return;
                }
                if let Some(next) = spawn_max_agents_input(entry.text().as_str()) {
                    app.update_state(|state| {
                        state.spawn_cap.saving = true;
                        state.spawn_cap.focused = false;
                    });
                    entry.set_sensitive(false);
                    app.set_setting(
                        SettingsChange {
                            spawn_max_agents: Some(next),
                            ..SettingsChange::default()
                        },
                        SettingRow::Pane,
                    );
                } else {
                    entry.set_text(&if value == 0 {
                        String::new()
                    } else {
                        value.to_string()
                    });
                }
            }
        };
        entry.connect_activate(commit.clone());
        // Only explicit submission writes the cap: a status redraw or pane
        // change may destroy a focused entry while its text is incomplete.
        let entry_for_save = entry.clone();
        let save = widgets::button("Save", &["primary"], move || commit(&entry_for_save));
        save.set_sensitive(!edit.saving);
        let controls = widgets::hbox(8, &[]);
        controls.append(&entry);
        controls.append(&save);
        settings_row(
            SPAWN_MAX_AGENTS_TITLE,
            SPAWN_MAX_AGENTS_DETAIL,
            "view-grid-symbolic",
            Some(controls.upcast_ref()),
            &[],
        )
        .0
    }
}
