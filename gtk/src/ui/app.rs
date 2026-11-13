//! The application: lifecycle, core events, sign-in, links, the tray and
//! dialogs, laid out like the Mac app (`docs/workspace-layout.md`).
//!
//! Everything it shows comes from the core: the `status` snapshot and
//! events, and the results of contract methods. Every action is a contract
//! method. The app itself only draws, opens links and notifications, copies
//! to the clipboard, and manages its own systemd user service and updates.

use super::desktop::{Answer, Confirmation, Desktop};

mod connection;
mod content;
mod lifecycle;
mod links;
use super::shell::Shell;
use super::state::{Page, Pane, PendingConnect, State, View, ViewKey};
use super::theme::{Styling, Theme};
use super::tray::Tray;
use super::widgets::{self, hbox, text, vbox, LabelStyle};
use crate::bridge::{Bridge, BridgeEvent, BridgeOptions, CoreError};
use crate::models::{
    development_badge, diagnostics_report, sign_in_detail, sign_in_heading,
    STARTUP_TIMEOUT_MESSAGE, STARTUP_TITLE,
};
use crate::prefs::Preferences;
use crate::service::UserService;
use agent_relay_desktop_core::install::InstallKind;
use agent_relay_desktop_core::contract::account::SignInProvider;
use agent_relay_node::relay::contract::account::SignInLink;
use agent_relay_node::relay::contract::connection::CoreNotification;
use agent_relay_node::relay::contract::{method, Method, NoParams};
use agent_relay_node::relay::state::{SignInState, Status};
use agent_relay_desktop_core::telemetry::{
    TelemetryFailure, TelemetryOperation, TelemetryOutcome, TelemetrySurface,
};
pub(super) use connection::web_link;
use gtk::{gio, glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The application id: the D-Bus name, the desktop entry and the
/// notifications' sender.
pub const APP_ID: &str = "com.agentrelay.desktop";
pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(5);
const FIRST_UPDATE_CHECK: Duration = Duration::from_secs(20);
const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Something the app does, run by name: a read run again, an action.
pub type Action = fn(&App);

/// What the app is built from.
pub struct Options {
    pub bridge: BridgeOptions,
    /// Where the bridge, the tray and blocking work run.
    pub runtime: tokio::runtime::Handle,
    pub prefs: Preferences,
    pub service: Arc<dyn UserService>,
    pub desktop: Rc<dyn Desktop>,
    pub tray: Box<dyn Tray>,
    pub install_kind: InstallKind,
    /// The window the app draws in; it is never destroyed, only hidden.
    pub window: gtk::Window,
}

pub(super) struct Inner {
    pub(super) state: RefCell<State>,
    pub(super) bridge: Bridge,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) prefs: RefCell<Preferences>,
    pub(super) service: Arc<dyn UserService>,
    pub(super) desktop: Rc<dyn Desktop>,
    pub(super) tray: Box<dyn Tray>,
    pub(super) styling: Styling,
    pub(super) window: gtk::Window,
    /// The signed-in shell, while the connected view shows.
    pub(super) shell: RefCell<Option<Rc<Shell>>>,
    /// The sign-in progress screen's heading, detail, code and browser
    /// button.
    signing: RefCell<Option<(gtk::Label, gtk::Label, gtk::Label, gtk::Button)>>,
    /// Reads in flight by name, each with the ticket of the call that owns
    /// the entry, and the read to run again once each ends.
    in_flight: RefCell<HashMap<&'static str, u64>>,
    next_ticket: Cell<u64>,
    rerun: RefCell<HashMap<&'static str, Action>>,
    pub(super) claude_delivery_changes: Cell<usize>,
    /// Settings are being drawn: switches set while drawing change nothing.
    pub(super) updating: Cell<bool>,
    timers_started: Cell<bool>,
    pub(super) sign_in_provider: Cell<SignInProvider>,
    pub(super) search_telemetry: RefCell<HashMap<TelemetrySurface, glib::SourceId>>,
}

/// The app. Cheap to clone; every clone is the same app. Lives on the GTK
/// main thread.
#[derive(Clone)]
pub struct App {
    pub(super) inner: Rc<Inner>,
}

/// An [`App`] a callback holds without keeping it alive.
#[derive(Clone)]
pub struct WeakApp(Weak<Inner>);

impl WeakApp {
    pub fn upgrade(&self) -> Option<App> {
        self.0.upgrade().map(|inner| App { inner })
    }
}

impl App {
    /// Builds the app and starts following the core.
    pub fn new(options: Options) -> App {
        let (bridge, events) = Bridge::start(options.bridge, &options.runtime);
        let styling = Styling::start(options.prefs.appearance(), options.prefs.ui_zoom());
        let app = App {
            inner: Rc::new(Inner {
                state: RefCell::new(State::new(options.install_kind)),
                bridge,
                runtime: options.runtime,
                prefs: RefCell::new(options.prefs),
                service: options.service,
                desktop: options.desktop,
                tray: options.tray,
                styling,
                window: options.window,
                shell: RefCell::new(None),
                signing: RefCell::new(None),
                in_flight: RefCell::default(),
                next_ticket: Cell::new(0),
                rerun: RefCell::default(),
                sign_in_provider: Cell::new(SignInProvider::Google),
                claude_delivery_changes: Cell::new(0),
                updating: Cell::new(false),
                timers_started: Cell::new(false),
                search_telemetry: RefCell::default(),
            }),
        };
        let weak = app.downgrade();
        app.inner.styling.on_change(move |theme| {
            if let Some(app) = weak.upgrade() {
                app.theme_changed(theme);
            }
        });
        app.theme_changed(app.inner.styling.theme());
        app.inner.window.set_title(Some(&crate::build_profile::window_title()));
        app.prepare_zoom();
        app.inner.window.set_hide_on_close(true);
        app.follow(events);
        app.render();
        app.refresh_service();
        app
    }

    pub fn downgrade(&self) -> WeakApp {
        WeakApp(Rc::downgrade(&self.inner))
    }

    /// A callback that runs `action` on the app while it lives.
    pub(super) fn on(&self, action: impl Fn(&App) + 'static) -> impl Fn() + 'static {
        let weak = self.downgrade();
        move || {
            if let Some(app) = weak.upgrade() {
                action(&app);
            }
        }
    }

    // What tests and the shell read.

    pub fn window(&self) -> &gtk::Window {
        &self.inner.window
    }

    pub fn bridge(&self) -> &Bridge {
        &self.inner.bridge
    }

    /// Reads the window's state.
    pub fn with_state<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        read(&self.inner.state.borrow())
    }

    pub fn view(&self) -> Option<View> {
        self.with_state(|state| state.view)
    }

    pub fn page(&self) -> Page {
        self.with_state(|state| state.page)
    }

    /// The signed-in shell, while it shows.
    pub fn shell(&self) -> Option<Rc<Shell>> {
        self.inner.shell.borrow().clone()
    }

    pub fn styling(&self) -> &Styling {
        &self.inner.styling
    }

    pub fn prefs(&self) -> std::cell::Ref<'_, Preferences> {
        self.inner.prefs.borrow()
    }

    /// The reads in flight, by name.
    pub fn in_flight(&self) -> HashSet<&'static str> {
        self.inner.in_flight.borrow().keys().copied().collect()
    }

    // Lifecycle.

    // The command line of this instance or a later launch: `ui::launch`.

    pub fn activate(&self) {
        if !self.with_state(|state| state.background_requested) {
            self.show_window();
        }
        if !self.inner.timers_started.replace(true) {
            self.start_timers();
        }
    }

    fn start_timers(&self) {
        let weak = self.downgrade();
        glib::timeout_add_local(POLL_INTERVAL, move || match weak.upgrade() {
            Some(app) => {
                app.poll();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        // Both follow the preference when they fire, so turning it on or
        // off takes effect without a restart.
        glib::timeout_add_local_once(FIRST_UPDATE_CHECK, self.on(|app| app.check_updates(false)));
        let weak = self.downgrade();
        glib::timeout_add_local(UPDATE_INTERVAL, move || match weak.upgrade() {
            Some(app) => {
                app.check_updates(false);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    /// The actions the tray, notifications and keyboard shortcuts reach.
    pub fn install_actions(&self, application: &gtk::Application) {
        let simple: [(&str, Action); 12] = [
            ("show", |app| app.show_window()),
            ("sessions", |app| app.go(Page::Sessions)),
            ("settings", |app| app.go(Page::Settings(Pane::General))),
            ("account", |app| app.go(Page::Settings(Pane::Account))),
            ("pause", |app| app.set_paused(true)),
            ("resume", |app| app.set_paused(false)),
            ("refresh", |app| app.refresh_page()),
            ("find", |app| app.focus_search()),
            ("message", |app| app.show_message_composer("")),
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

    pub fn show_window(&self) {
        self.prepare_window_reveal();
        self.inner.window.present();
    }

    pub(super) fn theme_changed(&self, theme: Theme) {
        let palette = match theme {
            Theme::Light => &super::theme::LIGHT,
            Theme::Dark => &super::theme::DARK,
        };
        widgets::set_palette(palette);
        // Marks are drawn in the theme's colors when built: rebuild what
        // shows them.
        if self.with_state(|state| state.connected()) {
            self.render_page();
            if let Some(shell) = self.shell() {
                shell
                    .sessions_button
                    .set_child(Some(&widgets::header_brand()));
            }
        }
    }

    // Calls.

    /// Calls `M` on the core; `done` runs on the main thread with the
    /// answer.
    pub(super) fn call<M>(
        &self,
        params: M::Params,
        done: impl FnOnce(&App, Result<M::Result, CoreError>) + 'static,
    ) where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        let bridge = self.inner.bridge.clone();
        let answer = self
            .inner
            .runtime
            .spawn(async move { bridge.call::<M>(params).await });
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = answer
                .await
                .unwrap_or_else(|_| Err(CoreError::unavailable()));
            if let Some(app) = weak.upgrade() {
                done(&app, result);
            }
        });
    }

    /// Calls `M` for the account view shown now. An answer that arrives
    /// after that view is gone is dropped, so nothing it would start (a
    /// refresh, an error) lands in the next account's view.
    pub(super) fn view_call<M>(
        &self,
        params: M::Params,
        done: impl FnOnce(&App, Result<M::Result, CoreError>) + 'static,
    ) where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        let generation = self.with_state(|state| state.generation);
        self.call::<M>(params, move |app, result| {
            if app.with_state(|state| state.generation) == generation {
                done(app, result);
            }
        });
    }

    /// Calls `M` as the read named `name`; asked again while it is in
    /// flight, `again` runs once that answer arrives, since the core may
    /// have changed the listing after the first read began.
    pub(super) fn once<M>(
        &self,
        name: &'static str,
        params: M::Params,
        again: Action,
        done: impl FnOnce(&App, Result<M::Result, CoreError>) + 'static,
    ) where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        if !self.inner.bridge.offers(M::NAME) {
            return;
        }
        if self.inner.in_flight.borrow().contains_key(name) {
            self.inner.rerun.borrow_mut().insert(name, again);
            return;
        }
        // The read ends and any re-read asked for runs whatever the view
        // did meanwhile; only the answer's use belongs to the view that
        // asked. A view change mid-read used to leave the name in flight
        // for good, so no later read of it ever started. The entry belongs
        // to this call by ticket: a read that outlives a newer one of its
        // name never ends the newer one, or takes its re-read.
        let ticket = self.inner.next_ticket.get() + 1;
        self.inner.next_ticket.set(ticket);
        self.inner.in_flight.borrow_mut().insert(name, ticket);
        let generation = self.with_state(|state| state.generation);
        self.call::<M>(params, move |app, result| {
            let owns = app.inner.in_flight.borrow().get(name) == Some(&ticket);
            if owns {
                app.inner.in_flight.borrow_mut().remove(name);
            }
            if app.with_state(|state| state.generation) == generation {
                done(app, result);
            }
            if owns {
                let follow_up = app.inner.rerun.borrow_mut().remove(name);
                if let Some(follow_up) = follow_up {
                    follow_up(app);
                }
            }
        });
    }

    /// Runs blocking `work` off the main thread; `done` gets its answer on
    /// it.
    pub(super) fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&App, T) + 'static,
    ) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(value) = gio::spawn_blocking(work).await else {
                return;
            };
            if let Some(app) = weak.upgrade() {
                done(&app, value);
            }
        });
    }

    pub(super) fn confirm(&self, confirmation: Confirmation) -> Answer {
        self.inner.desktop.confirm(&self.inner.window, confirmation)
    }

    // The core.

    fn follow(&self, mut events: mpsc::Receiver<BridgeEvent>) {
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                app.on_event(event);
            }
        });
    }

    fn on_event(&self, event: BridgeEvent) {
        match event {
            BridgeEvent::Connected => self.on_connection(true, None),
            BridgeEvent::Disconnected { error } => self.on_connection(false, error),
            BridgeEvent::Status(snapshot) => self.on_status(*snapshot),
            // The core logs each delivery; the window shows no delivery log.
            BridgeEvent::Activity(_) => {}
            BridgeEvent::Invalidated(invalidation) => self.invalidated(invalidation.what),
            BridgeEvent::Notify(notification) => self.notify(&notification),
            BridgeEvent::SignIn(report) => self.sign_in_link(report.link),
        }
    }

    /// A snapshot newer than any before.
    pub fn on_status(&self, snapshot: Status) {
        let (landed, first) = self.update_state(|state| {
            let previous_account = state.prepare_account_update(&snapshot.account);
            let before = state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.account.state);
            state.snapshot = Some(snapshot);
            state.core_connected = true;
            let landed = state.sign_in_landed(before);
            let first = !std::mem::replace(&mut state.launch_recorded, true);
            let account = state.account();
            if previous_account.workspace_id != account.workspace_id
                || previous_account.person.as_ref().map(|person| &person.id)
                    != account.person.as_ref().map(|person| &person.id)
                || (account.state == SignInState::SignedOut
                    && before != Some(SignInState::SignedOut))
            {
                state.relaycast_upgrade = None;
                state.last_relaycast_upgrade_at = None;
                state.request_identity_revision = state.request_identity_revision.wrapping_add(1);
                state.request_identity_started_at_ms =
                    chrono::Utc::now().timestamp_millis().max(0) as u64;
            }
            match account.state {
                SignInState::Denied | SignInState::Expired | SignInState::Error => {
                    state.sign_in_error = account.error.clone();
                }
                SignInState::SignedOut => {}
                _ => state.sign_in_error = None,
            }
            if account.state != SignInState::PendingApproval && !state.connecting_workspace() {
                state.sign_in_link = None;
            }
            if state.connect_requested && state.has_workspace() {
                state.connect_requested = false;
            }
            if state.starting_sign_in
                && (Some(account.state) != state.sign_in_from || state.sign_in_answered)
            {
                // The snapshot shows where the attempt stands; its state
                // guards from here.
                state.starting_sign_in = false;
            }
            if state.signing_out && state.sign_out_answered && !state.account_present() {
                state.signing_out = false;
            }
            (landed, first)
        });
        if first {
            self.record(
                TelemetryOperation::AppLaunch,
                TelemetryOutcome::Started,
                None,
            );
            self.load_settings();
        }
        self.sync_tools_account();
        self.render();
        if self.with_state(|state| state.connected()) {
            if landed {
                self.go(Page::Sessions);
            }
            self.render_chrome();
            if matches!(self.page(), Page::Settings(_)) && !self.inner.updating.get() {
                self.render_settings();
            }
        }
        self.replay_links();
    }

    fn invalidated(&self, what: agent_relay_node::relay::state::Listing) {
        use agent_relay_node::relay::state::Listing;
        if !self.with_state(|state| state.connected()) {
            return;
        }
        match what {
            Listing::Uploader => {
                let sessions = self.page() == Page::Sessions;
                self.refresh_uploader(sessions);
            }
            Listing::Sessions => self.refresh_local(),
            Listing::Agents => {
                self.refresh_agents();
                self.refresh_local();
            }
            Listing::Roster => self.refresh_roster(),
            // The workspace's people are not shown.
            Listing::People => {}
            // An agent changed a setting (`PATCH /settings`).
            Listing::Settings => self.load_settings(),
            Listing::Activity => self.refresh_claude_delivery(),
            Listing::Messages => self.refresh_chats(),
            // Skip chat, projects and CLI fleet brokers are not read by this
            // window.
            Listing::Chat | Listing::Projects | Listing::Fleet => {}
        }
        // A session or reply changing can change whether a followed
        // conversation's recipient runs and what became of its message.
        if matches!(
            what,
            Listing::Sessions | Listing::Roster | Listing::Messages
        ) {
            self.follow_recipients();
        }
    }

    pub fn notify(&self, notification: &CoreNotification) {
        self.inner.desktop.notify(notification);
    }

    /// A link from a notification: the app's own links are parsed by the
    /// core; web links open in the browser.
    pub fn open_url(&self, url: &str) {
        if crate::launch::is_app_link(url) {
            self.open_link(url);
        } else if web_link(url) {
            self.inner.desktop.open_uri(url);
        }
    }

    // Which screen.

    /// Shows the screen the state calls for.
    pub fn render(&self) {
        if self.render_tools_onboarding() {
            return;
        }
        let (view, key) = self.with_state(State::view_for);
        let (previous, previous_key) =
            self.with_state(|state| (state.view, state.view_key.clone()));
        // Any other screen, the disconnect screen included, replaces the
        // account's view: what it showed and the calls it made are dropped.
        // A sign-out that fails comes back to a freshly read view.
        if previous == Some(View::Connected) && view != View::Connected {
            self.forget_account_view();
        }
        if Some(view) != previous || key != previous_key {
            let entering = Some(view) != previous;
            self.update_state(|state| {
                state.view = Some(view);
                state.view_key = key.clone();
            });
            match (view, key) {
                (View::Loading, ViewKey::Loading { error }) => self.show_loading(error.as_deref()),
                (View::Loading, _) => self.show_loading(None),
                (View::SigningOut, _) => {
                    self.show_progress(
                        "Disconnecting this computer…",
                        "Uploads are stopping safely.",
                        false,
                    );
                }
                (View::SigningIn, _) => self.show_signing_in(),
                (View::ChoosingWorkspace, _) => self.show_workspace_picker(),
                (View::SignIn, _) if entering => {
                    self.show_sign_in();
                    self.refresh_preview();
                }
                (View::SignIn, _) => self.draw_sign_in(),
                (View::Connected, _) => {
                    if entering {
                        self.show_connected();
                        self.refresh_all();
                    }
                }
            }
        }
        if view == View::SigningIn {
            self.update_signing_in();
        }
        self.update_tray();
    }

    // Signed out: the Mac's split welcome — a dark brand panel and a quiet
    // light sign-in panel.

    fn welcome(&self, panel: &gtk::Box) -> gtk::Box {
        let root = hbox(0, &["welcome"]);
        let splash = vbox(0, &["signin-splash"]);
        super::zoom::size(&splash, 540, -1);
        splash.set_hexpand(true);
        splash.append(&widgets::login_ribbons());
        root.append(&splash);
        let surface = vbox(0, &["signin-panel"]);
        surface.set_hexpand(true);
        let form = vbox(0, &[]);
        form.set_valign(gtk::Align::Center);
        form.set_halign(gtk::Align::Center);
        form.set_vexpand(true);
        let brand = hbox(12, &[]);
        brand.append(&widgets::brand(42));
        brand.append(&widgets::wordmark(28));
        widgets::name(&brand, "Sign in to Agent Relay");
        form.append(&brand);
        if let Some(site) = self.site_brand() {
            super::zoom::margins(&site, 12, 0, 0, 0);
            form.append(&site);
        }
        super::zoom::margins(panel, 32, 0, 0, 0);
        super::zoom::size(panel, 360, -1);
        form.append(panel);
        widgets::pad(&form, 40, None, None);
        surface.append(&form);
        // A core for another site than production says so, and which: the
        // core's site once it reported one, else this build's own
        // (AppModel.developmentBadge on macOS decides the same way).
        let site = self.with_state(|state| state.account().site_url);
        if let Some(site) = development_badge(site.as_deref()) {
            let dev = text("Development", &["secondary"]).centered();
            dev.set_tooltip_text(Some(site));
            super::zoom::margins(&dev, 0, 0, 20, 0);
            surface.append(&dev);
        }
        root.append(&surface);
        root
    }

    fn show_progress(&self, heading: &str, detail: &str, cancel: bool) {
        let panel = vbox(24, &[]);
        panel.append(&widgets::loading_bars());
        let heading_label = text(heading, &["signin-title"]).wrapped().centered();
        let detail_label = text(detail, &["secondary", "signin-subtitle"])
            .wrapped()
            .centered();
        let code_label = text("", &["settings-row-title"]).centered().copyable();
        panel.append(&heading_label);
        panel.append(&detail_label);
        panel.append(&code_label);
        let open_browser = widgets::button(
            "Open browser",
            &["primary"],
            self.on(App::open_approval_again),
        );
        if cancel {
            let actions = hbox(10, &[]);
            actions.set_homogeneous(true);
            actions.append(&widgets::button(
                "Cancel",
                &[],
                self.on(App::cancel_sign_in),
            ));
            actions.append(&open_browser);
            panel.append(&actions);
        }
        self.replace(&self.welcome(&panel));
        *self.inner.signing.borrow_mut() =
            cancel.then_some((heading_label, detail_label, code_label, open_browser));
    }

    fn show_loading(&self, error: Option<&str>) {
        let panel = vbox(24, &[]);
        match error {
            Some(error) => {
                panel.append(&text("Let’s get you connected.", &["signin-title"]).wrapped());
                panel.append(&text(error, &["secondary", "signin-subtitle"]).wrapped());
                panel.append(&widgets::labeled_button(
                    "Try again",
                    "view-refresh-symbolic",
                    &["primary"],
                    self.on(App::retry_core),
                ));
                let copy = widgets::button(
                    "Copy diagnostics",
                    &["link"],
                    self.on(|app| {
                        widgets::copy_to_clipboard(&diagnostics_report(&app.diagnostics()))
                    }),
                );
                copy.set_halign(gtk::Align::Start);
                panel.append(&copy);
            }
            None => {
                let heading = hbox(12, &[]);
                let reconnecting = self.with_state(|state| state.launch_recorded);
                heading.append(&text(
                    if reconnecting {
                        "Reconnecting"
                    } else {
                        STARTUP_TITLE
                    },
                    &["signin-title"],
                ));
                let bars = widgets::loading_bars_scaled(0.5);
                bars.set_hexpand(true);
                bars.set_halign(gtk::Align::End);
                bars.set_valign(gtk::Align::Center);
                heading.append(&bars);
                panel.append(&heading);
                super::startup_detail::add(&panel, reconnecting);
                let weak_panel = panel.downgrade();
                glib::timeout_add_local_once(
                    self.with_state(|state| state.startup_timeout),
                    self.on(move |app| {
                        // A replaced panel is a different attempt. Its timer must
                        // not overwrite a later retry, error, or ready screen.
                        if weak_panel
                            .upgrade()
                            .is_some_and(|panel| panel.root().is_some())
                            && app.with_state(|state| {
                                state.snapshot.is_none() && state.core_error.is_none()
                            })
                        {
                            app.update_state(|state| {
                                state.core_error = Some(STARTUP_TIMEOUT_MESSAGE.into())
                            });
                            app.render();
                        }
                    }),
                );
            }
        }
        self.replace(&self.welcome(&panel));
    }

    fn retry_core(&self) {
        self.update_state(|state| state.core_error = None);
        self.show_loading(None);
        let service = self.inner.service.clone();
        self.blocking(move || service.start(), |app, _| app.inner.bridge.restart());
    }

    fn refresh_preview(&self) {
        if !self
            .inner
            .bridge
            .offers(agent_relay_desktop_core::contract::method::UploaderSessionsPreview::NAME)
        {
            return;
        }
        let params = agent_relay_desktop_core::contract::account::PreviewParams {
            limit: None,
            refresh: false,
        };
        self.call::<agent_relay_desktop_core::contract::method::UploaderSessionsPreview>(params, |app, result| {
            let count = result.map(|preview| preview.sessions.len()).unwrap_or(0);
            app.update_state(|state| state.preview_count = count);
        });
    }

    /// The sign-in screen drawn from what is known, asking nothing.
    pub(super) fn draw_sign_in(&self) {
        let (account, error, starting) = self.with_state(|state| {
            let account = state.account();
            let error = state.sign_in_error.clone().or(account.error.clone());
            (account, error, state.starting_sign_in)
        });
        let panel = vbox(28, &[]);
        panel.append(&self.sign_in_choices(starting));
        let works = vbox(0, &["signin-works-with"]);
        super::zoom::margins(&works, 12, 0, 0, 0);
        let divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        divider.add_css_class("signin-divider");
        works.append(&divider);
        let caption = text("Works with all your agents", &["secondary", "signin-works-caption"]);
        super::zoom::margins(&caption, 24, 0, 14, 0);
        works.append(&caption);
        let agents = widgets::login_agent_strip();
        agents.set_opacity(0.72);
        works.append(&agents);
        panel.append(&works);
        self.append_account_action(&panel, account.action, starting);
        if let Some(error) = error {
            panel.append(&text(&error, &["error"]).wrapped());
        }
        self.replace(&self.welcome(&panel));
    }

    pub fn sign_in_with_provider(&self, provider: SignInProvider) {
        self.inner.sign_in_provider.set(provider);
        self.start_sign_in(false, None, Some(provider));
    }

    /// Opens the browser account chooser, sharing only the sessions chosen in
    /// My sessions, as the Mac does.
    pub fn sign_in(&self, acknowledge_uninspected_schedules: bool, workspace: Option<String>) {
        self.start_sign_in(acknowledge_uninspected_schedules, workspace, None);
    }

    /// A `sign_in` event (or `account.sign_in`'s answer): the link waiting
    /// for approval, opened once.
    pub(super) fn sign_in_link(&self, link: SignInLink) {
        let url = link
            .verification_url_complete
            .clone()
            .or(link.verification_url.clone())
            .filter(|url| web_link(url));
        let open = self.update_state(|state| {
            if link.verification_url.is_some() {
                state.sign_in_link = Some(link);
            }
            url.filter(|url| state.opened_links.insert(url.clone()))
        });
        if let Some(url) = open {
            self.open_approval_url(&url);
        }
        self.update_signing_in();
    }

    pub fn open_approval_again(&self) {
        let url = self.with_state(|state| {
            state.sign_in_link.as_ref().and_then(|link| {
                link.verification_url_complete
                    .clone()
                    .or(link.verification_url.clone())
            })
        });
        if let Some(url) = url.filter(|url| web_link(url)) {
            self.open_approval_url(&url);
        }
    }

    fn show_workspace_picker(&self) {
        let account = self.with_state(|state| state.account());
        let panel = vbox(28, &[]);
        panel.append(&text("Choose your workspace", &["signin-title"]).wrapped());
        if let Some(email) = account.person.and_then(|person| person.email) {
            panel.append(&text(&format!("Signed in as {email}"), &["secondary"]).wrapped());
        }
        for workspace in account.workspace_choices.unwrap_or_default() {
            let label = match workspace.organization_name {
                Some(org) => format!("{} — {org}", workspace.name),
                None => workspace.name,
            };
            panel.append(&widgets::button(
                &label,
                &["workspace-choice"],
                self.on(move |app| {
                    app.call::<agent_relay_desktop_core::contract::method::AccountSelectWorkspace>(
                        agent_relay_desktop_core::contract::account::ConnectWorkspaceParams {
                            workspace_id: workspace.id.clone(),
                        },
                        |app, result| {
                            if let Err(error) = result {
                                app.show_error(&error.message);
                            }
                        },
                    );
                }),
            ));
        }
        panel.append(&widgets::button(
            "Cancel",
            &["flat"],
            self.on(App::cancel_sign_in),
        ));
        self.replace(&self.welcome(&panel));
    }

    fn show_signing_in(&self) {
        self.show_progress(
            "Continue in your browser",
            "Your browser will open to Agent Relay.",
            true,
        );
    }

    /// The sign-in progress screen's heading, detail and code.
    pub fn signing_labels(&self) -> Option<(String, String, String)> {
        self.inner
            .signing
            .borrow()
            .as_ref()
            .map(|(heading, detail, code, _)| {
                (
                    heading.text().into(),
                    detail.text().into(),
                    code.text().into(),
                )
            })
    }

    fn update_signing_in(&self) {
        if self.view() != Some(View::SigningIn) {
            return;
        }
        let Some((heading, detail, code, open_browser)) = self.inner.signing.borrow().clone()
        else {
            return;
        };
        let (account, link) = self.with_state(|state| {
            (
                state.account(),
                state.sign_in_link.clone().unwrap_or_default(),
            )
        });
        let waiting = account.state == SignInState::PendingApproval;
        heading.set_text(if waiting {
            "Continue in your browser"
        } else {
            sign_in_heading(&account)
        });
        detail.set_text(&sign_in_detail(&account, &link));
        code.set_text(
            &link
                .user_code
                .as_deref()
                .map(|code| format!("Code: {code}"))
                .unwrap_or_default(),
        );
        open_browser.set_visible(waiting);
        open_browser.set_sensitive(link.verification_url.is_some());
    }

    pub fn cancel_sign_in(&self) {
        self.update_state(|state| {
            // An `account.sign_in` answer still on its way belongs to the
            // cancelled attempt.
            state.sign_in_requests += 1;
            state.starting_sign_in = false;
            state.pending_connect = None;
            state.sign_in_error = None;
            state.sign_in_link = None;
        });
        self.call::<agent_relay_desktop_core::contract::method::AccountSignInCancel>(NoParams {}, |_, _| {});
    }

    // Signed in.

    fn show_connected(&self) {
        let shell = Shell::build(self);
        self.replace(&shell.root);
        *self.inner.shell.borrow_mut() = Some(shell);
        self.render_chrome();
        self.render_page();
    }

    /// Draws the page shown.
    pub fn render_page(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        if !self.with_state(|state| state.connected()) {
            return;
        }
        let page = self.page();
        shell.page_stack.set_visible_child_name(page.stack_name());
        let error = self.with_state(|state| state.page_error.clone());
        shell.page_error.set_tooltip_text(error.as_deref());
        shell.page_error.set_visible(error.is_some());
        match page {
            Page::Sessions => self.render_sessions(),
            Page::Messages => self.render_messages(),
            Page::Settings(_) => self.render_settings(),
        }
    }

    pub fn refresh_page(&self) {
        match self.page() {
            Page::Sessions => {
                self.refresh_uploader(true);
                self.refresh_local();
            }
            Page::Messages => {
                if let Some(shown) = self.shown_chat() {
                    self.refresh_chat(&shown);
                }
            }
            Page::Settings(Pane::Tools) => self.refresh_provider_preferences(),
            Page::Settings(Pane::Relay) => {
                self.load_settings();
                self.refresh_claude_delivery();
                self.refresh_local();
            }
            Page::Settings(Pane::General) => {
                self.load_settings();
                self.refresh_service();
            }
            Page::Settings(_) => self.load_settings(),
        }
    }

    fn poll(&self) {
        let ticks = self.update_state(|state| {
            state.poll_ticks = super::uploader_safety_net::next_tick(state.poll_ticks);
            state.poll_ticks
        });
        if !self.with_state(|state| state.connected()) {
            return;
        }
        let visible = self.inner.window.is_visible();
        let page = self.page();
        self.uploader_safety_net(ticks, visible);
        // Diagnostics shows the request log: read it every 5 s there, and
        // every 30 s otherwise so a failed request or a plan limit still
        // shows on any page.
        if (visible && page == Page::Settings(Pane::Diagnostics)) || ticks % 6 == 0 {
            self.refresh_request_log();
        }
        // The roster has no push: it is read every 30 s while a page shows
        // it.
        if ticks % 6 == 0 && (!visible || !matches!(page, Page::Settings(_))) {
            self.refresh_roster();
        }
        if visible && ticks % 6 == 0 {
            self.refresh_settings_delivery(page);
        }
    }

    // Account.

    pub fn confirm_workspace_sign_out(&self) {
        if self.with_state(|state| state.busy || state.signing_out) {
            return;
        }
        let name = self.with_state(|state| state.workspace_name());
        let answer = self.confirm(Confirmation::new(
            "Sign out to switch workspace?",
            &format!("Uploads stop and stay tied to {name}. Sign back in to that workspace to resume without uploading existing history again."),
            "Sign out",
        ).destructive());
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            if answer.await {
                if let Some(app) = weak.upgrade() {
                    app.sign_out();
                }
            }
        });
    }

    pub fn confirm_logout(&self) {
        if self.with_state(|state| state.busy || state.signing_out) {
            return;
        }
        let answer = self.confirm(
            Confirmation::new(
                "Log out of Agent Relay?",
                "Uploads stop and you will need to sign in again to reconnect.",
                "Log out",
            )
            .destructive(),
        );
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            if answer.await {
                if let Some(app) = weak.upgrade() {
                    app.sign_out();
                }
            }
        });
    }

    fn sign_out(&self) {
        self.update_state(|state| {
            state.signing_out = true;
            state.sign_out_answered = false;
        });
        self.render();
        self.call::<agent_relay_desktop_core::contract::method::AccountSignOut>(NoParams {}, |app, result| {
            app.update_state(|state| {
                state.sign_out_answered = true;
                state.sign_in_error = result.as_ref().err().map(|error| {
                    format!(
                        "Could not finish disconnecting. Uploads may still be running. Sign in again to retry cleanup. {}",
                        error.message
                    )
                });
                // Signed out, the disconnect screen stays until the snapshot
                // no longer shows the account, which arrives on the event
                // stream and may trail this answer.
                if result.is_err() || !state.account_present() {
                    state.signing_out = false;
                }
            });
            app.render();
        });
    }

    // Dialogs.

    /// Says what went wrong: above the page in the signed-in window,
    /// else in a dialog over a shown window.
    pub fn show_error(&self, message: &str) {
        let visible = self.inner.window.is_visible();
        if visible && self.with_state(|state| state.connected()) && self.shell().is_some() {
            self.update_state(|state| state.page_error = Some(message.to_string()));
            if let Some(shell) = self.shell() {
                shell.page_error.set_tooltip_text(Some(message));
                shell.page_error.set_visible(true);
            }
        } else if visible {
            self.inner
                .desktop
                .inform(&self.inner.window, "Agent Relay hit a problem", message);
        }
    }
}
