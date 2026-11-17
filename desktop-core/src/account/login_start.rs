//! Opening the device or browser sign-in an attempt then waits on.

use super::{browser_link, enrollment_key, failed, validation, Account, Flow, Start};
use crate::browser_login::BrowserLogin;
use agent_relay_node::relay::cloud_login::{DeviceLogin, LoginError};
use agent_relay_node::relay::contract::account::SignInLink;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use agent_relay_node::relay::websocket_tls::platform_root_environment_is_safe;
use crate::contract::account::{SignInProvider, SignInRequest};
use std::path::Path;
use std::sync::Once;

/// The state and phase of a sign-in refused because `SSL_CERT_FILE` or
/// `SSL_CERT_DIR` is set: Cloud calls never trust file-selected roots.
pub(crate) const TLS_ROOTS_REFUSED: &str = "tls_roots_refused";
/// What such a sign-in says. Static: it never names the variables' values.
pub(crate) const TLS_ROOTS_REFUSED_MESSAGE: &str = "Agent Relay does not accept SSL_CERT_FILE or SSL_CERT_DIR certificate overrides. Unset them (for example `env -u SSL_CERT_FILE -u SSL_CERT_DIR agent-relay-probe ...`) and add your CA to the operating system trust store.";

/// Whether a certificate-file override would make every Cloud call fail,
/// as node's `platform_root_environment_is_safe` decides. The first refusal
/// is written to core.log once, without the variables' values.
fn certificate_overrides_refused() -> bool {
    if platform_root_environment_is_safe() {
        return false;
    }
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        // Never print the environment values: they can contain private paths.
        agent_relay_node::relay::diagnostics::write(format_args!(
            "relay: sign-in refuses certificate-file overrides; unset SSL_CERT_FILE and SSL_CERT_DIR and install the CA in the operating system trust store"
        ));
    });
    true
}

/// Keep the loopback receiver, state, workspace and PKCE challenge intact,
/// choosing only which Cloud OAuth entry point opens in the browser.
pub(super) fn authorization_url(
    login: &BrowserLogin,
    site: &str,
    reuse: bool,
    workspace: Option<&str>,
    provider: Option<SignInProvider>,
) -> Option<String> {
    let link = login.authorization_url_for_workspace(site, reuse && provider.is_none(), workspace)?;
    let mut url = url::Url::parse(&link).ok()?;
    if let Some(provider) = provider {
        url.set_path(match provider {
            SignInProvider::Google => "/cloud/api/auth/google/start",
            SignInProvider::Microsoft => "/cloud/api/auth/microsoft/start",
        });
    }
    Some(url.into())
}

/// Refuses a request naming something that is not a workspace, or whose
/// flow and enrollment key file disagree.
pub(super) fn check_request(request: &SignInRequest) -> Result<(), RpcError> {
    if let Some(workspace) = request.workspace.as_deref() {
        if !validation::valid_id(workspace) {
            return Err(RpcError::invalid_params(
                "That is not an Agent Relay workspace.",
            ));
        }
    }
    enrollment_key::check_request(request)
}

/// The link a browser sign-in waits on, as [`authorization_url`] makes it;
/// refused when the site is not an https origin.
pub(super) fn browser_start_link(
    login: &BrowserLogin,
    site: &str,
    reuse: bool,
    workspace: Option<&str>,
    provider: Option<SignInProvider>,
) -> Result<SignInLink, RpcError> {
    authorization_url(login, site, reuse, workspace, provider)
        .map(browser_link)
        .ok_or_else(|| {
            RpcError::new(
                ErrorKind::Refused,
                "The Agent Relay site must be an https origin.",
            )
        })
}

impl Account {
    /// Opens `flow`'s sign-in for `attempt`; one that cannot open ends the
    /// attempt with its reason. An enrollment key is read from `key_file`.
    pub(super) async fn begin_login(
        &self,
        attempt: u64,
        site: &str,
        flow: Flow,
        key_file: Option<&Path>,
    ) -> Result<Start, RpcError> {
        match flow {
            Flow::Device => match DeviceLogin::begin(site, Some(&self.desktop_host()?)).await {
                Ok(login) => Ok(Start::Device(login)),
                Err(error) => {
                    self.fail(attempt, &error);
                    Err(failed(error.message))
                }
            },
            Flow::Browser => match BrowserLogin::bind().await {
                Ok(login) => Ok(Start::Browser(
                    login.with_desktop_host(&self.desktop_host()?),
                )),
                Err(error) => {
                    self.fail(attempt, &error);
                    Err(failed(error.message))
                }
            },
            Flow::EnrollmentKey => self.enrollment_key_start(attempt, key_file),
        }
    }

    /// Ends `attempt` as `tls_roots_refused` when `SSL_CERT_FILE` or
    /// `SSL_CERT_DIR` is set. Every sign-in calls Cloud, whose clients refuse
    /// file-selected roots; this says so before any flow starts, instead of
    /// the generic "Could not reach Agent Relay Cloud." a refused call gives.
    pub(super) fn refuse_certificate_overrides(&self, attempt: u64) -> Result<(), RpcError> {
        if !certificate_overrides_refused() {
            return Ok(());
        }
        self.fail(
            attempt,
            &LoginError {
                state: TLS_ROOTS_REFUSED,
                message: TLS_ROOTS_REFUSED_MESSAGE.into(),
            },
        );
        Err(RpcError::new(ErrorKind::Refused, TLS_ROOTS_REFUSED_MESSAGE))
    }

    /// A browser sign-in that is the desktop's own, naming this computer.
    pub(super) async fn desktop_browser_login(&self) -> Result<BrowserLogin, RpcError> {
        let login = BrowserLogin::bind()
            .await
            .map_err(|error| failed(error.message))?;
        Ok(login.with_desktop_host(&self.desktop_host()?))
    }

    /// This computer's name, which a desktop sign-in carries for the
    /// approval page to show.
    pub(super) fn desktop_host(&self) -> Result<String, RpcError> {
        let core = self
            .core()
            .ok_or_else(|| failed("Could not identify this computer for sign-in."))?;
        Ok(core.config.host_name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::account::{Flow, SharingMode};
    use agent_relay_node::relay::daemon::{Config, Core};
    use agent_relay_node::relay::paths::CorePaths;
    use agent_relay_node::relay::state::SignInState;
    use std::sync::Arc;

    const CHILD: &str = "RELAY_SIGN_IN_TLS_CHILD";
    const PRIVATE_ROOTS: &str = "unused-test-root-path";

    /// Run in a child process the parent test starts with the environment
    /// it names, so no test here mutates this process's environment.
    #[test]
    fn certificate_override_child() {
        let Ok(mode) = std::env::var(CHILD) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://127.0.0.1:{}", closed.local_addr().unwrap().port());
        drop(closed);
        let account = Arc::new(
            Account::new(
                &Default::default(),
                &dir.path().join("app-data"),
                Arc::new(crate::history::uploader::Uploader::new(Arc::new(
                    crate::history::uploader::ProcessRunner::new(dir.path().join("no-probe")),
                ))),
            )
            .with_site(&site)
            .unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (core, _signals) = runtime.block_on(async {
            Core::new(Config {
                paths: CorePaths::for_socket(dir.path().join("core.sock")).unwrap(),
                home: dir.path().into(),
                app_data_dir: dir.path().join("app-data"),
                env: Default::default(),
                version: "0.0.0-test".into(),
                host_name: "Test Desk.local".into(),
                rediscover_every: std::time::Duration::from_secs(3600),
                push_timing: Default::default(),
                delivery_timing: Default::default(),
            })
        });
        account.attach(&core);
        for flow in [
            Some(Flow::Device),
            None,
            Some(Flow::Browser),
            Some(Flow::Device),
        ] {
            let result = runtime.block_on(account.sign_in(SignInRequest {
                workspace: None,
                sharing: SharingMode::Selected,
                acknowledge_uninspected_schedules: false,
                flow,
                provider: None,
                enrollment_key_file: None,
            }));
            let status = account.status();
            if mode == "refused" {
                let error = result.unwrap_err();
                assert_eq!(error.data.kind, ErrorKind::Refused);
                assert_eq!(error.message, TLS_ROOTS_REFUSED_MESSAGE);
                assert_eq!(status.state, SignInState::Error);
                assert_eq!(status.phase.as_deref(), Some(TLS_ROOTS_REFUSED));
                assert_eq!(status.error.as_deref(), Some(TLS_ROOTS_REFUSED_MESSAGE));
                assert!(!TLS_ROOTS_REFUSED_MESSAGE.contains(PRIVATE_ROOTS));
            } else {
                assert_ne!(status.phase.as_deref(), Some(TLS_ROOTS_REFUSED));
                assert_ne!(status.error.as_deref(), Some(TLS_ROOTS_REFUSED_MESSAGE));
            }
        }
    }

    #[test]
    fn a_certificate_override_refuses_sign_in_with_its_reason_logged_once() {
        for (key, value, refused) in [
            ("SSL_CERT_FILE", PRIVATE_ROOTS, true),
            ("SSL_CERT_DIR", PRIVATE_ROOTS, true),
            ("SSL_CERT_FILE", "", false),
            ("SSL_CERT_DIR", "", false),
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "account::login_start::tests::certificate_override_child",
                ])
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env(key, value)
                .env(CHILD, if refused { "refused" } else { "allowed" })
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{key}={value:?}: {stdout}");
            assert!(stdout.contains("1 passed"));
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let logged = diagnostic
                .matches("sign-in refuses certificate-file overrides")
                .count();
            assert_eq!(
                logged,
                usize::from(refused),
                "{key}={value:?}: {diagnostic}"
            );
            assert!(!diagnostic.contains(PRIVATE_ROOTS));
        }
    }
}
