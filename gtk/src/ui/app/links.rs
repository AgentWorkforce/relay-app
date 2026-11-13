//! `agentrelay://` links: the core parses them, for the scheme and the site
//! it was built with. A link that arrives while the core is not connected
//! brings the window up and waits for it, until the startup deadline; one
//! the core could not be reached for is tried once more, after a short
//! delay or on the next snapshot or reconnect, whichever comes first, and
//! only a refusal is shown as one (`AppModel.openConnection` on macOS does
//! the same).

use super::*;
use crate::bridge::Failure;
use crate::models::{link_hold_timed_out, link_refusal, LINK_STILL_STARTING};
use agent_relay_node::relay::contract::directory::LinkParams;
use agent_relay_node::relay::links::DeepLink;
use agent_relay_node::relay::protocol::ErrorKind;

/// How many times a link is tried again after the core could not be
/// reached for it.
const LINK_RETRIES: u8 = 1;

/// How long a link held for its retry waits while the core stays
/// connected. Nothing else may replay it then: a snapshot comes only when
/// something changes and a reconnect only when the event stream drops, so
/// a timed-out parse would otherwise never be tried again or answered.
/// Long enough for a core that was briefly busy to settle
/// (`AppModel.linkRetryDelay` on macOS).
const LINK_RETRY_DELAY: Duration = Duration::from_secs(2);

/// The core cannot parse a link yet: it is not connected, or has not sent
/// its first snapshot.
fn core_away(state: &State) -> bool {
    state.snapshot.is_none() || !state.core_connected
}

impl App {
    /// An `agentrelay://` link the person opened.
    /// While the core is away the window comes up, so the person sees the
    /// app starting or reconnecting; the same link already held is not held
    /// twice.
    pub fn open_link(&self, url: &str) {
        let (away, held) = self.with_state(|state| {
            let held = state.pending_links.iter().any(|(held, _)| held == url);
            (core_away(state), held)
        });
        if away {
            self.show_window();
        }
        if !held {
            self.parse_link(url.to_string(), 0);
        }
    }

    /// Tries the held links again, once the core is connected and has sent
    /// a snapshot; until then they stay held.
    pub(super) fn replay_links(&self) {
        let links = self.update_state(|state| std::mem::take(&mut state.pending_links));
        for (url, retries) in links {
            self.parse_link(url, retries);
        }
    }

    fn parse_link(&self, url: String, retries: u8) {
        if self.with_state(core_away) {
            self.hold_while_away(url, retries);
            return;
        }
        let params = LinkParams { url: url.clone() };
        self.call::<method::LinksParse>(params, move |app, result| match result {
            Ok(link) => {
                app.update_state(|state| state.link_error = None);
                match link {
                    DeepLink::Message { endpoint, .. } => {
                        app.show_window();
                        app.show_message_composer(&endpoint);
                    }
                    DeepLink::Connect { workspace_id, .. } => {
                        app.connect_link(PendingConnect { workspace_id })
                    }
                }
            }
            Err(error) => app.link_failed(url, retries, &error),
        });
    }

    /// Keeps why `url` was not opened, and says so: the core's sentence for
    /// a refusal or any other answer of its own; for a core that could not
    /// be reached, holds the link for one more try, then says it is still
    /// starting.
    fn link_failed(&self, url: String, retries: u8, error: &CoreError) {
        // The core says why; a link for another site names it
        // (AppModel.linkRefusal on macOS words it the same way).
        let cause = link_refusal(&error.message);
        self.keep_link_failure(cause.clone());
        let (failure, shown) = match error.failure {
            Failure::Core(ErrorKind::InvalidParams) => (TelemetryFailure::InvalidConnection, cause),
            Failure::Unavailable if retries < LINK_RETRIES => {
                self.hold_for_retry(url, retries + 1);
                return;
            }
            Failure::Unavailable if error.timed_out() => {
                (TelemetryFailure::TimedOut, LINK_STILL_STARTING.to_string())
            }
            Failure::Unavailable => (
                TelemetryFailure::Unexpected,
                LINK_STILL_STARTING.to_string(),
            ),
            Failure::Core(_) | Failure::Unreadable => (TelemetryFailure::Unexpected, cause),
        };
        self.reject_link(failure, &shown);
    }

    /// Keeps why a link was not opened, for Diagnostics, and logs it.
    fn keep_link_failure(&self, cause: String) {
        eprintln!("Agent Relay could not open a connection link: {cause}");
        self.update_state(|state| state.link_error = Some(cause));
    }

    /// Records that a link was not opened and says `shown`.
    fn reject_link(&self, failure: TelemetryFailure, shown: &str) {
        self.record(
            TelemetryOperation::ConnectionLink,
            TelemetryOutcome::Failed,
            Some(failure),
        );
        self.show_window();
        self.show_error(shown);
    }

    /// Holds `url` for its retry. With the core connected, a timer tries it
    /// after [`LINK_RETRY_DELAY`]; a snapshot or reconnect that comes first
    /// replays it instead, and the timer then finds it gone. A link held
    /// while the core is away waits for the reconnect, until the deadline.
    fn hold_for_retry(&self, url: String, retries: u8) {
        if !self.with_state(|state| state.core_connected) {
            self.hold_while_away(url, retries);
            return;
        }
        self.update_state(|state| state.pending_links.push((url.clone(), retries)));
        self.take_after(LINK_RETRY_DELAY, url, retries, |app, url, retries| {
            app.parse_link(url, retries)
        });
    }

    /// Holds `url` until the core is connected and has sent a snapshot, as
    /// long as startup waits for the core (`state.startup_timeout`): a core
    /// that stays away would otherwise leave the link held for good and the
    /// person with no answer. A snapshot or reconnect that replays it first
    /// leaves the deadline nothing to drop.
    fn hold_while_away(&self, url: String, retries: u8) {
        let timeout = self.update_state(|state| {
            state.pending_links.push((url.clone(), retries));
            state.startup_timeout
        });
        self.take_after(timeout, url, retries, move |app, _, _| {
            app.keep_link_failure(link_hold_timed_out(timeout));
            app.reject_link(TelemetryFailure::TimedOut, LINK_STILL_STARTING);
        });
    }

    /// After `delay`, takes `url` out of the held links and runs `then`,
    /// unless a snapshot or reconnect replayed it first.
    fn take_after(
        &self,
        delay: Duration,
        url: String,
        retries: u8,
        then: impl FnOnce(&App, String, u8) + 'static,
    ) {
        let weak = self.downgrade();
        glib::timeout_add_local_once(delay, move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let held = app.update_state(|state| {
                let index = state
                    .pending_links
                    .iter()
                    .position(|(held, tries)| *held == url && *tries == retries)?;
                Some(state.pending_links.remove(index))
            });
            if let Some((url, retries)) = held {
                then(&app, url, retries);
            }
        });
    }
}
