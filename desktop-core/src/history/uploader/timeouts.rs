//! How long each probe command may run.
use std::time::Duration;
/// `installs` is a pure read, and launch must never wait on it for long.
pub const INSTALLS: Duration = Duration::from_secs(15);
pub const STATUS: Duration = Duration::from_secs(15);
/// The probe bounds a collector replacement at ten minutes; this leaves
/// it time to withdraw a timed-out stop request and say so.
pub const START: Duration = Duration::from_secs(10 * 60 + 30);
/// A reset deletes the local store and asks Cloud to forget the sessions;
/// it holds every reader meanwhile, so a hung helper must not wedge them.
/// Once the helper has saved its plan (`session-reset-pending.json`), a
/// retry finishes it, as Cloud's side is idempotent by request id; one
/// killed before the plan exists starts over on retry.
pub const RESET: Duration = Duration::from_secs(10 * 60 + 30);
pub const PAUSE: Duration = Duration::from_secs(30);
/// Compaction is bounded by rows, not time; committed pages survive a
/// kill, so another attempt continues it.
pub const COMPACT: Duration = Duration::from_secs(180);
pub const PREVIEW: Duration = Duration::from_secs(3);
pub const PREVIEW_REFRESH: Duration = Duration::from_secs(15);
pub const SESSIONS: Duration = Duration::from_secs(30);
pub const DISCONNECT: Duration = Duration::from_secs(60);

/// The sentence for a command that ran past its deadline. A reset is told
/// apart from the other `sessions` commands: it may have deleted some
/// sessions already.
pub fn message(args: &[String]) -> &'static str {
    match args {
        [group, command, ..] if group == "sessions" && command == "reset" => {
            "Deleting your sessions took too long. Some may already be gone; run the reset again to finish it."
        }
        _ => super::timeout_message(args.first().map(String::as_str).unwrap_or_default()),
    }
}
