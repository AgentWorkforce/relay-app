//! `session-open plan|open|release|strings`: request JSON in, one answer
//! object out.
use crate::session_open::command::{answer, strings_json, Verbs, USAGE};
use crate::session_open::contract::{Action, OpenResult, Outcome, PlanView, Request, State};
use crate::session_open::view;
use crate::session_open::wording::Local;

fn args(verb: &str) -> Vec<String> {
    vec![verb.to_owned()]
}

fn stub_plan(request: &Request) -> PlanView {
    let mut view = view::refused(Local::SeveralLocations);
    view.title = format!("{}:{}", request.source, request.session_id);
    view
}

fn stub_open(request: &Request) -> OpenResult {
    view::refused_open(
        crate::session_open::wording::local(Local::AlreadyOpening),
        stub_plan(request),
    )
}

fn stub_release(request: &Request) -> PlanView {
    let mut view = stub_plan(request);
    view.title = format!("released {}", request.session_id);
    view
}

type Stub<T> = fn(&Request) -> T;

fn stubs() -> Verbs<Stub<PlanView>, Stub<OpenResult>, Stub<PlanView>> {
    Verbs {
        plan: stub_plan,
        open: stub_open,
        release: stub_release,
    }
}

fn real() -> Verbs<Stub<PlanView>, Stub<OpenResult>, Stub<PlanView>> {
    Verbs {
        plan: crate::session_open::plan,
        open: crate::session_open::open,
        release: crate::session_open::release_launch,
    }
}

const REQUEST: &str = r#"{"source":"codex","session_id":"abc","app_data_dir":"/tmp/app","default_terminal":"ghostty","use_default_terminal":true}"#;

#[test]
fn a_request_round_trips_to_one_json_line() {
    let out = answer(&args("plan"), REQUEST.as_bytes(), stubs()).unwrap();
    assert!(out.ends_with('\n') && !out.trim_end().contains('\n'));
    let view: PlanView = serde_json::from_str(&out).unwrap();
    assert_eq!(view.title, "codex:abc");
    let out = answer(&args("open"), REQUEST.as_bytes(), stubs()).unwrap();
    let result: OpenResult = serde_json::from_str(&out).unwrap();
    assert_eq!(result.outcome, Outcome::Refused);
    assert_eq!(result.reason_code.as_deref(), Some("already_opening"));
    assert_eq!(result.plan.title, "codex:abc");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    for key in ["outcome", "precision", "reason_code", "message", "plan"] {
        assert!(value.get(key).is_some(), "{key} in {value}");
    }
    let out = answer(&args("release"), REQUEST.as_bytes(), stubs()).unwrap();
    let view: PlanView = serde_json::from_str(&out).unwrap();
    assert_eq!(view.title, "released abc");
}

#[test]
fn strings_needs_no_request_and_prints_the_table() {
    for input in [&b""[..], b"ignored"] {
        let out = answer(&args("strings"), input, stubs()).unwrap();
        assert_eq!(out, strings_json().unwrap());
        let table: crate::session_open::contract::Strings = serde_json::from_str(&out).unwrap();
        assert_eq!(table, crate::session_open::wording::strings());
    }
}

#[test]
fn malformed_input_is_exit_two_without_echoing_it() {
    let secret = r#"{"source":"codex","session_id":"x","app_data_dir":"/a","password":"hunter2"}"#;
    for input in [&b"not json"[..], b"{}", secret.as_bytes(), b""] {
        let (code, line) = answer(&args("plan"), input, stubs()).unwrap_err();
        assert_eq!(code, 2);
        assert!(line.starts_with("session-open:"), "{line}");
        assert!(!line.contains("hunter2"));
    }
    for verb in [vec![], args("resume"), vec!["plan".into(), "extra".into()]] {
        let (code, line) = answer(&verb, REQUEST.as_bytes(), stubs()).unwrap_err();
        assert_eq!((code, line.as_str()), (2, USAGE));
    }
}

#[test]
fn the_real_plan_refuses_an_unknown_provider_without_a_look() {
    let app = tempfile::tempdir().unwrap();
    let input = serde_json::json!({
        "source": "cursor",
        "session_id": "made-up",
        "app_data_dir": app.path(),
    })
    .to_string();
    let out = answer(&args("plan"), input.as_bytes(), real()).unwrap();
    let view: PlanView = serde_json::from_str(&out).unwrap();
    assert_eq!(view.state, State::Unsupported);
    assert_eq!(view.action, Action::Unavailable);
    assert_eq!(view.reason_code.as_deref(), Some("unsupported_source"));
}

#[test]
fn the_real_commands_refuse_a_relative_app_directory() {
    let input = r#"{"source":"codex","session_id":"x","app_data_dir":"relative"}"#;
    for verb in ["plan", "open", "release"] {
        let out = answer(&args(verb), input.as_bytes(), real()).unwrap();
        assert!(out.contains("\"invalid_request\""), "{out}");
    }
}
