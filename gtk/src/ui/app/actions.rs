//! The application's actions: what the tray, notifications and keyboard
//! shortcuts reach.

use super::*;

impl App {
    /// The actions the tray, notifications and keyboard shortcuts reach.
    pub fn install_actions(&self, application: &gtk::Application) {
        let simple: [(&str, Action); 13] = [
            ("show", |app| app.show_window()),
            ("sessions", |app| app.go(Page::Sessions)),
            ("settings", |app| app.go(Page::Settings(Pane::General))),
            ("account", |app| app.go(Page::Settings(Pane::Account))),
            ("pause", |app| app.set_paused(true)),
            ("resume", |app| app.set_paused(false)),
            ("refresh", |app| app.refresh_page()),
            ("find", |app| app.focus_search()),
            ("message", |app| app.show_message_composer("")),
            ("chat", |app| app.show_chat_page(None)),
            ("check-updates", |app| app.check_updates(true)),
            ("sign-in", |app| app.sign_in(false, None)),
            ("quit", |app| app.quit()),
        ];
        for (name, run) in simple {
            let action = gio::SimpleAction::new(name, None);
            let run = self.on(run);
            action.connect_activate(move |_, _| run());
            application.add_action(&action);
        }
        let open_link = gio::SimpleAction::new("open-link", Some(glib::VariantTy::STRING));
        let weak = self.downgrade();
        open_link.connect_activate(move |_, value| {
            if let (Some(app), Some(url)) = (weak.upgrade(), value.and_then(|value| value.str())) {
                app.open_url(url);
            }
        });
        application.add_action(&open_link);
        for (action, accels) in [
            ("app.refresh", "<Control>r"),
            ("app.find", "<Control>f"),
            ("app.settings", "<Control>comma"),
            ("app.message", "<Control>n"),
            ("app.quit", "<Control>q"),
        ] {
            application.set_accels_for_action(action, &[accels]);
        }
        self.install_zoom_actions(application);
    }
}
