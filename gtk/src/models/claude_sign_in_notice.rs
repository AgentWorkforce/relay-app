//! The one notification posted when Claude Code cloud sessions stop for a
//! reason Claude Code's own sign-in fixes (`claude_code_sign_in`), with the
//! Mac's words and rule (`ClaudeSignInNotice` in RelayDesktopCore).

use super::claude_code_sign_in_offered;
use agent_relay_desktop_core::contract::account::ClaudeCloudHealth;

pub const CLAUDE_SIGN_IN_NOTICE_TITLE: &str = "Claude Code cloud sessions stopped";
pub const CLAUDE_SIGN_IN_NOTICE_BODY: &str = "Sign in to Claude Code to resume them.";
/// The notification's button; it runs the same sign-in as Settings'
/// *Sign in to Claude Code*.
pub const CLAUDE_SIGN_IN_NOTICE_ACTION: &str = "Sign in";
/// The preference that remembers this episode was notified, so a relaunch
/// while it lasts posts nothing.
pub const CLAUDE_SIGN_IN_NOTIFIED_KEY: &str = "claude_code_sign_in_notified";

/// What to do with the latest health: post or not, and the flag to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaudeSignInNoticeDecision {
    pub notify: bool,
    pub notified: bool,
}

/// One notification per episode: posted when the sign-in is first offered,
/// never again while it stays offered, and allowed again once the core
/// reports health without it. No health (not reported yet, signed out, cloud
/// sessions off) changes nothing.
pub fn claude_sign_in_notice(
    health: Option<&ClaudeCloudHealth>,
    notified: bool,
) -> ClaudeSignInNoticeDecision {
    match health {
        None => ClaudeSignInNoticeDecision {
            notify: false,
            notified,
        },
        Some(health) if claude_code_sign_in_offered(Some(health)) => ClaudeSignInNoticeDecision {
            notify: !notified,
            notified: true,
        },
        Some(_) => ClaudeSignInNoticeDecision {
            notify: false,
            notified: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_desktop_core::contract::account::ClaudeCloudHealthState;

    fn health(state: ClaudeCloudHealthState, sign_in: bool) -> ClaudeCloudHealth {
        ClaudeCloudHealth {
            state,
            reason: None,
            at_ms: None,
            last_success_ms: None,
            keychain_error_code: None,
            claude_code_sign_in: sign_in,
        }
    }

    fn offered() -> Option<ClaudeCloudHealth> {
        Some(health(ClaudeCloudHealthState::NeedsAttention, true))
    }

    fn working() -> Option<ClaudeCloudHealth> {
        Some(health(ClaudeCloudHealthState::Working, false))
    }

    /// Runs each status refresh's health through the rule, carrying the
    /// flag as the app keeps it; whether each one posted.
    fn posts(healths: &[Option<ClaudeCloudHealth>], notified: &mut bool) -> Vec<bool> {
        healths
            .iter()
            .map(|health| {
                let decision = claude_sign_in_notice(health.as_ref(), *notified);
                *notified = decision.notified;
                decision.notify
            })
            .collect()
    }

    #[test]
    fn the_first_offer_notifies_once() {
        let mut notified = false;
        assert_eq!(posts(&[working(), offered()], &mut notified), [false, true]);
        assert!(notified);
    }

    #[test]
    fn an_offer_that_continues_does_not_notify_again() {
        let mut notified = false;
        assert_eq!(
            posts(&[offered(), offered(), offered()], &mut notified),
            [true, false, false]
        );
    }

    #[test]
    fn a_relaunch_during_the_same_episode_does_not_notify() {
        let mut notified = true;
        assert_eq!(posts(&[None, offered()], &mut notified), [false, false]);
        assert!(notified);
    }

    #[test]
    fn a_new_episode_notifies_again() {
        let mut notified = false;
        assert_eq!(
            posts(&[offered(), working(), offered()], &mut notified),
            [true, false, true]
        );
        let mut notified = true;
        let unsaid = Some(health(ClaudeCloudHealthState::Blocked, false));
        assert_eq!(posts(&[unsaid, offered()], &mut notified), [false, true]);
    }

    #[test]
    fn no_health_neither_notifies_nor_ends_the_episode() {
        let mut notified = false;
        assert_eq!(posts(&[None, None], &mut notified), [false, false]);
        assert!(!notified);
        let mut notified = true;
        assert_eq!(posts(&[None], &mut notified), [false]);
        assert!(notified);
    }

    #[test]
    fn the_words_match_the_mac() {
        assert_eq!(
            CLAUDE_SIGN_IN_NOTICE_TITLE,
            "Claude Code cloud sessions stopped"
        );
        assert_eq!(
            CLAUDE_SIGN_IN_NOTICE_BODY,
            "Sign in to Claude Code to resume them."
        );
        assert_eq!(CLAUDE_SIGN_IN_NOTICE_ACTION, "Sign in");
    }
}
