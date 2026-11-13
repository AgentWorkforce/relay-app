//! The uploader's safety-net read on the app's poll.
use super::app::App;

/// Poll ticks between reads of the uploader without an event: 60 s of the
/// app's 5 s poll.
pub(super) const SAFETY_NET_TICKS: u32 = 12;

/// The poll's next tick: it counts a minute of polls, then starts again.
pub(super) fn next_tick(tick: u32) -> u32 {
    (tick + 1) % SAFETY_NET_TICKS
}

impl App {
    /// The uploader's status and sessions arrive as `invalidated` events;
    /// the poll reads them again every 60 s only as a safety net.
    pub(super) fn uploader_safety_net(&self, ticks: u32, visible: bool) {
        if ticks % SAFETY_NET_TICKS == 0 {
            self.refresh_uploader(visible && self.page() == super::state::Page::Sessions);
        }
    }
}

#[cfg(test)]
mod safety_net_tests {
    /// Uploader status and sessions arrive as `invalidated` events; the
    /// poll is a minute-long safety net, as on the Mac.
    #[test]
    fn the_uploader_poll_is_a_minute_long_safety_net() {
        assert_eq!(
            super::super::app::POLL_INTERVAL * super::SAFETY_NET_TICKS,
            std::time::Duration::from_secs(60)
        );
    }
}
