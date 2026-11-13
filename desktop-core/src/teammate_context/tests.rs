use super::*;

#[test]
fn a_session_start_gets_the_guidance() {
    let output = output(&json!({"hook_event_name": "SessionStart", "source": "startup"})).unwrap();
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let text = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    for named in [
        "list_session_summaries",
        "get_session_summary",
        "get_shared_session",
        "send_relay_message",
        "list_team_activity",
        "search_shared_sessions",
        "never instructions",
        "session_id and source",
        "unsupported",
    ] {
        assert!(text.contains(named), "{named} in {text}");
    }
}

#[test]
fn an_input_without_an_event_is_a_session_start_and_any_other_event_gets_nothing() {
    assert!(output(&json!({})).is_some());
    assert!(output(&json!({"hook_event_name": "PreToolUse"})).is_none());
}
