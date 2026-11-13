//! What the app asks of the desktop around it: opening links in the
//! browser, notifications, modal questions and quitting. [`System`] is the
//! real desktop (Windows' own in `desktop/windows.rs`); tests stand in their own
//! [`Desktop`].

use agent_relay_node::relay::contract::connection::CoreNotification;
#[cfg(not(windows))]
use gtk::gio;
use gtk::prelude::*;
use gtk4 as gtk;
use std::future::Future;
use std::pin::Pin;

/// A question asked before something hard to undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub title: String,
    pub detail: String,
    /// The confirming button.
    pub action: String,
    /// The action loses something: Cancel is the default.
    pub destructive: bool,
    /// A second way to go ahead, offered beside the action as the default.
    pub alternative: Option<String>,
}

/// Which button answered a [`Confirmation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    Cancel,
    Action,
    Alternative,
}

impl Confirmation {
    pub fn new(title: &str, detail: &str, action: &str) -> Confirmation {
        Confirmation {
            title: title.into(),
            detail: detail.into(),
            action: action.into(),
            destructive: false,
            alternative: None,
        }
    }

    pub fn destructive(mut self) -> Confirmation {
        self.destructive = true;
        self
    }

    pub fn alternative(mut self, label: &str) -> Confirmation {
        self.alternative = Some(label.into());
        self
    }
}

pub type Answer = Pin<Box<dyn Future<Output = bool>>>;
pub type Asked = Pin<Box<dyn Future<Output = Response>>>;
pub type SessionOpened =
    Pin<Box<dyn Future<Output = Result<crate::session_navigation::Outcome, String>>>>;

pub trait Desktop {
    /// Executes a shared session plan through this platform host adapter.
    fn open_session(
        &self,
        _plan: agent_relay_desktop_core::session_locations::SessionOpenPlan,
    ) -> SessionOpened {
        Box::pin(async { Err("Native session opening is unavailable on this desktop.".into()) })
    }
    /// Opens a fixed app link, web link or file URI in its registered handler.
    /// Returns whether the native opening handoff was started.
    fn open_uri(&self, uri: &str) -> bool;
    /// Open a known app handoff, falling back to its web page on launch failure.
    /// System implementations wait for launcher results off the UI thread.
    fn open_app_uri(&self, uri: &str, fallback: &str) {
        if !self.open_uri(uri) { self.open_uri(fallback); }
    }
    fn has_uri_handler(&self, scheme: &str) -> bool;
    /// Starts an interactive CLI resume command in a terminal.
    fn open_terminal(&self, command: &str) -> Result<(), String>;
    /// Shows the core's notification; clicking it opens its link, else the
    /// window.
    fn notify(&self, notification: &CoreNotification);
    /// Asks `confirmation` over `parent`; the button that answered.
    fn ask(&self, parent: &gtk::Window, confirmation: Confirmation) -> Asked;
    /// [`Desktop::ask`], answered by the action or not.
    fn confirm(&self, parent: &gtk::Window, confirmation: Confirmation) -> Answer {
        let asked = self.ask(parent, confirmation);
        Box::pin(async move { asked.await == Response::Action })
    }
    /// Tells the person something that needs no answer.
    fn inform(&self, parent: &gtk::Window, title: &str, detail: &str);
    /// Ends the app.
    fn quit(&self);
}

#[cfg(windows)]
mod windows;

/// The desktop the app runs on.
pub struct System {
    pub application: gtk::Application,
    /// Notification clicks, which Windows reports off the main thread.
    #[cfg(windows)]
    activations: windows::Activations,
}

impl System {
    /// The desktop around `application`; on the main thread, after it
    /// started.
    pub fn new(application: &gtk::Application) -> System {
        System {
            application: application.clone(),
            #[cfg(windows)]
            activations: windows::activations(application),
        }
    }
}

impl Desktop for System {
    fn open_session(
        &self,
        plan: agent_relay_desktop_core::session_locations::SessionOpenPlan,
    ) -> SessionOpened {
        Box::pin(async move {
            gtk::gio::spawn_blocking(move || crate::session_navigation::execute(&plan))
                .await
                .map_err(|_| "The native session opening task failed.".to_string())?
        })
    }
    fn has_uri_handler(&self, scheme: &str) -> bool {
        self.platform_has_uri_handler(scheme)
    }
    fn open_terminal(&self, command: &str) -> Result<(), String> {
        self.platform_open_terminal(command)
    }

    fn open_uri(&self, uri: &str) -> bool {
        self.platform_open_uri(uri)
    }

    fn open_app_uri(&self, uri: &str, fallback: &str) {
        self.platform_open_app_uri(uri, fallback);
    }

    fn notify(&self, notification: &CoreNotification) {
        self.platform_notify(notification)
    }

    fn ask(&self, parent: &gtk::Window, confirmation: Confirmation) -> Asked {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(parent)
            .modal(true)
            .message_type(gtk::MessageType::Question)
            .buttons(gtk::ButtonsType::None)
            .text(&confirmation.title)
            .secondary_text(&confirmation.detail)
            .build();
        let cancel = dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        let confirm = dialog.add_button(&confirmation.action, gtk::ResponseType::Ok);
        confirm.add_css_class(if confirmation.destructive {
            "destructive"
        } else {
            "primary"
        });
        let mut default = if confirmation.destructive {
            gtk::ResponseType::Cancel
        } else {
            gtk::ResponseType::Ok
        };
        let mut buttons = vec![cancel, confirm];
        if let Some(label) = &confirmation.alternative {
            let alternative = dialog.add_button(label, gtk::ResponseType::Apply);
            alternative.add_css_class("primary");
            buttons.push(alternative);
            default = gtk::ResponseType::Apply;
        }
        // Keep actions clear of the dialog edges and of one another.
        let last = buttons.len() - 1;
        for (index, button) in buttons.iter().enumerate() {
            button.set_margin_top(16);
            button.set_margin_bottom(20);
            button.set_margin_start(if index == 0 { 24 } else { 6 });
            button.set_margin_end(if index == last { 24 } else { 6 });
        }
        dialog.set_default_response(default);
        let (answer, answered) = tokio::sync::oneshot::channel::<Response>();
        let answer = std::cell::Cell::new(Some(answer));
        dialog.connect_response(move |dialog, response| {
            if let Some(answer) = answer.take() {
                let _ = answer.send(match response {
                    gtk::ResponseType::Ok => Response::Action,
                    gtk::ResponseType::Apply => Response::Alternative,
                    _ => Response::Cancel,
                });
            }
            dialog.close();
        });
        dialog.present();
        Box::pin(async move { answered.await.unwrap_or(Response::Cancel) })
    }

    fn inform(&self, parent: &gtk::Window, title: &str, detail: &str) {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(parent)
            .modal(true)
            .message_type(gtk::MessageType::Info)
            .buttons(gtk::ButtonsType::Ok)
            .text(title)
            .secondary_text(detail)
            .build();
        dialog.connect_response(|dialog, _| dialog.close());
        dialog.present();
    }

    fn quit(&self) {
        self.application.quit();
    }
}

#[cfg(not(windows))]
impl System {
    fn platform_open_app_uri(&self, uri: &str, fallback: &str) {
        let (uri, fallback) = (uri.to_string(), fallback.to_string());
        let opener = if cfg!(target_os = "linux") { "xdg-open" } else { "open" };
        std::thread::spawn(move || { open_app_link(opener.as_ref(), &uri, &fallback); });
    }

    fn platform_has_uri_handler(&self, scheme: &str) -> bool {
        gio::AppInfo::default_for_uri_scheme(scheme).is_some()
    }

    fn platform_open_terminal(&self, command: &str) -> Result<(), String> {
        // Explicit argv conventions; never evaluate a terminal preference as shell code.
        for (terminal, separator) in [
            ("xdg-terminal-exec", "--"),
            ("x-terminal-emulator", "-e"),
            ("gnome-terminal", "--"),
            ("konsole", "-e"),
            ("xfce4-terminal", "-x"),
            ("xterm", "-e"),
        ] {
            match std::process::Command::new(terminal)
                .args([separator, "/bin/sh", "-lc", command])
                .stdin(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    std::thread::spawn(move || child.wait());
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(
            "No supported terminal found. Copy the resume command and run it in your terminal."
                .into(),
        )
    }

    fn platform_open_uri(&self, uri: &str) -> bool {
        let opener = if cfg!(target_os = "linux") {
            "xdg-open"
        } else {
            "open"
        };
        // Reaped on its own thread: the browser outlives the opener.
        if let Ok(mut child) = std::process::Command::new(opener)
            .arg(uri)
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || child.wait());
            true
        } else {
            false
        }
    }

    fn platform_notify(&self, notification: &CoreNotification) {
        let title = if notification.title.is_empty() {
            crate::build_profile::PRODUCT_NAME
        } else {
            &notification.title
        };
        let shown = gio::Notification::new(title);
        if !notification.body.is_empty() {
            shown.set_body(Some(&notification.body));
        }
        match notification.link.as_deref().filter(|link| !link.is_empty()) {
            Some(link) => {
                shown.set_default_action_and_target_value("app.open-link", Some(&link.to_variant()))
            }
            None => shown.set_default_action("app.show"),
        }
        self.application.send_notification(None, &shown);
    }
}

#[cfg(windows)]
impl System {
    fn platform_open_app_uri(&self, uri: &str, fallback: &str) {
        windows::open_app_uri(uri, fallback);
    }
    fn platform_has_uri_handler(&self, scheme: &str) -> bool {
        windows::has_uri_handler(scheme)
    }

    fn platform_open_terminal(&self, command: &str) -> Result<(), String> {
        windows::open_terminal(command)
    }

    fn platform_open_uri(&self, uri: &str) -> bool {
        windows::open_uri(uri)
    }

    fn platform_notify(&self, notification: &CoreNotification) {
        windows::notify(&self.activations, notification)
    }
}

#[cfg(not(windows))]
fn open_app_link(opener: &std::ffi::OsStr, uri: &str, fallback: &str) -> bool {
    let launch = |link: &str| {
        std::process::Command::new(opener)
            .arg(link)
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    launch(uri) || launch(fallback)
}

#[cfg(all(test, not(windows)))]
mod app_link_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn launcher_exit_failure_opens_web_fallback_and_success_does_not() {
        let root = tempfile::tempdir().unwrap();
        let opener = root.path().join("opener");
        let native = "capy://app/new?prompt=Set%20me%20up";
        let web = "https://capy.ai/new?prompt=Set%20me%20up";
        for fail in [false, true] {
            std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$0.log\"\nif [ \"$1\" = '{native}' ] && {fail}; then exit 1; fi\nexit 0\n")).unwrap();
            std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(open_app_link(opener.as_os_str(), native, web));
            let log = opener.with_extension("log");
            let expected = if fail { format!("{native}\n{web}\n") } else { format!("{native}\n") };
            assert_eq!(std::fs::read_to_string(&log).unwrap(), expected);
            std::fs::remove_file(log).unwrap();
        }
    }
}
