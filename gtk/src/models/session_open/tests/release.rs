//! Releasing a pending launch, the terminal icon, and the engine's fixed
//! words, as on the Mac (`SessionOpenModelTests`, `SessionOpenStringsTests`).
use super::*;

#[test]
fn the_fixed_words_are_the_checked_in_table() {
    let file: Strings = serde_json::from_str(include_str!(
        "../../../../../shared/session-open-strings.json"
    ))
    .unwrap();
    assert_eq!(file, open_strings());
}

#[test]
fn a_release_only_shows_its_answer_and_opens_nothing() {
    let mut opens = SessionOpens::default();
    let pending = plan_json(
        "unavailable",
        json!({"pending_launch": true, "reason_code": "launch_pending", "message": "Sent."}),
    );
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(KEY, ticket, Some(pending));
    assert!(opens.view(KEY).pending_launch);
    assert!(opens.begin_release(KEY));
    assert!(!opens.begin_release(KEY), "one release at a time");
    assert!(!opens.begin_open(KEY), "no open beside a release");
    assert_eq!(opens.begin_plan(KEY), None);
    assert_eq!(opens.click(KEY), OpenClick::Nothing);
    let view = opens.view(KEY);
    assert!(view.releasing && !view.sensitive);
    let resumable = plan_json(
        "resume",
        json!({"title": "Resume in Kitty", "destination": "terminal"}),
    );
    opens.released(KEY, Some(resumable));
    let view = opens.view(KEY);
    assert!(!view.releasing && !view.pending_launch && view.sensitive);
    assert_eq!(view.title, "Resume in Kitty");
    // What a click would now do is an open the person still has to make.
    assert_eq!(opens.click(KEY), OpenClick::Open(OpenResend::default()));
}

#[test]
fn a_release_without_an_answer_says_so() {
    let mut opens = SessionOpens::default();
    assert!(opens.begin_release(KEY));
    opens.released(KEY, None);
    let view = opens.view(KEY);
    assert!(!view.releasing);
    assert_eq!(view.notice, Some(open_strings().plan_failed));
    assert!(
        opens.begin_plan(KEY).is_some(),
        "and may be looked at again"
    );
}

#[test]
fn the_terminal_icon_follows_the_destination() {
    for (destination, terminal) in [("terminal", true), ("desktop_app", false), ("none", false)] {
        let mut opens = SessionOpens::default();
        let ticket = opens.begin_plan(KEY).unwrap();
        opens.planned(
            KEY,
            ticket,
            Some(plan_json("focus", json!({"destination": destination}))),
        );
        assert_eq!(opens.view(KEY).terminal, terminal, "{destination}");
    }
    assert!(!SessionOpens::default().view(KEY).terminal);
}

#[test]
fn the_answers_words_win_over_the_fallback() {
    let mut words = serde_json::to_value(open_strings()).unwrap();
    words["open_unknown"] = json!("Unknown (the answer's words).");
    let mut opens = SessionOpens::default();
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(
        KEY,
        ticket,
        Some(plan_json("resume", json!({"strings": words}))),
    );
    assert!(opens.begin_open(KEY));
    opens.opened(KEY, &OpenResend::default(), None);
    assert_eq!(
        opens.view(KEY).notice.as_deref(),
        Some("Unknown (the answer's words).")
    );
}
