//! The notices around the pages: the upgrade notice above the status bar,
//! and under the header the sign-in-again notice, shown while the core has
//! no Cloud session to act as the person (the Mac's `signInAgainBanner`).

use super::{upgrade_notice, Shell};
use crate::models;
use crate::ui::app::App;
use crate::ui::widgets::{self, hbox, text, vbox};
use agent_relay_desktop_core::contract::account::{Flow, SignInRequest};
use agent_relay_node::relay::state::SignInState;
use agent_relay_desktop_core::contract::account::SharingMode;
use gtk::prelude::*;
use gtk4 as gtk;

/// The sign-in-again notice's widget name.
pub const SIGN_IN_AGAIN: &str = "sign-in-again";

impl App {
    /// Shows each notice when it applies.
    pub(super) fn render_notices(&self, shell: &Shell) {
        let upgrade = self.with_state(|state| state.relaycast_upgrade.clone());
        upgrade_notice::render(&shell.upgrade_notice, &shell.upgrade_text, upgrade.as_ref());
        let (shown, renewing) = self.with_state(|state| {
            let renewing = state.renew_requested || state.approving_change();
            (
                models::needs_sign_in_again(&state.account(), renewing),
                renewing,
            )
        });
        let (notice, button) = self.sign_in_notice(&shell.root);
        notice.set_visible(shown);
        button.set_label(match renewing {
            true => models::SIGN_IN_AGAIN_WAITING,
            false => models::SIGN_IN_AGAIN_ACTION,
        });
        button.set_sensitive(!renewing);
    }

    /// Signs in again in the browser while this computer stays connected:
    /// the core keeps a new session for Cloud calls such as session
    /// summaries, and uploads carry on.
    pub fn renew_sign_in(&self) {
        let start = self.update_state(|state| {
            if state.account_state() != SignInState::SignedIn || state.renew_requested {
                return false;
            }
            state.renew_requested = true;
            state.renew_answered = false;
            true
        });
        if !start {
            return;
        }
        self.render_status_bar();
        let params = SignInRequest {
            workspace: None,
            sharing: SharingMode::New,
            acknowledge_uninspected_schedules: false,
            flow: Some(Flow::Browser),
            provider: None,
        };
        self.call::<agent_relay_desktop_core::contract::method::AccountSignIn>(params, |app, result| {
            match result {
                Ok(report) if report.account.state == SignInState::PendingApproval => {
                    let renewing = app.update_state(|state| {
                        state.renew_answered = state.renew_requested;
                        state.renew_requested
                    });
                    if renewing {
                        app.sign_in_link(report.link);
                    }
                }
                // The core already holds a usable session.
                Ok(_) => app.update_state(|state| state.renew_requested = false),
                Err(error) => app.update_state(|state| {
                    state.renew_requested = false;
                    state.sign_in_error = Some(error.message);
                }),
            }
            app.render_status_bar();
        });
    }

    /// The sign-in-again notice and its button, made under the header the
    /// first time.
    fn sign_in_notice(&self, root: &gtk::Box) -> (gtk::Box, gtk::Button) {
        let mut child = root.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == SIGN_IN_AGAIN {
                if let (Ok(notice), Some(button)) = (
                    widget.clone().downcast::<gtk::Box>(),
                    widget
                        .last_child()
                        .and_then(|b| b.downcast::<gtk::Button>().ok()),
                ) {
                    return (notice, button);
                }
            }
            child = widget.next_sibling();
        }
        let notice = hbox(12, &["banner-warn"]);
        widgets::pad(&notice, 10, Some(24), None);
        notice.set_widget_name(SIGN_IN_AGAIN);
        let words = vbox(2, &[]);
        words.set_hexpand(true);
        words.append(&text(models::SIGN_IN_AGAIN_TITLE, &["heading"]));
        let message = text(models::SIGN_IN_AGAIN_MESSAGE, &["caption", "secondary"]);
        message.set_wrap(true);
        words.append(&message);
        notice.append(&words);
        let button = gtk::Button::with_label(models::SIGN_IN_AGAIN_ACTION);
        button.add_css_class("primary");
        button.set_valign(gtk::Align::Center);
        let renew = self.on(|app| app.renew_sign_in());
        button.connect_clicked(move |_| renew());
        notice.append(&button);
        // Under the header and its separator.
        let separator = root.first_child().and_then(|header| header.next_sibling());
        root.insert_child_after(&notice, separator.as_ref());
        (notice, button)
    }
}
