//! What the sign-in screen offers when an attempt ended needing the person
//! to act: continue past schedules that could not be checked, or create a
//! workspace first. The Mac's `SignInView` matches these.

use super::app::App;
use super::widgets::{self, hbox};
use crate::models::workspace_settings_url;
use agent_relay_node::relay::state::AccountAction;
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    pub(super) fn append_account_action(
        &self,
        panel: &gtk::Box,
        action: Option<AccountAction>,
        starting: bool,
    ) {
        match action {
            Some(AccountAction::AcknowledgeUninspectedSchedules) => {
                let anyway = widgets::button(
                    "Continue anyway",
                    &[],
                    self.on(|app| app.sign_in(true, None)),
                );
                anyway.set_tooltip_text(Some(
                    "Sign in although an earlier uploader's schedules could not be checked",
                ));
                anyway.set_sensitive(!starting);
                panel.append(&anyway);
            }
            Some(AccountAction::CreateWorkspace) => {
                let actions = hbox(12, &[]);
                let dashboard = widgets::button(
                    "Open dashboard",
                    &["primary"],
                    self.on(|app| {
                        let site = app.with_state(|state| state.account().site_url);
                        if let Some(url) = workspace_settings_url(site.as_deref()) {
                            app.open_url(&url);
                        }
                    }),
                );
                actions.append(&dashboard);
                let again =
                    widgets::button("Try again", &[], self.on(|app| app.sign_in(false, None)));
                again.set_sensitive(
                    self.inner.bridge.offers(agent_relay_desktop_core::contract::method::AccountSignIn::NAME) && !starting,
                );
                actions.append(&again);
                panel.append(&actions);
            }
            None => {}
        }
    }
}
