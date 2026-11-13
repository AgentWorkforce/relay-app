//! Judges session activity again when it changes by time alone. A Devin
//! Cloud session is Active only within a day of its last activity, so its
//! Status, `is:active` and the tray's counts change with no new data; the
//! check runs at the first such change (never later than
//! `ACTIVITY_RECHECK_MS`, which also covers a sleep or a clock set
//! forward), as macOS's `RelayModel` does.
use super::*;
use crate::models::next_activity_check;
use gtk4::glib;
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    /// The pending check. The app lives on the GTK main thread, one per
    /// process, so the check is kept beside the code that sets it.
    static PENDING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

impl App {
    /// Replaces the pending activity check with one at the next change.
    pub(super) fn schedule_activity_check(&self) {
        let now = chrono::Utc::now().timestamp_millis();
        let due = self.with_state(|state| next_activity_check(&state.local_sessions, now));
        let wait = Duration::from_millis(u64::try_from(due.saturating_sub(now)).unwrap_or(0));
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(wait, move || {
            // This source is spent: removing it again would panic.
            PENDING.with(|pending| pending.borrow_mut().take());
            let Some(app) = weak.upgrade() else {
                return;
            };
            // Redraws every surface with the current time, reads the core's
            // page again if any activity changed, and schedules the next.
            app.relay_changed();
        });
        if let Some(previous) = PENDING.with(|pending| pending.borrow_mut().replace(source)) {
            previous.remove();
        }
    }
}
