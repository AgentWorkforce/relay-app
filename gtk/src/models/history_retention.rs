//! The session history setting's wording (`history_retention_days`), shared
//! with the Mac (`HistoryRetentionCopy`): how long this computer keeps
//! local session history, and the note that history not yet uploaded is
//! kept longer.

use agent_relay_node::relay::contract::agents::history_retention::{CHOICES, DEFAULT_DAYS, FOREVER};

pub const HISTORY_RETENTION_SECTION: &str = "Session history";
pub const HISTORY_RETENTION_TITLE: &str = "Keep session history";
pub const HISTORY_RETENTION_DETAIL: &str = "How long this computer keeps the sessions Agent Relay finds. Older sessions are removed from this computer; copies already uploaded to your workspace are not affected.";

/// The choices Settings offers, in order, with their labels.
pub fn history_retention_choices() -> Vec<(u32, String)> {
    CHOICES
        .into_iter()
        .map(|days| (days, history_retention_label(days)))
        .collect()
}

/// `7 days`, `30 days (default)`, `Forever`.
pub fn history_retention_label(days: u32) -> String {
    match days {
        FOREVER => "Forever".to_string(),
        DEFAULT_DAYS => format!("{days} days (default)"),
        1 => "1 day".to_string(),
        days => format!("{days} days"),
    }
}

/// Said under a limit when history can outlast it: sessions are never
/// evicted before they upload, which takes longest while signed out or
/// paused, and a short limit is the likeliest to be reached first.
pub const HISTORY_RETENTION_NOTE: &str = "Unuploaded sessions are kept until delivery succeeds, so history may be retained longer than your selected limit.";

/// [`HISTORY_RETENTION_NOTE`] for a choice shorter than the default, or any
/// limit while uploads are held (signed out or paused).
pub fn history_retention_warning(days: u32, uploads_held: bool) -> Option<String> {
    let shows = days != FOREVER && (uploads_held || days < DEFAULT_DAYS);
    shows.then(|| HISTORY_RETENTION_NOTE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_read_as_the_mac_reads_them() {
        let labels: Vec<String> = history_retention_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert_eq!(
            labels,
            ["7 days", "30 days (default)", "90 days", "Forever"]
        );
    }

    #[test]
    fn a_short_choice_or_held_uploads_say_history_is_kept_until_it_uploads() {
        assert_eq!(
            history_retention_warning(7, false).as_deref(),
            Some("Unuploaded sessions are kept until delivery succeeds, so history may be retained longer than your selected limit.")
        );
        for days in [30, 90, FOREVER] {
            assert_eq!(history_retention_warning(days, false), None, "{days}");
        }
        for days in [7, 30, 90] {
            assert_eq!(
                history_retention_warning(days, true).as_deref(),
                Some(HISTORY_RETENTION_NOTE),
                "{days}"
            );
        }
        assert_eq!(history_retention_warning(FOREVER, true), None);
    }
}
