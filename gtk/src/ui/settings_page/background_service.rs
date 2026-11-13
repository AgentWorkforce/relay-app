//! The background service rows and actions (the systemd user unit on Linux,
//! the logon task on Windows), and keeping agents connected
//! after the app quits.
use super::*;
use crate::service::wording as service_wording;
use crate::ui::desktop::Response;

impl App {
    /// Quits. With Keep agents connected after quitting off, the core stops
    /// with the app, so a signed-in person is asked first and may keep their
    /// agents connected instead.
    pub fn quit(&self) {
        let signed_in = self.with_state(|state| state.account_connected());
        if !signed_in || self.prefs().keeps_agents_connected_after_quit() {
            self.inner.desktop.quit();
            return;
        }
        self.show_window();
        let asked = self.inner.desktop.ask(
            &self.inner.window,
            Confirmation::new(
                service_wording::QUIT_OFFLINE_TITLE,
                service_wording::QUIT_OFFLINE_DETAIL,
                service_wording::QUIT_ANYWAY,
            )
            .destructive()
            .alternative(service_wording::KEEP_AGENTS_CONNECTED),
        );
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let response = asked.await;
            let Some(app) = weak.upgrade() else {
                return;
            };
            match response {
                Response::Cancel => {}
                Response::Action => app.inner.desktop.quit(),
                Response::Alternative => {
                    let _ = app
                        .inner
                        .prefs
                        .borrow_mut()
                        .set_keeps_agents_connected_after_quit(true);
                    app.inner.desktop.quit();
                }
            }
        });
    }

    pub(super) fn background_service_row(&self) -> gtk::Box {
        let (state, error) =
            self.with_state(|state| (state.service_state, state.service_error.clone()));
        let keeps_connected = self.prefs().keeps_agents_connected_after_quit();
        let gui_startup = self.with_state(|state| state.login_item);
        let mut extra: Vec<gtk::Widget> = Vec::new();
        let accessory: Option<gtk::Widget> = match state {
            None => {
                let spinner = gtk::Spinner::new();
                spinner.start();
                Some(spinner.upcast())
            }
            Some(service) if !service.installed => {
                extra.push(notice(
                    if keeps_connected {
                        service_wording::NOT_INSTALLED_KEEPS_CONNECTED
                    } else {
                        service_wording::NOT_INSTALLED
                    },
                    Tone::Warn,
                ));
                None
            }
            Some(service) if gui_startup.is_some_and(|gui| gui != service.enabled) => {
                extra.push(notice(
                    "The app and background service have different login settings. Choose Open at login to start both, or turn the switch off to schedule neither.",
                    Tone::Warn,
                ));
                Some(button("Open at login", &["primary"], self.on(App::enable_service)).upcast())
            }
            Some(_) => None,
        };
        if let Some(service) = state {
            extra.insert(0, text(service.label(), &["caption"]).upcast());
        }
        if let Some(error) = error {
            extra.push(notice(&error, Tone::Error));
        }
        settings_row(
            "Background service",
            "Starts with Open at login to keep your agents connected and uploads running.",
            "system-run-symbolic",
            accessory.as_ref(),
            &extra,
        )
        .0
    }

    pub fn refresh_service(&self) {
        let service = self.inner.service.clone();
        self.blocking(
            move || service.state(),
            |app, state| {
                app.update_state(|current| current.service_state = Some(state));
                app.reconcile_startup();
                if app.page() == Page::Settings(Pane::General) {
                    app.render_settings();
                }
            },
        );
    }

    pub fn enable_service(&self) {
        self.change_autostart(true);
    }

    pub fn change_keep_connected(&self, enabled: bool) {
        if self.inner.updating.get() {
            return;
        }
        let _ = self
            .inner
            .prefs
            .borrow_mut()
            .set_keeps_agents_connected_after_quit(enabled);
        self.render_settings();
    }
}
