//! The sign-in methods and branding a self-host site publishes for its
//! sign-in screen (`GET /cloud/api/v1/auth/login-config`, public; the site
//! also serves it at `/api/v1/auth/login-config`). Sign-in itself still goes
//! through the site's browser page or the device flow; this only tells the
//! screen what the site will offer there. The password, when the site allows
//! one, is typed on that page and never reaches the Desktop.

use crate::contract::account::LoginConfig;
use agent_relay_node::relay::cloud_http;
use agent_relay_node::relay::cloud_session::api_base;
use agent_relay_node::relay::workspace::validated_base_url;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The route under the site's `/cloud/api/v1`, as every other Cloud call.
const ROUTE: &str = "/auth/login-config";
/// The screen waits on this, so a site that does not answer is given up on
/// quickly and the screen shows the default methods this time.
const TIMEOUT: Duration = Duration::from_secs(4);
const MAX_URL: usize = 2048;
const MAX_NAME: usize = 80;

/// Whether the core has already logged that the site has no login config,
/// and that asking for it failed.
static LOGGED_NONE: AtomicBool = AtomicBool::new(false);
static LOGGED_FAILED: AtomicBool = AtomicBool::new(false);

/// The site could not say this time: a network error, a timeout or an
/// answer other than a 2xx or a 404. The screen shows the default methods
/// and asks again the next time it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotNow;

impl std::fmt::Display for NotNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the site's sign-in methods could not be read")
    }
}

impl std::error::Error for NotNow {}

/// `site`'s login config. The unavailable one when the build is for the
/// production site (which publishes none) or the site has none: a 404, or
/// a body that is not a JSON object. [`NotNow`] when the site could not say.
pub async fn for_site(site: &str) -> Result<LoginConfig, NotNow> {
    if !crate::site_build::is_site_build_for(site) {
        return Ok(LoginConfig::default());
    }
    let Some(site) = validated_base_url(site) else {
        return Ok(unavailable("not a site address"));
    };
    let url = format!("{}{ROUTE}", api_base(&site));
    match cloud_http::send("GET", url, None, None, TIMEOUT).await {
        Ok(answer) if answer.ok() && answer.inner().is_object() => Ok(parse(answer.inner())),
        Ok(answer) if answer.ok() || answer.status == 404 => {
            Ok(unavailable(&format!("status {}", answer.status)))
        }
        Ok(answer) => Err(failed(&format!("status {}", answer.status))),
        Err(_) => Err(failed("unreachable")),
    }
}

fn unavailable(why: &str) -> LoginConfig {
    if !LOGGED_NONE.swap(true, Ordering::Relaxed) {
        eprintln!("debug: login config unavailable ({why}); sign-in shows the default methods");
    }
    LoginConfig::default()
}

fn failed(why: &str) -> NotNow {
    if !LOGGED_FAILED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "debug: login config failed ({why}); sign-in shows the default methods and asks again"
        );
    }
    NotNow
}

/// Reads a login config, keeping only what the screen can show safely: a
/// logo on an absolute https URL on a public host name with no credentials,
/// and short one-line names. Unknown methods are ignored.
pub fn parse(value: &Value) -> LoginConfig {
    let methods: Vec<&str> = value["methods"]
        .as_array()
        .map(|methods| methods.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // OIDC and Microsoft are not among the `methods` (the server lists only
    // `email_link` and `password`): the site names each on its own, and null
    // when it is not configured. They are read by presence, whatever the
    // `methods` say.
    let oidc_name = display_name(&value["oidc"]);
    let microsoft_name = display_name(&value["microsoft"]);
    let logo_url = value["logo_url"].as_str().and_then(logo);
    let org_name = value["org_name"].as_str().and_then(name);
    let email_link = methods.contains(&"email_link");
    let password = methods.contains(&"password");
    let methods_line = methods_line(
        microsoft_name.as_deref(),
        oidc_name.as_deref(),
        email_link,
        password,
    );
    LoginConfig {
        available: true,
        logo_url,
        org_name,
        oidc_name,
        microsoft_name,
        email_link,
        password,
        methods_line,
    }
}

/// A provider's display name: an object with `displayName` (or
/// `display_name`), or a bare string, through [`name`]; nothing otherwise.
fn display_name(provider: &Value) -> Option<String> {
    provider
        .as_str()
        .or_else(|| provider["displayName"].as_str())
        .or_else(|| provider["display_name"].as_str())
        .and_then(name)
}

/// An absolute https URL on a public host name with no credentials, or
/// nothing.
fn logo(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    (raw.len() <= MAX_URL
        && url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(host)) if public_host_name(host))
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

/// Whether `host` names a machine on the public internet: a dotted name
/// outside the local-only suffixes. IP literals never reach here; the
/// fetch's resolver still refuses a name that resolves to a non-public
/// address.
fn public_host_name(host: &str) -> bool {
    const LOCAL: [&str; 6] = [
        "localhost",
        "local",
        "internal",
        "lan",
        "home.arpa",
        "localdomain",
    ];
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.contains('.')
        && !LOCAL
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// A trimmed one-line name of at most [`MAX_NAME`] characters, or nothing.
fn name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty() && raw.chars().count() <= MAX_NAME && !raw.chars().any(char::is_control))
        .then(|| raw.to_string())
}

/// The line under the sign-in button naming what the site's page offers,
/// worded once here so every platform shows the same words.
fn methods_line(
    microsoft: Option<&str>,
    oidc: Option<&str>,
    email_link: bool,
    password: bool,
) -> Option<String> {
    let mut ways: Vec<String> = Vec::new();
    ways.extend(microsoft.map(str::to_string));
    ways.extend(oidc.map(str::to_string));
    if email_link {
        ways.push("an email link".into());
    }
    if password {
        ways.push("a password".into());
    }
    let last = ways.pop()?;
    let ways = if ways.is_empty() {
        last
    } else {
        format!("{} or {last}", ways.join(", "))
    };
    Some(format!("Sign in with {ways} in your browser."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_config_names_every_method_and_shows_the_logo() {
        let config = parse(&json!({
            "methods": ["oidc", "email_link", "password"],
            "oidc": {"displayName": "Example SSO"},
            "logo_url": "https://cdn.example.com/logo.png",
            "org_name": "  Example Org  ",
            "password_policy": {"min_length": 12},
        }));
        assert_eq!(
            config,
            LoginConfig {
                available: true,
                logo_url: Some("https://cdn.example.com/logo.png".into()),
                org_name: Some("Example Org".into()),
                oidc_name: Some("Example SSO".into()),
                microsoft_name: None,
                email_link: true,
                password: true,
                methods_line: Some(
                    "Sign in with Example SSO, an email link or a password in your browser.".into()
                ),
            }
        );
    }

    #[test]
    fn a_password_only_site_names_just_the_password() {
        let config = parse(&json!({"methods": ["password"]}));
        assert!(config.available && config.password && !config.email_link);
        assert_eq!(
            config.methods_line.as_deref(),
            Some("Sign in with a password in your browser.")
        );
        assert_eq!(config.logo_url, None);
    }

    #[test]
    fn two_methods_are_joined_with_or() {
        let config = parse(&json!({"methods": ["email_link", "password"]}));
        assert_eq!(
            config.methods_line.as_deref(),
            Some("Sign in with an email link or a password in your browser.")
        );
    }

    #[test]
    fn only_an_https_logo_without_credentials_is_shown() {
        for refused in [
            "http://cdn.example.com/logo.png",
            "//cdn.example.com/logo.png",
            "/logo.png",
            "https://user:secret@cdn.example.com/logo.png",
            "https://user@cdn.example.com/logo.png",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
        ] {
            let config = parse(&json!({"logo_url": refused, "org_name": "Example Org"}));
            assert_eq!(config.logo_url, None, "{refused}");
            // The name is read on its own; each screen decides where it goes.
            assert_eq!(config.org_name.as_deref(), Some("Example Org"), "{refused}");
        }
        let long = format!("https://cdn.example.com/{}", "a".repeat(MAX_URL));
        assert_eq!(parse(&json!({ "logo_url": long })).logo_url, None);
    }

    #[test]
    fn a_logo_is_only_on_a_public_host_name() {
        for refused in [
            "https://127.0.0.1/logo.png",
            "https://10.0.0.8/logo.png",
            "https://172.16.4.2/logo.png",
            "https://192.168.1.10/logo.png",
            "https://169.254.169.254/logo.png",
            "https://8.8.8.8/logo.png",
            "https://[::1]/logo.png",
            "https://[fd00::1]/logo.png",
            "https://[2001:4860:4860::8888]/logo.png",
            "https://localhost/logo.png",
            "https://LOCALHOST./logo.png",
            "https://app.localhost/logo.png",
            "https://printer.local/logo.png",
            "https://cdn.internal/logo.png",
            "https://nas.home.arpa/logo.png",
            "https://intranet/logo.png",
        ] {
            assert_eq!(
                parse(&json!({ "logo_url": refused })).logo_url,
                None,
                "{refused}"
            );
        }
        assert_eq!(
            parse(&json!({"logo_url": "https://cdn.example.com/brand/logo.png"})).logo_url,
            Some("https://cdn.example.com/brand/logo.png".into())
        );
    }

    #[test]
    fn names_are_one_short_line() {
        let long = "x".repeat(MAX_NAME + 1);
        for refused in ["", "   ", "Example\nOrg", long.as_str()] {
            let config = parse(&json!({
                "methods": ["oidc"],
                "oidc": {"displayName": refused},
                "logo_url": "https://cdn.example.com/logo.png",
                "org_name": refused,
            }));
            assert_eq!(config.oidc_name, None, "{refused:?}");
            assert_eq!(config.org_name, None, "{refused:?}");
            assert_eq!(config.methods_line, None, "{refused:?}");
        }
    }

    #[test]
    fn an_oidc_name_is_shown_whenever_the_site_names_it() {
        let named = parse(&json!({"methods": ["password"], "oidc": {"displayName": "Example SSO"}}));
        assert_eq!(named.oidc_name.as_deref(), Some("Example SSO"));
        assert_eq!(
            named.methods_line.as_deref(),
            Some("Sign in with Example SSO or a password in your browser.")
        );
        let unnamed = parse(&json!({"methods": ["password"]}));
        assert_eq!(unnamed.oidc_name, None);
        let alone = parse(&json!({"methods": ["oidc"], "oidc": {"display_name": "Example SSO"}}));
        assert_eq!(
            alone.methods_line.as_deref(),
            Some("Sign in with Example SSO in your browser.")
        );
    }

    #[test]
    fn a_microsoft_sign_in_is_named_first() {
        let config = parse(&json!({
            "methods": ["oidc", "email_link", "password"],
            "oidc": {"displayName": "Example SSO"},
            "microsoft": {"displayName": "Microsoft"},
        }));
        assert_eq!(config.microsoft_name.as_deref(), Some("Microsoft"));
        assert_eq!(config.oidc_name.as_deref(), Some("Example SSO"));
        assert_eq!(
            config.methods_line.as_deref(),
            Some(
                "Sign in with Microsoft, Example SSO, an email link or a password in your browser."
            )
        );
    }

    #[test]
    fn a_microsoft_name_is_read_from_display_name_or_a_string() {
        for microsoft in [
            json!({"display_name": "Contoso Microsoft"}),
            json!("Contoso Microsoft"),
        ] {
            let config = parse(&json!({"methods": ["password"], "microsoft": microsoft}));
            assert_eq!(
                config.microsoft_name.as_deref(),
                Some("Contoso Microsoft"),
                "{microsoft}"
            );
        }
    }

    #[test]
    fn a_site_without_microsoft_names_none() {
        for config in [
            json!({"methods": ["password"], "microsoft": null}),
            json!({"methods": ["password"]}),
            json!({"methods": ["password"], "microsoft": 7}),
            json!({"methods": ["password"], "microsoft": {"name": "Microsoft"}}),
        ] {
            let parsed = parse(&config);
            assert_eq!(parsed.microsoft_name, None, "{config}");
            assert_eq!(
                parsed.methods_line.as_deref(),
                Some("Sign in with a password in your browser."),
                "{config}"
            );
        }
    }

    #[test]
    fn a_microsoft_name_is_one_short_line() {
        let long = "x".repeat(MAX_NAME + 1);
        for refused in ["", "   ", "Micro\u{7}soft", "Micro\nsoft", long.as_str()] {
            let config = parse(&json!({
                "methods": ["password"],
                "microsoft": {"displayName": refused},
            }));
            assert_eq!(config.microsoft_name, None, "{refused:?}");
            assert_eq!(
                config.methods_line.as_deref(),
                Some("Sign in with a password in your browser."),
                "{refused:?}"
            );
        }
    }

    /// Microsoft is not one of the `methods`, so it is read whatever they
    /// list, even none.
    #[test]
    fn a_microsoft_name_is_read_whatever_the_methods() {
        for methods in [json!(["password"]), json!([]), json!(["oidc"])] {
            let config = parse(&json!({
                "methods": methods,
                "microsoft": {"displayName": "Microsoft"},
            }));
            assert_eq!(
                config.microsoft_name.as_deref(),
                Some("Microsoft"),
                "{methods}"
            );
        }
        let alone = parse(&json!({"methods": [], "microsoft": {"displayName": "Microsoft"}}));
        assert_eq!(
            alone.methods_line.as_deref(),
            Some("Sign in with Microsoft in your browser.")
        );
    }

    #[test]
    fn unknown_methods_are_ignored() {
        let config = parse(&json!({"methods": ["passkey", 7, null]}));
        assert!(config.available);
        assert!(!config.email_link && !config.password);
        assert_eq!(config.methods_line, None);
    }

    #[tokio::test]
    async fn the_production_site_is_never_asked() {
        let production = agent_relay_node::relay::links::DEFAULT_SITE_URL;
        assert_eq!(for_site(production).await, Ok(LoginConfig::default()));
    }

    /// Answers one request per `(status, body)` on loopback and hands back
    /// each request line it read.
    fn answers(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use crate::history::cloud::tests::{read_http_request, write_http_response};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                lines.push(read_http_request(&mut stream).0);
                write_http_response(&mut stream, status, body);
            }
            lines
        });
        (site, server)
    }

    /// A site that fails to answer is not taken to have no login config:
    /// the call fails, and the next one asks the site again.
    #[tokio::test]
    async fn a_failed_login_config_call_asks_again() {
        let (site, server) = answers(vec![
            ("503 Service Unavailable", r#"{"error":"unavailable"}"#),
            ("500 Internal Server Error", ""),
            ("429 Too Many Requests", r#"{"error":"rate_limited"}"#),
            (
                "200 OK",
                r#"{"methods":["password"],"org_name":"Example Org"}"#,
            ),
        ]);
        for _ in 0..3 {
            assert_eq!(for_site(&site).await, Err(NotNow));
        }
        let config = for_site(&site).await.unwrap();
        assert!(config.available && config.password);
        assert_eq!(config.org_name.as_deref(), Some("Example Org"));
        let lines = server.join().unwrap();
        assert_eq!(lines.len(), 4);
        for line in lines {
            assert!(
                line.starts_with("GET /cloud/api/v1/auth/login-config "),
                "{line}"
            );
        }
    }

    #[tokio::test]
    async fn an_unreachable_site_fails_the_call() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert_eq!(for_site(&site).await, Err(NotNow));
    }

    /// Only a 404 or a body that is not an object means the site has none.
    #[tokio::test]
    async fn a_site_without_a_login_config_is_unavailable() {
        let (site, server) = answers(vec![
            ("404 Not Found", r#"{"error":"no route"}"#),
            ("200 OK", "[]"),
        ]);
        assert_eq!(for_site(&site).await, Ok(LoginConfig::default()));
        assert_eq!(for_site(&site).await, Ok(LoginConfig::default()));
        server.join().unwrap();
    }
}
