//! A core still running another build than this app carries, as a manual
//! install can leave behind, is a problem the person is told about. On
//! Windows a core of an older release is first replaced: an upgrade over a
//! running app (Setup, a zip) can leave the old release's core serving, and
//! nothing there restarts it on the new build.

use super::super::Bridge;
use crate::host::SHUTDOWN_WAIT;
use agent_relay_desktop_core::executable_identity::core_replaced_by_app;
use agent_relay_node::relay::transport::CoreStream;
use std::sync::atomic::Ordering;
use std::sync::PoisonError;
use std::time::Duration;
use tokio::time::Instant;

impl Bridge {
    /// Where the app replaces a core of an older release
    /// ([`crate::host::HostOptions::replace_older_core`]): whether the core
    /// that answered is one, now asked to stop, waited for until it frees
    /// the socket, so the next attempt starts the logon task (which the app
    /// keeps pointed at itself) or hosts this app's core. A core this app
    /// hosts is its own. A release is replaced once per run: if a core of
    /// it answers again (a logon task still pointed at the old app), it is
    /// used, and [`Bridge::stale_core`] says so.
    pub(in crate::bridge) async fn replaced_older_core(&self) -> bool {
        let Some(host) = self.inner.options.host.as_ref() else {
            return false;
        };
        let (Some(bundled), Some(running)) = (host.version.as_deref(), self.daemon_version())
        else {
            return false;
        };
        {
            let mut replaced = self
                .inner
                .replaced_release
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self.inner.retiring.load(Ordering::SeqCst) {
                return true;
            }
            if !replaces_older_core(
                host.replace_older_core,
                self.hosting(),
                &running,
                bundled,
                replaced.as_deref(),
            ) {
                return false;
            }
            *replaced = Some(running);
            self.inner.retiring.store(true, Ordering::SeqCst);
        }
        let socket = &self.inner.options.socket;
        let _ = self.ask_core_to_stop().await;
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while Instant::now() < deadline && CoreStream::connect(socket).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.inner.retiring.store(false, Ordering::SeqCst);
        true
    }

    /// The person-facing problem when the core that answered runs another
    /// build than the one this app carries. `None` when they match or this
    /// app carries no core version.
    pub fn stale_core(&self) -> Option<String> {
        let bundled = self.inner.options.host.as_ref()?.version.as_deref()?;
        stale_core_message(self.daemon_version().as_deref(), bundled)
    }
}

/// The macOS app's wording (`AppModel.staleCoreMessage`) for a core still
/// running another build than the app carries.
pub fn stale_core_message(running: Option<&str>, bundled: &str) -> Option<String> {
    let running = running?;
    (running != bundled).then(|| {
        format!(
            "The background service is still running Agent Relay {running}, not {bundled}. Quit Agent Relay and open it again; if this stays, restart your computer."
        )
    })
}

/// Whether the core that answered, reporting `running`, is replaced by
/// this app carrying `bundled`: only where the app replaces older cores
/// (`replace_older_core`), never a core this app hosts, only one of an
/// older release ([`core_replaced_by_app`]), and only once per release
/// (`already`, the release this run replaced).
fn replaces_older_core(
    replace_older_core: bool,
    hosting: bool,
    running: &str,
    bundled: &str,
    already: Option<&str>,
) -> bool {
    replace_older_core
        && !hosting
        && already != Some(running)
        && core_replaced_by_app(running, bundled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installing 2026.11.12 over a running 2026.11.9 on Windows: the new
    /// app replaces the old core it finds rather than attach to it.
    #[test]
    fn an_older_releases_core_is_replaced_where_the_app_replaces_cores() {
        assert!(replaces_older_core(
            true,
            false,
            "2026.11.9",
            "2026.11.12",
            None
        ));
        // Another release replaced earlier this run does not hold it back.
        assert!(replaces_older_core(
            true,
            false,
            "2026.11.9",
            "2026.11.12",
            Some("2026.11.6")
        ));
    }

    /// macOS and Linux, where launchd and systemd restart the core, leave
    /// it alone; so does every app for a core it hosts, one of the same or
    /// a newer release, and one of a release it already replaced this run.
    #[test]
    fn other_cores_are_used() {
        assert!(!replaces_older_core(
            false,
            false,
            "2026.11.9",
            "2026.11.12",
            None
        ));
        assert!(!replaces_older_core(
            true,
            true,
            "2026.11.9",
            "2026.11.12",
            None
        ));
        assert!(!replaces_older_core(
            true,
            false,
            "2026.11.12",
            "2026.11.12",
            None
        ));
        assert!(!replaces_older_core(
            true,
            false,
            "2026.11.13",
            "2026.11.12",
            None
        ));
        assert!(!replaces_older_core(
            true,
            false,
            "2026.11.9",
            "2026.11.12",
            Some("2026.11.9")
        ));
    }

    #[test]
    fn a_core_of_another_build_is_a_visible_problem() {
        assert_eq!(stale_core_message(Some("1.4.0"), "1.4.0"), None);
        assert_eq!(stale_core_message(None, "1.4.0"), None);
        let message = stale_core_message(Some("1.3.2"), "1.4.0").unwrap();
        assert!(message.contains("still running Agent Relay 1.3.2, not 1.4.0"));
    }
}
