//! Opening the device or browser sign-in an attempt then waits on.

use super::{failed, Account, Flow, Start};
use crate::browser_login::BrowserLogin;
use agent_relay_node::relay::cloud_login::DeviceLogin;
use agent_relay_node::relay::protocol::RpcError;
use crate::contract::account::SignInProvider;

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

impl Account {
    /// Opens `flow`'s sign-in for `attempt`; one that cannot open ends the
    /// attempt with its reason.
    pub(super) async fn begin_login(
        &self,
        attempt: u64,
        site: &str,
        flow: Flow,
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
        }
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
