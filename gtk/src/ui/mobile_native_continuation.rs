//! The one mobile continuation row. The Mac's `mobileContinuationRow`
//! matches it: one switch, on unless turned off, sets both native routes the
//! core reports, and until this computer's ownership is verified the switch
//! is off with a status line under it saying what it waits for.

use super::app::App;
use super::settings_page::{switch_accessory, SettingRow};
use super::widgets::{notice, settings_divider, settings_row, Tone};
use crate::models::{
    mobile_continuation_change, mobile_continuation_enabled, mobile_continuation_supported,
    MOBILE_NATIVE_CONTINUATION_DETAIL, MOBILE_NATIVE_CONTINUATION_TITLE,
    MOBILE_NATIVE_CONTINUATION_UNAVAILABLE, MOBILE_NATIVE_CONTINUATION_WAITING,
};
use agent_relay_node::relay::state::SpawnClaimState;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    pub(super) fn append_mobile_native_continuation_row(&self, group: &gtk::Box) {
        let Some((settings, claimed)) = self.with_state(|state| {
            let settings = state.settings.clone()?;
            if settings.mobile_claude_handoff.is_none()
                && settings.mobile_native_continuation.is_none()
            {
                return None;
            }
            let claimed = state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.spawn.as_ref())
                .and_then(|spawn| spawn.claim.as_ref())
                .map(|claim| claim.state)
                == Some(SpawnClaimState::Claimed);
            Some((settings, claimed))
        }) else {
            return;
        };
        let supported = mobile_continuation_supported(&settings);
        let switch = gtk::Switch::new();
        super::widgets::action_cursor(&switch);
        switch.set_active(mobile_continuation_enabled(&settings));
        switch.set_sensitive(supported && claimed);
        super::widgets::name(&switch, MOBILE_NATIVE_CONTINUATION_TITLE);
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                let change = app.with_state(|state| {
                    state
                        .settings
                        .as_ref()
                        .map(|settings| mobile_continuation_change(settings, switch.is_active()))
                });
                if let Some(change) = change {
                    app.set_setting(change, SettingRow::Pane);
                }
            }
        });
        let extra: Vec<gtk::Widget> = (supported && !claimed)
            .then(|| notice(MOBILE_NATIVE_CONTINUATION_WAITING, Tone::Neutral))
            .into_iter()
            .collect();
        let pending = self.with_state(|state| {
            state.setting_pending("mobile_claude_handoff")
                || state.setting_pending("mobile_native_continuation")
        });
        settings_divider(group, 62);
        group.append(
            &settings_row(
                MOBILE_NATIVE_CONTINUATION_TITLE,
                if supported {
                    MOBILE_NATIVE_CONTINUATION_DETAIL
                } else {
                    MOBILE_NATIVE_CONTINUATION_UNAVAILABLE
                },
                "phone-symbolic",
                Some(&switch_accessory(&switch, pending)),
                &extra,
            )
            .0,
        );
    }
}
