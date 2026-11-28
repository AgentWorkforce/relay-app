//! The Claude Code sign-in notification: one per episode, kept across a
//! relaunch, with the Mac's words and a button that runs Settings' sign-in.

use super::*;
use agent_relay_app::models::CLAUDE_SIGN_IN_NOTIFIED_KEY;
use agent_relay_desktop_core::contract::method::UploaderStatus;

pub const TESTS: &[(&str, Test)] = &[
    (
        "claude_code_sign_in_is_notified_once_per_episode",
        claude_code_sign_in_is_notified_once_per_episode,
    ),
    (
        "a_relaunch_during_a_claude_code_sign_in_episode_does_not_notify",
        a_relaunch_during_a_claude_code_sign_in_episode_does_not_notify,
    ),
];

fn health(sign_in: bool) -> Value {
    if sign_in {
        json!({"state": "needs_attention", "reason": "Claude Code is not signed in", "claude_code_sign_in": true})
    } else {
        json!({"state": "working", "last_success_ms": 1_800_000_000_000_i64})
    }
}

/// Answers `uploader.status` with `health` and has the app read it again.
fn report(case: &Case, health: Value) {
    let mut status = uploader();
    status["claude_cloud"] = health;
    case.answer("uploader.status", check::<UploaderStatus>(status));
    let before = case.called("uploader.status").len();
    case.emit("invalidated", json!({"what": "uploader"}));
    case.pump(|case| case.called("uploader.status").len() > before);
    case.settle();
}

fn notified_flag(case: &Case) -> Option<bool> {
    Preferences::load(case.prefs_path.clone())
        .get(CLAUDE_SIGN_IN_NOTIFIED_KEY)
        .and_then(Value::as_bool)
}

fn claude_code_sign_in_is_notified_once_per_episode(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.sign_in();
    report(&case, health(true));
    case.pump(|case| !case.desktop.notices.borrow().is_empty());
    let notice = case.desktop.notices.borrow()[0].clone();
    assert_eq!(notice.title, "Claude Code cloud sessions stopped");
    assert_eq!(notice.body, "Sign in to Claude Code to resume them.");
    assert_eq!(notice.default_action, "claude-code-settings");
    assert_eq!(
        notice.button,
        Some(("Sign in".to_string(), "claude-code-sign-in".to_string()))
    );
    assert_eq!(notified_flag(&case), Some(true));

    // Every later refresh while the sign-in is still offered posts nothing.
    report(&case, health(true));
    assert_eq!(case.desktop.notices.borrow().len(), 1);

    // The episode ends; the next one notifies again.
    report(&case, health(false));
    assert_eq!(notified_flag(&case), Some(false));
    report(&case, health(true));
    case.pump(|case| case.desktop.notices.borrow().len() == 2);

    // Its button runs the same sign-in as Settings' button.
    case.app.claude_sign_in_notice_answered(true);
    case.pump(|case| !case.called("claude_cloud.sign_in").is_empty());
}

fn a_relaunch_during_a_claude_code_sign_in_episode_does_not_notify(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::seeded(
        runtime,
        |_| {},
        Arc::new(FakeService::default()),
        |prefs| {
            prefs
                .set(CLAUDE_SIGN_IN_NOTIFIED_KEY, Value::Bool(true))
                .unwrap();
        },
    );
    case.sign_in();
    report(&case, health(true));
    report(&case, health(true));
    assert!(case.desktop.notices.borrow().is_empty());
    assert_eq!(notified_flag(&case), Some(true));
}
