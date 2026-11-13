//! Software update actions shared by the About pane and background checks.
use super::app::App;
use super::desktop::Confirmation;
use super::state::UpdateState;
use super::widgets;
use crate::bridge::Bridge;
use crate::build_profile::{self, Profile, ReleaseFeed};
use crate::updates::{self, Release};
use agent_relay_desktop_core::contract::account::FeedHeadersParams;
use agent_relay_desktop_core::contract::method::AccountFeedHeaders;
use agent_relay_desktop_core::install::{self, InstallKind};
use agent_relay_desktop_core::telemetry::{TelemetryFailure, TelemetryOperation, TelemetryOutcome};
use gtk::{gio, glib, prelude::*};
use gtk4 as gtk;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What finishing an update left.
enum Updated {
    /// The new version's launcher, which replaces this process.
    Installed(PathBuf),
    /// The verified package, for the package manager.
    Downloaded(PathBuf),
}

/// What a cloud feed request needs from the app, taken on the main thread:
/// the signed-in account's pins and the core that signs for them.
struct FeedAuth {
    workspace: Option<String>,
    account: Option<String>,
    bridge: Bridge,
    runtime: tokio::runtime::Handle,
}

impl FeedAuth {
    /// Fresh `account.feed_headers` for the pinned account on `site` (the
    /// built-in site); `None` when signed out, when the session is on
    /// another site, when the core cannot supply them (an older core
    /// included), or when the account changed since the pins were taken.
    fn headers(&self, site: &str) -> Option<BTreeMap<String, String>> {
        let (Some(workspace), Some(account)) = (&self.workspace, &self.account) else {
            return None;
        };
        if !self.bridge.offers(AccountFeedHeaders::NAME) {
            return None;
        }
        let params = FeedHeadersParams {
            site_url: site.to_owned(),
            workspace_id: workspace.clone(),
            account_id: account.clone(),
            profile_id: build_profile::profile_id().to_owned(),
        };
        self.runtime
            .block_on(self.bridge.call::<AccountFeedHeaders>(params))
            .ok()
            .map(|answer| answer.headers)
    }
}

/// Runs `body` with this system's [`updates::Host`]: for a build with a
/// cloud release feed, with the built-in site's feed (headers from
/// `auth`); otherwise with none, so GitHub is read.
fn with_host<T>(
    auth: &FeedAuth,
    body: impl FnOnce(&updates::Host<'_>) -> Result<T, updates::UpdateError>,
) -> Result<T, updates::UpdateError> {
    let host =
        updates::Host::system().map_err(|error| updates::UpdateError::new(error.to_string()))?;
    match Profile::BUILT.release_source() {
        ReleaseFeed::GitHub => body(&host),
        ReleaseFeed::Cloud => {
            let headers = |site: &str| auth.headers(site);
            let feed = updates::CloudFeed::built(&headers);
            body(&updates::Host {
                feed: Some(&feed),
                ..host
            })
        }
    }
}

impl App {
    // Updates.

    /// The signed-in account's pins and the core, for [`FeedAuth`].
    fn feed_auth(&self) -> FeedAuth {
        let account = self.with_state(|state| state.account());
        FeedAuth {
            workspace: account.workspace_id,
            account: account.person.and_then(|person| person.id),
            bridge: self.bridge().clone(),
            runtime: self.inner.runtime.clone(),
        }
    }

    /// Looks for a newer release; unless `user` asked, only while automatic
    /// checks are on.
    pub fn check_updates(&self, user: bool) {
        if self.with_state(|state| state.update_state.busy()) {
            return;
        }
        if !user && !self.prefs().automatically_checks_for_updates() {
            return;
        }
        self.update_state(|state| state.update_state = UpdateState::Checking);
        self.render_settings();
        let include_beta = self.prefs().include_beta_releases();
        let auth = self.feed_auth();
        self.blocking(
            move || -> Result<Option<Release>, updates::UpdateError> {
                let arch = updates::current_architecture()?;
                with_host(&auth, |host| match host.feed {
                    // No newer release in the site's feed is `None`.
                    Some(_) => updates::cloud::latest(arch, include_beta, host),
                    None => updates::latest_for_channel(arch, include_beta, host.fetch).map(Some),
                })
            },
            |app, result| {
                app.update_state(|state| {
                    match result {
                        Ok(None) => {
                            state.update_state = UpdateState::UpToDate;
                            state.update_release = None;
                            state.update_message = None;
                        }
                        Ok(Some(release)) => {
                            state.update_state = if updates::is_newer(
                                &release.version,
                                agent_relay_node::VERSION,
                            ) {
                                UpdateState::Available
                            } else {
                                UpdateState::UpToDate
                            };
                            state.update_release = Some(release);
                            state.update_message = None;
                        }
                        Err(error) => {
                            state.update_state = UpdateState::Unavailable;
                            state.update_release = None;
                            state.update_message = Some(error.to_string());
                        }
                    }
                    state.update_checked = Some(chrono::Local::now().format("%H:%M").to_string());
                });
                if app.with_state(|state| state.connected()) {
                    app.render_chrome();
                    app.render_settings();
                }
            },
        );
    }

    /// About's update button: checks, or installs the available release as
    /// this install updates.
    pub fn update_action(&self) {
        let (state, release, kind) = self.with_state(|state| {
            (
                state.update_state,
                state.update_release.clone(),
                state.install_kind,
            )
        });
        let (UpdateState::Available, Some(release)) = (state, release) else {
            self.check_updates(true);
            return;
        };
        match kind {
            InstallKind::Source => {
                let page = if release.page_url.is_empty() {
                    // A build from source points at its profile's releases.
                    Profile::BUILT.releases_page()
                } else {
                    Some(release.page_url.clone())
                };
                if let Some(page) = page {
                    self.inner.desktop.open_uri(&page);
                }
            }
            InstallKind::Tarball => {
                let answer = self.confirm(Confirmation::new(
                    "Update Agent Relay now?",
                    &format!("Agent Relay will restart to install version {}. Your agents reconnect and uploads continue as soon as the new version starts.", release.version),
                    "Update",
                ));
                let weak = self.downgrade();
                glib::spawn_future_local(async move {
                    if let (true, Some(app)) = (answer.await, weak.upgrade()) {
                        app.install_update(release, kind);
                    }
                });
            }
            InstallKind::Deb => self.install_update(release, kind),
        }
    }

    fn install_update(&self, release: Release, kind: InstallKind) {
        self.record(
            TelemetryOperation::UpdateInstall,
            TelemetryOutcome::Started,
            None,
        );
        self.update_state(|state| state.update_state = UpdateState::Downloading);
        self.render_settings();
        let auth = self.feed_auth();
        self.blocking(
            move || -> Result<Updated, updates::UpdateError> {
                with_host(&auth, |host| {
                    if kind == InstallKind::Tarball {
                        let exe = install::current_exe()
                            .map_err(|error| updates::UpdateError::new(error.to_string()))?;
                        let root = install::install_root(&exe, &host.home);
                        updates::install_per_user(&release, &root, host).map(Updated::Installed)
                    } else {
                        updates::download_deb(&release, None, host).map(Updated::Downloaded)
                    }
                })
            },
            |app, result| app.update_finished(result),
        );
    }

    fn update_finished(&self, result: Result<Updated, updates::UpdateError>) {
        match result {
            Err(error) => {
                self.record(
                    TelemetryOperation::UpdateInstall,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::UpdateFailed),
                );
                self.update_state(|state| {
                    state.update_state = UpdateState::Failed;
                    state.update_message = Some(error.to_string());
                });
                self.render_settings();
            }
            Ok(Updated::Downloaded(path)) => {
                self.record(
                    TelemetryOperation::UpdateInstall,
                    TelemetryOutcome::RequiresAction,
                    None,
                );
                self.update_state(|state| {
                    state.update_state = UpdateState::Available;
                    state.update_message = None;
                });
                widgets::copy_to_clipboard(&format!(
                    "sudo apt-get install -y '{}'",
                    path.display()
                ));
                let folder = path.parent().map(PathBuf::from).unwrap_or_default();
                self.inner.desktop.inform(
                    self.window(),
                    "Update downloaded",
                    &format!("The verified package is in {}. Install it with your package manager; the command is on your clipboard.", folder.display()),
                );
                self.inner
                    .desktop
                    .open_uri(&gio::File::for_path(&folder).uri());
                self.render_settings();
            }
            Ok(Updated::Installed(launcher)) => {
                self.update_state(|state| state.update_state = UpdateState::Installing);
                self.render_settings();
                // The new version's launcher replaces this process and hosts
                // its own core, so a core this app hosts stops first. The
                // bridge keeps running, so the app stays usable when the
                // launcher cannot start.
                let bridge = self.bridge().clone();
                let event = agent_relay_desktop_core::telemetry::TelemetryEvent::new(
                    TelemetryOperation::UpdateInstall,
                    TelemetryOutcome::Succeeded,
                );
                // Bound the entire RPC (including connection/retries), while
                // allowing the core's two-second success flush before relaunch.
                let answer = self.inner.runtime.spawn(async move {
                    wait_for_update_telemetry(
                        bridge.call::<agent_relay_desktop_core::contract::method::TelemetryRecord>(event),
                        std::time::Duration::from_secs(3),
                    )
                    .await;
                });
                let weak = self.downgrade();
                glib::spawn_future_local(async move {
                    let _ = answer.await;
                    if let Some(app) = weak.upgrade() {
                        app.relaunch_updated(launcher);
                    }
                });
            }
        }
    }

    fn relaunch_updated(&self, launcher: PathBuf) {
        let background: Vec<String> = std::env::args()
            .skip(1)
            .filter(|argument| argument == "--background")
            .collect();
        let Err(error) = self.relaunch_into(&launcher, &background) else {
            return;
        };
        self.update_state(|state| {
            state.update_state = UpdateState::Failed;
            state.update_message = Some(format!(
                "Agent Relay could not restart into the new version: {error}."
            ));
        });
        self.render_settings();
    }
}

/// The deadline covers connection/retries as well as the core's flush.
async fn wait_for_update_telemetry(
    recording: impl std::future::Future,
    deadline: std::time::Duration,
) {
    let _ = tokio::time::timeout(deadline, recording).await;
}

#[cfg(test)]
mod tests {
    use super::wait_for_update_telemetry;
    use std::time::Duration;

    #[tokio::test]
    async fn update_telemetry_deadline_allows_relaunch_when_rpc_never_replies() {
        tokio::time::timeout(Duration::from_secs(1), async {
            wait_for_update_telemetry(std::future::pending::<()>(), Duration::from_millis(10))
                .await;
        })
        .await
        .expect("update must continue after the telemetry deadline");
    }

    #[tokio::test]
    async fn update_telemetry_waits_for_success_flush_reply() {
        let completed = std::sync::atomic::AtomicBool::new(false);
        wait_for_update_telemetry(
            async {
                tokio::task::yield_now().await;
                completed.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            Duration::from_secs(1),
        )
        .await;
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
    }
}
