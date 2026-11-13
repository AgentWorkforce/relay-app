//! What a self-host site's login configuration adds to the sign-in screen:
//! its logo and organisation name under the Agent Relay lockup, and its own
//! sign-in page in the browser in place of Google and Microsoft, naming the
//! methods it allows. A password, when the site allows one, is typed on that
//! page in the browser, never here. Without a configuration the screen is
//! as it always was. The Mac's `SignInView` matches these.

use super::app::App;
use super::google_glyph::{google_glyph, microsoft_glyph};
use super::state::{State, View};
use super::widgets::{self, hbox, text, vbox, LabelStyle, Sized};
use agent_relay_desktop_core::contract::account::{
    Flow, LoginConfig, SharingMode, SignInProvider, SignInRequest,
};
use agent_relay_desktop_core::contract::method::{AccountLoginConfig, AccountSignIn};
use agent_relay_desktop_core::history::cloud::history_agent;
use agent_relay_node::relay::contract::NoParams;
use agent_relay_node::relay::state::SignInState;
use gtk::{gdk, glib, prelude::*};
use gtk4 as gtk;
use std::cell::RefCell;
use std::time::Duration;

/// The logo is 22 points tall and at most 160 wide.
const LOGO_HEIGHT: i32 = 22;
const LOGO_MAX_WIDTH: i32 = 160;
/// A logo is at most 1 MiB and loads within 10 seconds, without redirects.
const LOGO_LIMIT: u64 = 1024 * 1024;
const LOGO_TIMEOUT: Duration = Duration::from_secs(10);

type WaitingLogo = glib::WeakRef<gtk::Box>;
thread_local! {
    /// The logo by its URL once it loaded. One that could not load is not
    /// kept, so the next sign-in screen tries again.
    static LOGO: RefCell<Option<(String, gdk::Texture)>> = const { RefCell::new(None) };
    static WAITING: RefCell<Vec<WaitingLogo>> = const { RefCell::new(Vec::new()) };
}

impl App {
    /// The sign-in screen, asking for the site's login configuration when
    /// it has none yet.
    pub fn show_sign_in(&self) {
        self.load_login_config();
        self.draw_sign_in();
    }

    /// Asks the core once for the site's login configuration; the sign-in
    /// screen is drawn again when one arrives. A failed call asks again the
    /// next time the screen shows, never on a redraw of the screen already
    /// showing, so how often it is redrawn does not change how often the
    /// site is asked.
    pub(super) fn load_login_config(&self) {
        if self.with_state(|state| state.login_config.is_some())
            || !self.bridge().offers(AccountLoginConfig::NAME)
        {
            return;
        }
        self.update_state(|state| state.login_config = Some(LoginConfig::default()));
        self.call::<AccountLoginConfig>(NoParams {}, |app, result| match result {
            Err(_) => app.update_state(|state| state.login_config = None),
            Ok(config) if config.available => {
                app.update_state(|state| state.login_config = Some(config));
                if app.view() == Some(View::SignIn) {
                    app.draw_sign_in();
                }
            }
            Ok(_) => {}
        });
    }

    /// The site's login configuration, when it published one.
    pub(super) fn site_login_config(&self) -> Option<LoginConfig> {
        self.with_state(|state| state.login_config.clone())
            .filter(|config| config.available)
    }

    /// The site's logo and organisation name, for under the lockup.
    pub(super) fn site_brand(&self) -> Option<gtk::Box> {
        let config = self.site_login_config()?;
        if config.logo_url.is_none() && config.org_name.is_none() {
            return None;
        }
        let row = hbox(8, &["signin-site-brand"]);
        if let Some(url) = config.logo_url {
            row.append(&logo(&url));
        }
        if let Some(org) = config.org_name {
            row.append(&text(&org, &["secondary", "signin-site-name"]).ellipsized());
            widgets::name(&row, &org);
        }
        Some(row)
    }

    /// The sign-in buttons: the site's own page in the browser when its
    /// login configuration names its methods, else Google and Microsoft.
    pub(super) fn sign_in_choices(&self, starting: bool) -> gtk::Box {
        let enabled = self.bridge().offers(AccountSignIn::NAME) && !starting;
        let choices = vbox(12, &[]);
        if let Some(config) = self.site_login_config() {
            // Microsoft, when the site has it configured, opens the site's
            // own Microsoft sign-in, above the site's page.
            if let Some(microsoft) = config.microsoft_name.as_deref() {
                let button = sign_in_button(Some(SignInProvider::Microsoft), Some(microsoft));
                button.set_sensitive(enabled);
                let sign_in = self.on(|app| app.sign_in_with_provider(SignInProvider::Microsoft));
                button.connect_clicked(move |_| sign_in());
                choices.append(&button);
            }
            let button = sign_in_button(None, None);
            button.set_sensitive(enabled);
            let sign_in = self.on(|app| app.sign_in(false, None));
            button.connect_clicked(move |_| sign_in());
            choices.append(&button);
            if let Some(methods) = config.methods_line {
                choices.append(&text(&methods, &["secondary", "signin-methods"]).wrapped());
            }
            return choices;
        }
        for provider in [SignInProvider::Google, SignInProvider::Microsoft] {
            let button = sign_in_button(Some(provider), None);
            button.set_sensitive(enabled);
            let sign_in = self.on(move |app| app.sign_in_with_provider(provider));
            button.connect_clicked(move |_| sign_in());
            choices.append(&button);
        }
        choices
    }

    /// The provider a sign-in names: on a self-host site only its own
    /// Microsoft sign-in when that button was clicked, else the site's own
    /// page; elsewhere the provider clicked, else the `last` one.
    pub(super) fn sign_in_provider_for(
        &self,
        chosen: Option<SignInProvider>,
        last: SignInProvider,
    ) -> Option<SignInProvider> {
        match self.site_login_config() {
            Some(config) => site_provider(&config, chosen),
            None => Some(chosen.unwrap_or(last)),
        }
    }

    /// A sign-in naming `chosen` when a provider's button was clicked; on a
    /// self-host site any other sign-in opens the site's own page.
    pub(super) fn start_sign_in(
        &self,
        acknowledge_uninspected_schedules: bool,
        workspace: Option<String>,
        chosen: Option<SignInProvider>,
    ) {
        let account_state = self.with_state(|state| state.account_state());
        if self.with_state(|state| state.starting_sign_in)
            || State::signing_in(account_state)
            || account_state == SignInState::SignedIn
        {
            return;
        }
        let pending = self.with_state(|state| state.pending_connect.clone());
        let params = SignInRequest {
            workspace: pending
                .as_ref()
                .map(|pending| pending.workspace_id.clone())
                .or(workspace),
            sharing: SharingMode::Selected,
            acknowledge_uninspected_schedules,
            flow: Some(Flow::Browser),
            provider: self.sign_in_provider_for(chosen, self.inner.sign_in_provider.get()),
        };
        let request = self.update_state(|state| {
            state.sign_in_error = None;
            state.sign_in_link = None;
            state.sign_in_requests += 1;
            state.starting_sign_in = true;
            state.sign_in_answered = false;
            state.sign_in_from = Some(account_state);
            state.sign_in_requests
        });
        self.render();
        self.call::<AccountSignIn>(params, move |app, result| {
            if app.with_state(|state| state.sign_in_requests) != request {
                return;
            }
            match result {
                Err(error) => {
                    app.update_state(|state| {
                        state.starting_sign_in = false;
                        state.sign_in_error = Some(error.message);
                    });
                    app.render();
                }
                Ok(report) => {
                    let started = State::signing_in(report.account.state)
                        || report.account.state == SignInState::SignedIn;
                    app.update_state(|state| {
                        state.pending_connect = None;
                        state.sign_in_answered = true;
                        if !started {
                            // The core started no attempt: nothing is left
                            // to wait for.
                            state.starting_sign_in = false;
                        }
                    });
                    if !started {
                        app.render();
                    }
                    app.sign_in_link(report.link);
                }
            }
        });
    }
}

/// The provider a sign-in on a self-host site names: Microsoft when it was
/// chosen and the site has it configured, else none (the site's own page).
pub(super) fn site_provider(
    config: &LoginConfig,
    chosen: Option<SignInProvider>,
) -> Option<SignInProvider> {
    chosen.filter(|provider| {
        *provider == SignInProvider::Microsoft && config.microsoft_name.is_some()
    })
}

/// The label a sign-in button shows: a provider's, or the site's page. A
/// site names its own Microsoft sign-in, as `name`.
pub(super) fn sign_in_label(provider: Option<SignInProvider>, name: Option<&str>) -> String {
    if let (Some(_), Some(name)) = (provider, name) {
        return format!("Continue with {name}");
    }
    match provider {
        Some(SignInProvider::Google) => "Continue with Google",
        Some(SignInProvider::Microsoft) => "Continue with Microsoft",
        None => "Continue in browser",
    }
    .into()
}

fn sign_in_button(provider: Option<SignInProvider>, name: Option<&str>) -> gtk::Button {
    let label = sign_in_label(provider, name);
    let label = label.as_str();
    let button = gtk::Button::new();
    widgets::action_cursor(&button);
    button.add_css_class("primary");
    button.add_css_class("provider-signin");
    super::zoom::size(&button, -1, 50);
    let content = hbox(12, &[]);
    content.set_halign(gtk::Align::Center);
    match provider {
        Some(SignInProvider::Google) => content.append(&google_glyph(18)),
        Some(SignInProvider::Microsoft) => content.append(&microsoft_glyph(18)),
        None => {}
    }
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    widgets::name(&button, label);
    button
}

/// The logo, loaded off the main thread and kept once it loaded; nothing
/// while it loads or when it cannot, and then the next screen tries again.
fn logo(url: &str) -> gtk::Box {
    let holder = hbox(0, &[]);
    holder.set_valign(gtk::Align::Center);
    let cached = LOGO.with(|logo| logo.borrow().clone());
    if let Some((_, texture)) = cached.filter(|(loaded, _)| loaded == url) {
        show_logo(&holder, &texture);
        return holder;
    }
    let first = WAITING.with(|waiting| {
        let mut waiting = waiting.borrow_mut();
        waiting.retain(|row| row.upgrade().is_some());
        waiting.push(holder.downgrade());
        waiting.len() == 1
    });
    if first {
        let url = url.to_owned();
        glib::spawn_future_local(async move {
            let fetch_url = url.clone();
            let bytes = gtk::gio::spawn_blocking(move || fetch_logo(&fetch_url))
                .await
                .ok()
                .flatten();
            let texture = bytes
                .and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok());
            let rows = WAITING.with(|waiting| std::mem::take(&mut *waiting.borrow_mut()));
            if let Some(texture) = texture {
                LOGO.with(|logo| *logo.borrow_mut() = Some((url, texture.clone())));
                for row in rows.iter().filter_map(|row| row.upgrade()) {
                    show_logo(&row, &texture);
                }
            }
        });
    }
    holder
}

fn show_logo(holder: &gtk::Box, texture: &gdk::Texture) {
    let (width, height) = logo_size(texture.width(), texture.height());
    let picture = gtk::Picture::for_paintable(texture);
    picture.set_can_shrink(true);
    picture.set_keep_aspect_ratio(true);
    holder.append(&Sized::new(width, height, &picture));
}

/// The logo's size: 22 points tall, as wide as its shape allows up to 160.
fn logo_size(width: i32, height: i32) -> (i32, i32) {
    let height_px = super::zoom::px(LOGO_HEIGHT);
    if width <= 0 || height <= 0 {
        return (height_px, height_px);
    }
    let width_px = (i64::from(width) * i64::from(height_px) / i64::from(height)) as i32;
    (
        width_px.clamp(1, super::zoom::px(LOGO_MAX_WIDTH)),
        height_px,
    )
}

/// The logo's bytes: an `https` answer without redirects that fits the limit,
/// through the agent the core's other site calls use: the platform's trust
/// roots, and a resolver that refuses a name with a non-public address.
fn fetch_logo(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("https://") {
        return None;
    }
    download(&logo_agent(history_agent()), url)
}

/// No redirects, and one deadline for the whole load, body included.
fn logo_agent(builder: ureq::AgentBuilder) -> ureq::Agent {
    builder.redirects(0).timeout(LOGO_TIMEOUT).build()
}

/// One GET that does not follow a redirect: only a 200 answer of at most
/// [`LOGO_LIMIT`] bytes is kept.
fn download(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let response = agent
        .get(url)
        .set("User-Agent", "AgentRelayDesktop")
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    let mut data = Vec::new();
    response
        .into_reader()
        .take(LOGO_LIMIT + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= LOGO_LIMIT).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_buttons_name_the_provider_or_the_site_page() {
        assert_eq!(
            sign_in_label(Some(SignInProvider::Google), None),
            "Continue with Google"
        );
        assert_eq!(
            sign_in_label(Some(SignInProvider::Microsoft), None),
            "Continue with Microsoft"
        );
        assert_eq!(sign_in_label(None, None), "Continue in browser");
    }

    #[test]
    fn a_site_names_its_own_microsoft_sign_in() {
        assert_eq!(
            sign_in_label(Some(SignInProvider::Microsoft), Some("Contoso Microsoft")),
            "Continue with Contoso Microsoft"
        );
        assert_eq!(sign_in_label(None, Some("Contoso")), "Continue in browser");
    }

    #[test]
    fn a_site_sign_in_names_only_its_configured_microsoft() {
        let with = LoginConfig {
            available: true,
            microsoft_name: Some("Microsoft".into()),
            ..LoginConfig::default()
        };
        let without = LoginConfig {
            available: true,
            ..LoginConfig::default()
        };
        let microsoft = Some(SignInProvider::Microsoft);
        assert_eq!(site_provider(&with, microsoft), microsoft);
        assert_eq!(site_provider(&with, None), None);
        assert_eq!(site_provider(&with, Some(SignInProvider::Google)), None);
        assert_eq!(site_provider(&without, microsoft), None);
    }

    #[test]
    fn a_logo_is_22_tall_and_keeps_its_shape_up_to_160_wide() {
        let tall = super::super::zoom::px(LOGO_HEIGHT);
        assert_eq!(logo_size(100, 50).1, tall);
        assert_eq!(logo_size(100, 50).0, tall * 2);
        assert_eq!(
            logo_size(10_000, 10).0,
            super::super::zoom::px(LOGO_MAX_WIDTH)
        );
        assert_eq!(logo_size(0, 0), (tall, tall));
    }

    #[test]
    fn a_logo_is_fetched_only_over_https() {
        assert_eq!(fetch_logo("http://example.com/logo.png"), None);
        assert_eq!(fetch_logo("file:///etc/hosts"), None);
    }

    /// Answers one request on loopback with `head` and `body` and hands
    /// back the request line it read.
    fn one_answer(head: &'static str, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/logo.png", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut header = String::new();
            while reader.read_line(&mut header).unwrap() > 2 {
                header.clear();
            }
            let head = format!(
                "{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            let _ = stream.write_all(&body);
            line
        });
        (url, server)
    }

    /// The logo agent's own rules, without the address rule that refuses
    /// the loopback test server.
    fn plain() -> ureq::Agent {
        logo_agent(ureq::AgentBuilder::new())
    }

    #[test]
    fn a_logo_redirect_is_refused_not_followed() {
        let (url, server) = one_answer(
            "HTTP/1.1 302 Found\r\nLocation: https://elsewhere.example/logo.png",
            Vec::new(),
        );
        assert_eq!(download(&plain(), &url), None);
        assert!(server.join().unwrap().starts_with("GET /logo.png "));
    }

    #[test]
    fn a_logo_is_at_most_1_mib() {
        let limit = LOGO_LIMIT as usize;
        let (url, server) = one_answer("HTTP/1.1 200 OK", vec![7; limit]);
        assert_eq!(
            download(&plain(), &url).map(|bytes| bytes.len()),
            Some(limit)
        );
        server.join().unwrap();
        let (url, server) = one_answer("HTTP/1.1 200 OK", vec![7; limit + 1]);
        assert_eq!(download(&plain(), &url), None);
        server.join().unwrap();
        let (url, server) = one_answer("HTTP/1.1 404 Not Found", b"missing".to_vec());
        assert_eq!(download(&plain(), &url), None);
        server.join().unwrap();
    }
}
