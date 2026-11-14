//! A core still running another build than this app carries, as a manual
//! install can leave behind, is a problem the person is told about. On
//! Windows a core of an older release is first replaced: an upgrade over a
//! running app (Setup, a zip) can leave the old release's core serving, and
//! nothing there restarts it on the new build.

use super::super::Bridge;
use crate::host::{HostOptions, SHUTDOWN_WAIT};
use agent_relay_desktop_core::executable_identity::core_replaced_by_app;
use agent_relay_desktop_core::probe_copy::Release;
use agent_relay_node::relay::transport::CoreStream;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;

/// How long after a replacement that did not stop the older core the next
/// is tried: doubled after each one that fails, up to [`LAST_RETRY`], so a
/// core that ignores `daemon.shutdown` is used meanwhile, never asked in a
/// loop.
const FIRST_RETRY: Duration = Duration::from_secs(10);
const LAST_RETRY: Duration = Duration::from_secs(300);

/// What this run did about cores of older releases
/// ([`HostOptions::replace_older_core`]), owned by the bridge.
#[derive(Default)]
pub(in crate::bridge) struct OlderCores(Mutex<Replacements>);

#[derive(Debug, Default)]
struct Replacements {
    /// Every release whose core this run stopped, build metadata aside. A
    /// core of one that answers again (a logon task still pointed at the
    /// old app) is used, so a service that keeps starting it never loops.
    replaced: Vec<Release>,
    /// Replacements in a row that left the older core running.
    failures: u32,
    /// Until when no replacement is tried after one that failed.
    retry_at: Option<Instant>,
}

impl Replacements {
    /// Settles an attempt to replace a core of `release`: recorded only
    /// once it stopped; otherwise the next attempt waits, longer each time.
    fn settle(&mut self, release: Option<Release>, stopped: bool, now: Instant) {
        if stopped {
            self.failures = 0;
            self.retry_at = None;
            self.replaced.extend(release);
            return;
        }
        let delay = FIRST_RETRY
            .saturating_mul(1u32 << self.failures.min(8))
            .min(LAST_RETRY);
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + delay);
    }

    /// Whether an attempt that failed holds the next one back at `now`.
    fn waiting(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now < at)
    }
}

impl Bridge {
    /// Where the app replaces a core of an older release
    /// ([`crate::host::HostOptions::replace_older_core`]): whether the core
    /// that answered is one, now asked to stop and waited for until it frees
    /// the socket, so the next attempt starts the logon task (which the app
    /// keeps pointed at itself) or hosts this app's core. A core this app
    /// hosts is its own, and one is kept while this app has no helper to
    /// replace it with. A release is replaced once per run: if a core of it
    /// answers again, it is used, and [`Bridge::stale_core`] says so. A core
    /// that did not stop is used too, and asked again after a while.
    pub(in crate::bridge) async fn replaced_older_core(&self) -> bool {
        let Some(host) = self.inner.options.host.as_ref() else {
            return false;
        };
        let (Some(bundled), Some(running)) = (host.version.as_deref(), self.daemon_version())
        else {
            return false;
        };
        {
            let state = self.inner.older_cores.lock();
            if self.inner.retiring.load(Ordering::SeqCst) {
                return true;
            }
            if state.waiting(Instant::now())
                || !replaces_older_core(
                    host.replace_older_core,
                    self.hosting(),
                    has_helper(host),
                    &running,
                    bundled,
                    &state.replaced,
                )
            {
                return false;
            }
            self.inner.retiring.store(true, Ordering::SeqCst);
        }
        let asked = self.ask_core_to_stop().await;
        let stopped = self.socket_freed(asked).await;
        self.inner
            .older_cores
            .lock()
            .settle(Release::parse(&running), stopped, Instant::now());
        self.inner.retiring.store(false, Ordering::SeqCst);
        stopped
    }

    /// Whether the core asked to stop frees the socket within
    /// [`SHUTDOWN_WAIT`]; at once when it could not be `asked`.
    async fn socket_freed(&self, asked: bool) -> bool {
        let socket = &self.inner.options.socket;
        let deadline = Instant::now() + if asked { SHUTDOWN_WAIT } else { Duration::ZERO };
        while Instant::now() < deadline && CoreStream::connect(socket).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        CoreStream::connect(socket).await.is_err()
    }

    /// The person-facing problem when the core that answered runs another
    /// build than the one this app carries. `None` when they match or this
    /// app carries no core version.
    pub fn stale_core(&self) -> Option<String> {
        let bundled = self.inner.options.host.as_ref()?.version.as_deref()?;
        stale_core_message(self.daemon_version().as_deref(), bundled)
    }
}

impl OlderCores {
    fn lock(&self) -> std::sync::MutexGuard<'_, Replacements> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether this app has the helper program a replaced core's successor
/// runs, whether this app hosts it or the logon task starts it.
fn has_helper(host: &HostOptions) -> bool {
    host.probe.as_deref().is_some_and(Path::is_file)
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
/// (`replace_older_core`), never a core this app hosts, only while this app
/// has a `helper` to replace it with, only one of an older release
/// ([`core_replaced_by_app`]), and only once per release (`replaced`, every
/// release this run replaced, compared without build metadata).
fn replaces_older_core(
    replace_older_core: bool,
    hosting: bool,
    helper: bool,
    running: &str,
    bundled: &str,
    replaced: &[Release],
) -> bool {
    replace_older_core
        && !hosting
        && helper
        && Release::parse(running).is_some_and(|release| !replaced.contains(&release))
        && core_replaced_by_app(running, bundled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &str = "2026.11.12";

    fn release(text: &str) -> Release {
        Release::parse(text).unwrap()
    }

    /// Where the app replaces cores and has a helper, with `replaced` the
    /// releases replaced this run.
    fn replaces(running: &str, replaced: &[Release]) -> bool {
        replaces_older_core(true, false, true, running, BUNDLED, replaced)
    }

    /// Installing 2026.11.12 over a running 2026.11.9 on Windows: the new
    /// app replaces the old core it finds rather than attach to it.
    #[test]
    fn an_older_releases_core_is_replaced_where_the_app_replaces_cores() {
        assert!(replaces("2026.11.9", &[]));
        // Another release replaced earlier this run does not hold it back.
        assert!(replaces("2026.11.9", &[release("2026.11.6")]));
    }

    /// macOS and Linux, where launchd and systemd restart the core, leave
    /// it alone; so does every app for a core it hosts, one it has no
    /// helper to replace, one of the same or a newer release, and one of a
    /// release it already replaced this run.
    #[test]
    fn other_cores_are_used() {
        let older = "2026.11.9";
        assert!(!replaces_older_core(
            false,
            false,
            true,
            older,
            BUNDLED,
            &[]
        ));
        assert!(!replaces_older_core(true, true, true, older, BUNDLED, &[]));
        assert!(!replaces_older_core(
            true,
            false,
            false,
            older,
            BUNDLED,
            &[]
        ));
        assert!(!replaces("2026.11.12", &[]));
        assert!(!replaces("2026.11.13", &[]));
        assert!(!replaces(older, &[release(older)]));
    }

    /// Cores of 2026.11.6, then 2026.11.9, then 2026.11.6 again: each
    /// release is replaced once, however many others come between.
    #[test]
    fn every_replaced_release_is_remembered() {
        let mut state = Replacements::default();
        let now = Instant::now();
        for (running, replaced) in [
            ("2026.11.6", true),
            ("2026.11.9", true),
            ("2026.11.6", false),
            ("2026.11.9", false),
        ] {
            assert_eq!(replaces(running, &state.replaced), replaced, "{running}");
            if replaced {
                state.settle(Release::parse(running), true, now);
            }
        }
    }

    /// Another build of a replaced release is that release.
    #[test]
    fn a_replaced_release_is_remembered_whatever_its_build() {
        let mut state = Replacements::default();
        assert!(replaces("2026.11.9+abc123", &state.replaced));
        state.settle(Release::parse("2026.11.9+abc123"), true, Instant::now());
        assert!(!replaces("2026.11.9+def456", &state.replaced));
        assert!(!replaces("2026.11.9", &state.replaced));
    }

    /// A core that did not stop is not recorded as replaced: it is used
    /// for a while, longer after each failure up to a bound, then asked
    /// again. Once one stops, the next failure waits the shortest time.
    #[test]
    fn a_core_that_did_not_stop_is_asked_again_later() {
        let mut state = Replacements::default();
        let now = Instant::now();
        state.settle(Release::parse("2026.11.9"), false, now);
        assert!(state.replaced.is_empty());
        assert!(state.waiting(now));
        assert!(state.waiting(now + FIRST_RETRY - Duration::from_millis(1)));
        assert!(!state.waiting(now + FIRST_RETRY));
        assert!(replaces("2026.11.9", &state.replaced));

        state.settle(Release::parse("2026.11.9"), false, now);
        assert!(state.waiting(now + FIRST_RETRY));
        assert!(!state.waiting(now + FIRST_RETRY * 2));
        for _ in 0..20 {
            state.settle(Release::parse("2026.11.9"), false, now);
        }
        assert!(!state.waiting(now + LAST_RETRY));

        state.settle(Release::parse("2026.11.9"), true, now);
        assert!(!state.waiting(now));
        assert!(!replaces("2026.11.9", &state.replaced));
        state.settle(Release::parse("2026.11.6"), false, now);
        assert!(!state.waiting(now + FIRST_RETRY));
    }

    #[test]
    fn a_core_of_another_build_is_a_visible_problem() {
        assert_eq!(stale_core_message(Some("1.4.0"), "1.4.0"), None);
        assert_eq!(stale_core_message(None, "1.4.0"), None);
        let message = stale_core_message(Some("1.3.2"), "1.4.0").unwrap();
        assert!(message.contains("still running Agent Relay 1.3.2, not 1.4.0"));
    }
}
