//! How long an open conversation waits before it is read again after a
//! read failed. A failed read waits the server's Retry-After when the error
//! names one (held to at least a second), otherwise a delay that starts at
//! the normal cadence, doubles with each failure in a row and stops at five
//! minutes, with up to a quarter more at random so many windows do not
//! retry together. A read that succeeds goes back to the normal cadence.
//!
//! The Mac's counterpart is `RelayDesktopCore/ReadBackoff.swift`; the two
//! read the same.

use std::time::Duration;

/// Failed reads of one conversation in a row, and the wait they set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadBackoff {
    failures: u32,
}

impl ReadBackoff {
    /// How often an open conversation is read while reads succeed, and the
    /// wait after the first failure.
    pub const CADENCE: Duration = Duration::from_secs(10);
    /// The longest wait the doubling reaches.
    pub const CAP: Duration = Duration::from_secs(300);
    /// The shortest wait a Retry-After is held to.
    pub const SHORTEST_RETRY_AFTER: Duration = Duration::from_secs(1);
    /// The largest share of the wait added at random.
    pub const JITTER: f64 = 0.25;

    /// Failed reads in a row since the last one that succeeded.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// A read failed: how long to wait before the next one. `retry_after`
    /// is the server's, when it named one; `random` (in `0..1`) picks the
    /// jitter.
    pub fn failed(&mut self, retry_after: Option<Duration>, random: f64) -> Duration {
        self.failures = self.failures.saturating_add(1);
        if let Some(retry_after) = retry_after {
            return retry_after.max(Self::SHORTEST_RETRY_AFTER);
        }
        let doubled = Self::CADENCE.as_secs_f64() * 2f64.powi(self.failures.min(17) as i32 - 1);
        let base = doubled.min(Self::CAP.as_secs_f64());
        let share = random.clamp(0.0, 1.0) * Self::JITTER;
        Duration::from_secs_f64((base * (1.0 + share)).min(Self::CAP.as_secs_f64()))
    }

    /// A read succeeded: the next waits the normal cadence.
    pub fn succeeded(&mut self) {
        self.failures = 0;
    }
}

/// What the conversation says while it waits `delay` after a read that
/// failed with `message`.
pub fn read_failure_text(message: &str, delay: Duration) -> String {
    failure_text("Could not read the conversation", message, delay)
}

/// What the conversation says while it waits `delay` after a look-up of
/// its recipient failed with `message` (and its history did not).
pub fn recipient_failure_text(message: &str, delay: Duration) -> String {
    failure_text("Could not look up the recipient", message, delay)
}

fn failure_text(what: &str, message: &str, delay: Duration) -> String {
    let reason = message.trim().trim_end_matches('.');
    format!("{what}: {reason}. Trying again in {}.", read_wait(delay))
}

/// `delay` in whole seconds, rounded up: "20 s", "2 min 30 s", "5 min".
pub fn read_wait(delay: Duration) -> String {
    let seconds = (delay.as_secs_f64().ceil() as u64).max(1);
    if seconds < 60 {
        return format!("{seconds} s");
    }
    match (seconds / 60, seconds % 60) {
        (minutes, 0) => format!("{minutes} min"),
        (minutes, rest) => format!("{minutes} min {rest} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn the_first_failure_waits_the_normal_cadence_and_each_further_one_doubles() {
        let mut backoff = ReadBackoff::default();
        let waits: Vec<Duration> = (0..5).map(|_| backoff.failed(None, 0.0)).collect();
        assert_eq!(
            waits,
            [secs(10.0), secs(20.0), secs(40.0), secs(80.0), secs(160.0)]
        );
        assert_eq!(backoff.failures(), 5);
    }

    #[test]
    fn the_doubling_stops_at_five_minutes() {
        let mut backoff = ReadBackoff::default();
        let waits: Vec<Duration> = (0..40).map(|_| backoff.failed(None, 0.99)).collect();
        assert_eq!(waits.iter().max(), Some(&ReadBackoff::CAP));
        assert_eq!(waits.last(), Some(&ReadBackoff::CAP));
        assert!(waits.iter().all(|wait| !wait.is_zero()));
    }

    #[test]
    fn jitter_adds_at_most_a_quarter_and_never_passes_the_cap() {
        assert_eq!(ReadBackoff::default().failed(None, 0.0), secs(10.0));
        assert_eq!(ReadBackoff::default().failed(None, 1.0), secs(12.5));
        // Out of range, it is held to the range.
        assert_eq!(ReadBackoff::default().failed(None, 7.0), secs(12.5));
        assert_eq!(ReadBackoff::default().failed(None, -1.0), secs(10.0));
        for step in 0..=100 {
            let random = f64::from(step) / 100.0;
            let mut backoff = ReadBackoff::default();
            for failure in 1..=8 {
                let base = (10.0 * 2f64.powi(failure - 1)).min(300.0);
                let wait = backoff.failed(None, random).as_secs_f64();
                assert!(wait >= base - 1e-9, "{failure} {random} {wait}");
                assert!(
                    wait <= (base * 1.25).min(300.0) + 1e-9,
                    "{failure} {random} {wait}"
                );
            }
        }
    }

    #[test]
    fn retry_after_wins_and_is_held_to_a_second() {
        let mut backoff = ReadBackoff::default();
        assert_eq!(backoff.failed(Some(secs(42.0)), 0.5), secs(42.0));
        assert_eq!(backoff.failed(Some(secs(0.2)), 0.0), secs(1.0));
        assert_eq!(backoff.failed(Some(Duration::ZERO), 0.0), secs(1.0));
        // Without one, the doubling goes on from the failures counted.
        assert_eq!(backoff.failed(None, 0.0), secs(80.0));
    }

    #[test]
    fn a_success_goes_back_to_the_normal_cadence() {
        let mut backoff = ReadBackoff::default();
        for _ in 0..3 {
            backoff.failed(None, 0.0);
        }
        backoff.succeeded();
        assert_eq!(backoff, ReadBackoff::default());
        assert_eq!(backoff.failed(None, 0.0), secs(10.0));
    }

    #[test]
    fn the_conversation_says_the_read_failed_and_when_it_is_tried_again() {
        assert_eq!(
            read_failure_text("Agent Relay could not complete the request.", secs(20.0)),
            "Could not read the conversation: Agent Relay could not complete the request. Trying again in 20 s."
        );
        assert_eq!(
            read_failure_text("Relaycast is busy", secs(12.1)),
            "Could not read the conversation: Relaycast is busy. Trying again in 13 s."
        );
        assert_eq!(
            recipient_failure_text("Relaycast is busy.", secs(20.0)),
            "Could not look up the recipient: Relaycast is busy. Trying again in 20 s."
        );
        assert_eq!(read_wait(secs(0.2)), "1 s");
        assert_eq!(read_wait(secs(59.5)), "1 min");
        assert_eq!(read_wait(secs(150.0)), "2 min 30 s");
        assert_eq!(read_wait(secs(300.0)), "5 min");
    }
}
