//! Browser sign-in for the desktop: a loopback receiver on `127.0.0.1`
//! with PKCE. The core builds the login URL, the UI opens it, and Cloud
//! redirects back to the receiver with a short-lived handoff token that
//! `crate::cloud_api::exchange_handoff` trades for a renewable session. A
//! callback naming another origin than the site being signed in to is
//! refused. Device sign-in (`DeviceLogin`) stays in the public core, where
//! `agent-relay cloud login` uses it; it is re-exported here with
//! `LoginError` so the account names both sign-ins from one place.

pub use agent_relay_node::relay::cloud_login::{DeviceLogin, LoginError};
use agent_relay_node::relay::store::CloudSession;
use agent_relay_node::relay::workspace::{validated_base_url, Secret};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn start_failed() -> LoginError {
    LoginError {
        state: "error",
        message: "Could not start local sign-in. Try again.".into(),
    }
}

/// Percent-encodes a query value the way the macOS app's `URLComponents`
/// does: URL punctuation stays, `&`, `=`, `+` and `#` are escaped. Cloud's
/// login routes are tested against exactly this form.
fn query_value(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-._~!$'()*,;:@/?".contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn query(items: &[(&str, &str)]) -> String {
    items
        .iter()
        .map(|(name, value)| format!("{name}={}", query_value(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// This computer's name as a desktop sign-in sends it for the approval page
/// to show: trimmed, 1 to 80 UTF-16 units, no control characters.
fn valid_desktop_host(host_name: &str) -> Option<String> {
    let host_name = host_name.trim();
    (!host_name.is_empty()
        && host_name.encode_utf16().count() <= 80
        && !host_name.chars().any(char::is_control))
    .then(|| host_name.to_string())
}

fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A browser sign-in: a loopback listener that only this attempt's state
/// and PKCE verifier can complete.
pub struct BrowserLogin {
    listener: TcpListener,
    port: u16,
    state: String,
    verifier: Secret,
    /// This computer's name, when the sign-in is the desktop's own: Cloud
    /// then asks the person to approve "Agent Relay Desktop on <host>".
    desktop_host: Option<String>,
}

impl BrowserLogin {
    /// Listens on a free loopback port.
    pub async fn bind() -> Result<BrowserLogin, LoginError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| start_failed())?;
        let port = listener.local_addr().map_err(|_| start_failed())?.port();
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| start_failed())?;
        Ok(BrowserLogin {
            listener,
            port,
            state: agent_relay_node::relay::store::random_uuid().to_ascii_uppercase(),
            verifier: Secret::new(base64_url(&bytes)),
            desktop_host: None,
        })
    }

    /// Proves to Cloud that whoever exchanges the returned token opened this
    /// attempt's login page.
    pub fn verifier(&self) -> &Secret {
        &self.verifier
    }

    /// The S256 challenge for the verifier.
    pub fn code_challenge(&self) -> String {
        base64_url(&Sha256::digest(self.verifier.expose().as_bytes()))
    }

    /// Makes this the desktop's own sign-in, naming this computer. A name
    /// Cloud would refuse leaves it an ordinary sign-in.
    pub fn with_desktop_host(mut self, host_name: &str) -> BrowserLogin {
        self.desktop_host = valid_desktop_host(host_name);
        self
    }

    /// Where Cloud returns. The state rides in the path: Cloud's
    /// post-Google return can lose every query parameter of `next` after
    /// the first, and a path survives.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback/{}", self.port, self.state)
    }

    /// The page that starts this attempt. By default it goes through Google
    /// so the person picks the account; `reuse_browser_session` opens
    /// Cloud's CLI login directly, which returns at once in a browser
    /// already signed in.
    pub fn authorization_url(&self, site_url: &str, reuse_browser_session: bool) -> Option<String> {
        self.authorization_url_for_workspace(site_url, reuse_browser_session, None)
    }

    pub fn authorization_url_for_workspace(
        &self,
        site_url: &str,
        reuse_browser_session: bool,
        workspace: Option<&str>,
    ) -> Option<String> {
        let site = validated_base_url(site_url)?;
        let redirect = self.redirect_uri();
        let challenge = self.code_challenge();
        let mut login = query(&[
            ("redirect_uri", &redirect),
            ("state", &self.state),
            ("client", "desktop"),
            ("workspace_selection", "desktop"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        if let Some(host_name) = &self.desktop_host {
            login.push_str(&format!("&desktop_host_name={}", query_value(host_name)));
        }
        if let Some(workspace) = workspace {
            login.push_str(&format!("&workspace={}", query_value(workspace)));
        }
        Some(if reuse_browser_session {
            format!("{site}/cloud/api/v1/cli/login?{login}")
        } else {
            let next = format!("/api/v1/cli/login?{login}");
            format!(
                "{site}/cloud/api/auth/google/start?next={}",
                query_value(&next)
            )
        })
    }

    /// Waits for this attempt's return and answers the browser. Anything
    /// else that reaches the port is turned away. Dropping the future
    /// cancels it; the caller bounds how long it waits.
    pub async fn receive(&self, site_url: &str) -> CloudSession {
        loop {
            let Ok((stream, _)) = self.listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            if let Some(session) = self.answer(stream, site_url).await {
                return session;
            }
        }
    }

    async fn answer(&self, mut stream: TcpStream, site_url: &str) -> Option<CloudSession> {
        let mut request = Vec::new();
        let mut buffer = [0u8; 2048];
        let read = tokio::time::timeout(Duration::from_secs(2), async {
            while request.len() < 8192 {
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&buffer[..count]),
                }
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
        })
        .await;
        let session = read
            .ok()
            .and_then(|_| self.accept(&String::from_utf8_lossy(&request), site_url));
        respond(&mut stream, session.is_some()).await;
        session
    }

    /// The handoff session a callback request carries, when it is this
    /// attempt's and names the site being signed in to.
    fn accept(&self, request: &str, site_url: &str) -> Option<CloudSession> {
        let line = request.split("\r\n").next()?;
        let mut parts = line.split(' ');
        let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
            return None;
        };
        let url = url::Url::parse(&format!("http://127.0.0.1:{}{target}", self.port)).ok()?;
        let in_path = url.path() == format!("/callback/{}", self.state);
        if !in_path && url.path() != "/callback" {
            return None;
        }
        let mut values = HashMap::new();
        for (name, value) in url.query_pairs() {
            values.entry(name.to_string()).or_insert(value.to_string());
        }
        let state_matches = in_path || values.get("state") == Some(&self.state);
        let token = values
            .get("access_token")
            .filter(|token| !token.is_empty())?;
        let api = values.get("api_url")?.trim_matches('/');
        let site = validated_base_url(site_url)?;
        if !state_matches || api != format!("{site}/cloud") {
            return None;
        }
        Some(CloudSession {
            access_token: Secret::new(token.clone()),
            access_token_expires_at: values
                .get("access_token_expires_at")
                .filter(|value| !value.is_empty())
                .cloned(),
            refresh_token: None,
            refresh_token_expires_at: None,
            site_url: site,
        })
    }
}

/// Closes the tab once it has shown the result. Browsers only let a page close
/// a tab whose history it owns, so a tab that stays open says so instead.
const CLOSE_TAB: &str = "<script>history.replaceState(null,'','/done');setTimeout(function(){window.close();setTimeout(function(){document.getElementById('detail').textContent='You can close this tab.'},300)},1500)</script>";

fn page(title: &str, detail: &str, script: &str) -> String {
    include_str!("../resources/sign-in/page.html")
        .replace("{{title}}", title)
        .replace("{{detail}}", detail)
        .replace("{{script}}", script)
}

async fn respond(stream: &mut TcpStream, success: bool) {
    let (status, body) = if success {
        (
            "200 OK",
            page(
                "This computer is connected",
                "You're signed in to Agent Relay. This tab closes on its own.",
                CLOSE_TAB,
            ),
        )
    } else {
        (
            "400 Bad Request",
            page(
                "Sign-in failed",
                "This sign-in request was not recognized. Start again from the Agent Relay app.",
                "",
            ),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_bound_login_preserves_the_choice_and_pkce_through_google() {
        let login = super::BrowserLogin::bind().await.unwrap();
        let workspace = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for reuse in [false, true] {
            let url = url::Url::parse(
                &login
                    .authorization_url_for_workspace(
                        "https://agentrelay.com",
                        reuse,
                        Some(workspace),
                    )
                    .unwrap(),
            )
            .unwrap();
            let url = if reuse {
                url
            } else {
                let next = url
                    .query_pairs()
                    .find(|(key, _)| key == "next")
                    .unwrap()
                    .1
                    .into_owned();
                url::Url::parse(&format!("https://agentrelay.com{next}")).unwrap()
            };
            let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
            assert_eq!(pairs.get("workspace").unwrap(), workspace);
            assert_eq!(
                pairs.get("code_challenge").unwrap(),
                &login.code_challenge()
            );
        }
    }

    async fn response_to_callback(success: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        respond(&mut server, success).await;
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn the_callback_page_is_complete_and_closes_only_after_success() {
        for success in [true, false] {
            let response = response_to_callback(success).await;
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            assert!(head.contains(&format!("Content-Length: {}", body.len())));
            assert!(!body.contains("{{"));
            assert_eq!(body.contains("window.close()"), success);
            assert!(body.contains(if success {
                "<h1>This computer is connected</h1>"
            } else {
                "<h1>Sign-in failed</h1>"
            }));
        }
    }

    #[test]
    fn the_login_url_matches_the_macos_app_s_encoding() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let login = runtime.block_on(BrowserLogin::bind()).unwrap();
        let state = login.state.clone();
        let port = login.port;
        let challenge = login.code_challenge();
        let google = login
            .authorization_url("https://agentrelay.com", false)
            .unwrap();
        assert_eq!(
            google,
            format!(
                "https://agentrelay.com/cloud/api/auth/google/start?next=/api/v1/cli/login?redirect_uri%3Dhttp://127.0.0.1:{port}/callback/{state}%26state%3D{state}%26client%3Ddesktop%26workspace_selection%3Ddesktop%26code_challenge%3D{challenge}%26code_challenge_method%3DS256"
            )
        );
        let direct = login
            .authorization_url("https://agentrelay.com", true)
            .unwrap();
        assert_eq!(
            direct,
            format!(
                "https://agentrelay.com/cloud/api/v1/cli/login?redirect_uri=http://127.0.0.1:{port}/callback/{state}&state={state}&client=desktop&workspace_selection=desktop&code_challenge={challenge}&code_challenge_method=S256"
            )
        );
        assert!(login
            .authorization_url("http://evil.example", true)
            .is_none());
    }

    #[test]
    fn the_desktop_s_own_browser_sign_in_names_this_computer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let login = runtime
            .block_on(BrowserLogin::bind())
            .unwrap()
            .with_desktop_host(" test-mini ");
        let challenge = login.code_challenge();
        let direct = login
            .authorization_url_for_workspace("https://agentrelay.com", true, Some("w1"))
            .unwrap();
        assert!(
            direct.ends_with(&format!(
                "&code_challenge={challenge}&code_challenge_method=S256&desktop_host_name=test-mini&workspace=w1"
            )),
            "{direct}"
        );
        let google = login
            .authorization_url("https://agentrelay.com", false)
            .unwrap();
        // Cloud's google/start reads `next` as one query value and returns
        // to it whole: decoded, it must be the full sign-in with every
        // parameter, the computer's name and the challenge included.
        let google = url::Url::parse(&google).unwrap();
        let next: Vec<String> = google
            .query_pairs()
            .filter(|(key, _)| key == "next")
            .map(|(_, value)| value.into_owned())
            .collect();
        assert_eq!(google.query_pairs().count(), 1, "only next: {google}");
        let next = url::Url::parse(&format!("https://agentrelay.com/cloud{}", next[0])).unwrap();
        let inner: std::collections::HashMap<_, _> = next.query_pairs().into_owned().collect();
        assert_eq!(next.path(), "/cloud/api/v1/cli/login");
        assert_eq!(inner["desktop_host_name"], "test-mini");
        assert_eq!(inner["code_challenge"], challenge);
        assert_eq!(inner["code_challenge_method"], "S256");
        assert_eq!(inner["client"], "desktop");

        // A name Cloud would refuse, or an ordinary sign-in, names nothing.
        for login in [
            runtime
                .block_on(BrowserLogin::bind())
                .unwrap()
                .with_desktop_host("bad\u{7}name"),
            runtime.block_on(BrowserLogin::bind()).unwrap(),
        ] {
            let url = login
                .authorization_url("https://agentrelay.com", true)
                .unwrap();
            assert!(!url.contains("desktop_host_name"), "{url}");
        }
    }

    #[test]
    fn the_challenge_is_s256_of_a_43_character_verifier() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let login = runtime.block_on(BrowserLogin::bind()).unwrap();
        assert_eq!(login.verifier().expose().len(), 43);
        assert_eq!(
            login.code_challenge(),
            base64_url(&Sha256::digest(login.verifier().expose().as_bytes()))
        );
        // RFC 7636 appendix B.
        assert_eq!(
            base64_url(&Sha256::digest(
                b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
            )),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn only_this_attempt_s_callback_for_this_site_is_accepted() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let login = runtime.block_on(BrowserLogin::bind()).unwrap();
        let state = login.state.clone();
        let site = "https://agentrelay.com";
        let get = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n");
        let good = login
            .accept(
                &get(&format!(
                    "/callback/{state}?access_token=tok&api_url=https://agentrelay.com/cloud/&access_token_expires_at=2026-10-01T00:00:00Z"
                )),
                site,
            )
            .unwrap();
        assert_eq!(good.access_token.expose(), "tok");
        assert_eq!(good.site_url, site);
        assert_eq!(
            good.access_token_expires_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert!(good.refresh_token.is_none());
        // State in the query, for a return that kept it.
        assert!(login
            .accept(
                &get(&format!(
                    "/callback?state={state}&access_token=tok&api_url=https://agentrelay.com/cloud"
                )),
                site
            )
            .is_some());
        for forged in [
            "/callback?state=OTHER&access_token=tok&api_url=https://agentrelay.com/cloud"
                .to_string(),
            "/callback/OTHER?access_token=tok&api_url=https://agentrelay.com/cloud".to_string(),
            format!("/callback/{state}?access_token=tok&api_url=https://evil.example/cloud"),
            format!("/callback/{state}?access_token=&api_url=https://agentrelay.com/cloud"),
            format!("/callback/{state}?api_url=https://agentrelay.com/cloud"),
            format!("/elsewhere/{state}?access_token=tok&api_url=https://agentrelay.com/cloud"),
        ] {
            assert!(login.accept(&get(&forged), site).is_none(), "{forged}");
        }
        assert!(login
            .accept(
                &format!("POST /callback/{state}?access_token=tok&api_url=https://agentrelay.com/cloud HTTP/1.1\r\n\r\n"),
                site
            )
            .is_none());
    }
}
