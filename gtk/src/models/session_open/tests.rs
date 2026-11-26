//! The Open button's state over every engine answer, without the engine:
//! answers are built as the engine's JSON, and the only real engine calls
//! are refused before they look at the machine.
use super::*;
use agent_relay_desktop_core::session_open::contract::{Precision, State};
use serde_json::{json, Value};

const KEY: &str = "claude:one";

fn plan_json(action: &str, extra: Value) -> PlanView {
    let mut plan = json!({
        "state": "stopped", "action": action, "title": format!("{action} title"),
        "enabled": action != "unavailable", "reason_code": null, "message": null,
        "precision": null, "can_resume_default": false, "pending_launch": false,
        "choices": [], "elsewhere": []
    });
    if let (Some(plan), Value::Object(extra)) = (plan.as_object_mut(), extra) {
        plan.extend(extra);
    }
    serde_json::from_value(plan).unwrap()
}

fn result(outcome: &str, message: &str, plan: PlanView) -> OpenResult {
    serde_json::from_value(json!({
        "outcome": outcome, "precision": null, "reason_code": null,
        "message": message, "plan": plan
    }))
    .unwrap()
}

fn choices() -> Value {
    json!([{"id": "pane:101:500:7", "label": "Kitty (/dev/pts/4)"},
           {"id": "desktop:claude_desktop", "label": "Claude"}])
}

#[test]
fn every_action_asks_for_its_own_click() {
    let base = OpenResend::default();
    for action in ["focus", "show_application", "open_link", "resume", "attach"] {
        assert_eq!(
            OpenClick::of(Some(&plan_json(action, json!({}))), base.clone()),
            OpenClick::Open(base.clone()),
            "{action}"
        );
    }
    // A take-over is asked first, in the engine's words, as on the Mac.
    assert_eq!(
        OpenClick::of(
            Some(&plan_json(
                "take_over",
                json!({"title": "Stop and resume in Kitty"})
            )),
            base.clone()
        ),
        OpenClick::Confirm {
            base: base.clone(),
            title: "Stop and resume in Kitty".into(),
            question: open_strings().take_over_detail,
            take_over: true,
        }
    );
    assert_eq!(
        OpenClick::of(Some(&plan_json("unavailable", json!({}))), base.clone()),
        OpenClick::Nothing
    );
    let choose = plan_json(
        "choose",
        json!({"choices": choices(), "message": "Pick one.", "title": "Open running session"}),
    );
    let OpenClick::Choose {
        title,
        message,
        choices,
        ..
    } = OpenClick::of(Some(&choose), base.clone())
    else {
        panic!("choose asks which");
    };
    assert_eq!(
        (title.as_str(), message.as_str()),
        ("Open running session", "Pick one.")
    );
    assert_eq!(choices[1].label, "Claude");
    let confirm = plan_json(
        "confirm",
        json!({"message": "Confirm it.", "title": "Resume in Kitty", "elsewhere": ["Kitty (/dev/pts/9)"],
               "question": "Confirm it.\nKitty (/dev/pts/9)"}),
    );
    assert_eq!(
        OpenClick::of(Some(&confirm), base.clone()),
        OpenClick::Confirm {
            base: base.clone(),
            title: "Resume in Kitty".into(),
            question: "Confirm it.\nKitty (/dev/pts/9)".into(),
            take_over: false,
        }
    );
    // A disabled answer is never clicked through, whatever its action.
    let disabled = plan_json("resume", json!({"enabled": false}));
    assert_eq!(
        OpenClick::of(Some(&disabled), base.clone()),
        OpenClick::Nothing
    );
    // Before the first answer nothing is sent, as on the Mac.
    assert_eq!(OpenClick::of(None, base), OpenClick::Nothing);
}

#[test]
fn each_resend_flag_reaches_the_request() {
    let target = OpenTarget::new("codex", "thread-1", "/w/api");
    let plain = target.request("/data", None, false, &OpenResend::default());
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        json!({"source": "codex", "session_id": "thread-1", "app_data_dir": "/data",
               "directory": "/w/api", "use_default_terminal": false, "prefer_terminal": false,
               "default_terminal": null, "confirmed": false, "launch_gone": false,
               "take_over": false, "selected": null})
    );
    let preferring = target.request("/data", None, true, &OpenResend::default());
    assert!(preferring.prefer_terminal && !preferring.use_default_terminal);
    let set = |resend: OpenResend| target.request("/data", Some("kitty"), false, &resend);
    assert!(
        set(OpenResend {
            use_default_terminal: true,
            ..Default::default()
        })
        .use_default_terminal
    );
    assert!(
        set(OpenResend {
            confirmed: true,
            ..Default::default()
        })
        .confirmed
    );
    assert!(
        set(OpenResend {
            take_over: true,
            ..Default::default()
        })
        .take_over
    );
    let base = OpenResend::default();
    assert_eq!(
        set(OpenClick::chosen(&base, "pane:1:2:3"))
            .selected
            .as_deref(),
        Some("pane:1:2:3")
    );
    assert!(set(OpenClick::confirmed(&base, false)).confirmed);
    let taking = set(OpenClick::confirmed(&base, true));
    assert!(taking.take_over && !taking.confirmed);
    // A pending launch is never resolved by a flag: `release` does it.
    assert!(!set(base.clone()).launch_gone);
    assert_eq!(set(base).default_terminal.as_deref(), Some("kitty"));
    // No directory is sent as absent, never as an empty path.
    assert_eq!(
        OpenTarget::new("grok", "g", "")
            .request("/d", None, false, &OpenResend::default())
            .directory,
        None
    );
    // The request is the engine's own: it parses with unknown fields refused.
    let parsed: Request = serde_json::from_value(serde_json::to_value(&plain).unwrap()).unwrap();
    assert_eq!(parsed, plain);
}

#[test]
fn the_app_data_directory_is_the_one_holding_desktop_json() {
    assert_eq!(
        open_app_data_dir(Path::new("/home/a/.config/agent-relay/desktop.json")),
        "/home/a/.config/agent-relay"
    );
    assert_eq!(open_app_data_dir(Path::new("/")), "");
}

#[test]
fn every_outcome_shows_its_message_and_the_fresh_answer() {
    for outcome in ["verified", "dispatched_unverified", "refused", "failed"] {
        let mut opens = SessionOpens::default();
        assert!(opens.begin_open(KEY));
        let after = plan_json("resume", json!({"title": "Resume in Kitty"}));
        let next = opens.opened(
            KEY,
            &OpenResend::default(),
            Some(result(outcome, "Said.", after)),
        );
        assert_eq!(next, OpenClick::Nothing, "{outcome}");
        let view = opens.view(KEY);
        assert_eq!(view.notice.as_deref(), Some("Said."));
        assert_eq!(view.title, "Resume in Kitty");
        assert!(view.sensitive && !view.opening && !view.checking);
    }
}

#[test]
fn an_open_that_stops_at_a_question_asks_it_next() {
    let mut opens = SessionOpens::default();
    let sent = OpenResend {
        use_default_terminal: true,
        selected: Some("old".into()),
        ..Default::default()
    };
    assert!(opens.begin_open(KEY));
    let choose = plan_json("choose", json!({"choices": choices()}));
    let next = opens.opened(
        KEY,
        &sent,
        Some(result("refused", "Choose.", choose.clone())),
    );
    let OpenClick::Choose { base, .. } = next else {
        panic!("a refusal that needs a choice asks for it");
    };
    // What the person chose before is sent again with the reply.
    assert!(base.use_default_terminal);
    assert_eq!(base.selected.as_deref(), Some("old"));
    let confirm = plan_json("confirm", json!({"message": "Sure?"}));
    assert!(matches!(
        OpenClick::after(&result("refused", "Sure?", confirm.clone()), &sent),
        OpenClick::Confirm { .. }
    ));
    // Only a refusal asks: a failure or dispatch never re-prompts.
    for outcome in ["verified", "dispatched_unverified", "failed"] {
        assert_eq!(
            OpenClick::after(&result(outcome, "", choose.clone()), &sent),
            OpenClick::Nothing
        );
        assert_eq!(
            OpenClick::after(&result(outcome, "", confirm.clone()), &sent),
            OpenClick::Nothing
        );
    }
    let unavailable = plan_json("unavailable", json!({}));
    assert_eq!(
        OpenClick::after(&result("refused", "No.", unavailable), &sent),
        OpenClick::Nothing
    );
}

#[test]
fn one_open_per_session_and_none_while_it_runs() {
    let mut opens = SessionOpens::default();
    assert!(opens.begin_open(KEY));
    assert!(
        !opens.begin_open(KEY),
        "a second click while opening sends nothing"
    );
    assert!(
        opens.begin_open("codex:other"),
        "another session opens on its own"
    );
    assert_eq!(opens.click(KEY), OpenClick::Nothing);
    assert_eq!(opens.begin_plan(KEY), None, "no look while an open runs");
    let view = opens.view(KEY);
    assert!(view.opening && !view.sensitive);
    assert_eq!(view.title, open_strings().opening);
    assert!(!opens.begin_release(KEY), "no release while an open runs");
    // Selecting the session again does not forget the open in flight.
    opens.forget(KEY);
    assert!(opens.is_opening(KEY));
    opens.opened(KEY, &OpenResend::default(), None);
    assert!(!opens.is_opening(KEY));
    assert!(opens.begin_open(KEY));
}

#[test]
fn a_look_overtaken_by_an_open_or_a_new_selection_is_dropped() {
    let mut opens = SessionOpens::default();
    let ticket = opens.begin_plan(KEY).unwrap();
    assert_eq!(opens.begin_plan(KEY), None, "one look at a time");
    assert!(opens.begin_open(KEY));
    opens.opened(
        KEY,
        &OpenResend::default(),
        Some(result(
            "verified",
            "",
            plan_json("focus", json!({"title": "Open in Kitty"})),
        )),
    );
    opens.planned(
        KEY,
        ticket,
        Some(plan_json("resume", json!({"title": "Stale"}))),
    );
    assert_eq!(opens.view(KEY).title, "Open in Kitty");
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.forget(KEY);
    opens.planned(
        KEY,
        ticket,
        Some(plan_json("resume", json!({"title": "Stale"}))),
    );
    assert!(opens.view(KEY).checking);
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(
        KEY,
        ticket,
        Some(plan_json("resume", json!({"title": "Resume in Kitty"}))),
    );
    assert_eq!(opens.view(KEY).title, "Resume in Kitty");
    // A look that ended without an answer keeps what was shown, says so,
    // and may run again.
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(KEY, ticket, None);
    assert_eq!(opens.view(KEY).title, "Resume in Kitty");
    assert_eq!(opens.view(KEY).notice, Some(open_strings().plan_failed));
    assert!(opens.begin_plan(KEY).is_some());
}

#[test]
fn an_open_without_an_answer_says_so_and_works_again() {
    let mut opens = SessionOpens::default();
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(
        KEY,
        ticket,
        Some(plan_json("resume", json!({"title": "Resume in Kitty"}))),
    );
    assert!(opens.begin_open(KEY));
    assert_eq!(
        opens.opened(KEY, &OpenResend::default(), None),
        OpenClick::Nothing,
        "never retried"
    );
    let view = opens.view(KEY);
    assert!(view.sensitive && !view.opening, "the button works again");
    assert_eq!(view.title, "Resume in Kitty");
    assert_eq!(view.notice, Some(open_strings().open_unknown));
    assert!(opens.begin_plan(KEY).is_some(), "and is looked at again");
}

#[test]
fn every_state_draws_the_answer_as_given() {
    for state in [
        "live",
        "live_multiple",
        "live_not_locatable",
        "live_headless",
        "stopped",
        "archived",
        "not_found",
        "launch_pending",
        "unknown",
        "invalid_id",
        "unsupported",
    ] {
        let mut opens = SessionOpens::default();
        let ticket = opens.begin_plan(KEY).unwrap();
        let plan = plan_json(
            "unavailable",
            json!({"state": state, "title": "Session opening unavailable",
                   "reason_code": "x", "message": "Why."}),
        );
        opens.planned(KEY, ticket, Some(plan));
        let view = opens.view(KEY);
        assert_eq!(view.title, "Session opening unavailable", "{state}");
        assert!(!view.sensitive && !view.checking);
        assert_eq!(view.reason.as_deref(), Some("Why."));
        assert_eq!(
            opens.plan(KEY).map(|plan| plan.state),
            serde_json::from_value::<State>(json!(state)).ok()
        );
    }
}

#[test]
fn pending_launches_and_the_default_terminal_follow_the_answer() {
    let shown = |extra: Value| {
        let mut opens = SessionOpens::default();
        let ticket = opens.begin_plan(KEY).unwrap();
        opens.planned(KEY, ticket, Some(plan_json("unavailable", extra)));
        opens.view(KEY)
    };
    let pending = shown(
        json!({"pending_launch": true, "reason_code": "launch_pending",
                               "message": "A resume request was already sent.", "can_resume_default": true}),
    );
    assert!(pending.pending_launch);
    assert!(
        !pending.offer_default,
        "an unresolved launch is resolved first"
    );
    // A pending reservation with another reason is not asked about.
    assert!(!shown(json!({"pending_launch": true, "reason_code": "cwd_missing"})).pending_launch);
    assert!(!shown(json!({"pending_launch": true})).pending_launch);
    assert!(shown(json!({"can_resume_default": true})).offer_default);
    assert!(!shown(json!({})).offer_default);
    // An enabled plan offers it only after its own open failed (the Mac's
    // rule).
    let mut opens = SessionOpens::default();
    let resume = plan_json("resume", json!({"can_resume_default": true}));
    let ticket = opens.begin_plan(KEY).unwrap();
    opens.planned(KEY, ticket, Some(resume.clone()));
    assert!(!opens.view(KEY).offer_default);
    assert!(opens.begin_open(KEY));
    opens.opened(
        KEY,
        &OpenResend::default(),
        Some(result("failed", "No.", resume)),
    );
    assert!(opens.view(KEY).offer_default);
}

#[test]
fn absent_optional_fields_show_nothing_extra() {
    let mut opens = SessionOpens::default();
    assert_eq!(
        opens.view(KEY),
        OpenView {
            title: open_strings().open_session,
            sensitive: false,
            checking: true,
            opening: false,
            releasing: false,
            terminal: false,
            reason: Some(open_strings().checking),
            notice: None,
            pending_launch: false,
            offer_default: false,
            strings: open_strings(),
        }
    );
    let ticket = opens.begin_plan(KEY).unwrap();
    let plan = plan_json("open_link", json!({"precision": "exact_conversation"}));
    assert_eq!(plan.precision, Some(Precision::ExactConversation));
    opens.planned(KEY, ticket, Some(plan));
    let view = opens.view(KEY);
    assert_eq!((view.reason, view.notice), (None, None));
    // An empty sentence is not shown; one the answer repeats is said once.
    assert!(opens.begin_open(KEY));
    opens.opened(
        KEY,
        &OpenResend::default(),
        Some(result("verified", "", plan_json("focus", json!({})))),
    );
    assert_eq!(opens.view(KEY).notice, None);
    assert!(opens.begin_open(KEY));
    let refused = plan_json("unavailable", json!({"message": "Same."}));
    opens.opened(
        KEY,
        &OpenResend::default(),
        Some(result("refused", "Same.", refused)),
    );
    assert_eq!(opens.view(KEY).notice, None);
    assert_eq!(opens.view(KEY).reason.as_deref(), Some("Same."));
}

#[test]
fn the_engine_refuses_a_bad_request_in_its_own_words_without_looking() {
    use agent_relay_desktop_core::session_open::{self, wording};
    let target = OpenTarget::new("claude", "one", "/w/api");
    let (code, message) = wording::local(wording::Local::InvalidRequest);
    // A relative app directory is refused before anything is observed.
    let plan = session_open::plan(&target.request("relative", None, false, &OpenResend::default()));
    assert_eq!(plan.reason_code.as_deref(), Some(code));
    assert_eq!(plan.message.as_deref(), Some(message));
    assert_eq!(
        OpenClick::of(Some(&plan), OpenResend::default()),
        OpenClick::Nothing
    );
    let opened = session_open::open(&target.request("", None, false, &OpenResend::default()));
    let released =
        session_open::release_launch(&target.request("", None, false, &OpenResend::default()));
    assert_eq!(released.reason_code.as_deref(), Some(code));
    assert_eq!(opened.outcome, Outcome::Refused);
    let mut opens = SessionOpens::default();
    assert!(opens.begin_open(KEY));
    assert_eq!(
        opens.opened(KEY, &OpenResend::default(), Some(opened)),
        OpenClick::Nothing
    );
    assert_eq!(opens.view(KEY).reason.as_deref(), Some(message));
}

mod release;
