use super::*;

fn preparing() -> ToolIntegration {
    serde_json::from_value(serde_json::json!({"id":"codex","name":"Codex","detected":true,"supported":true,"selected":true,"state":"preparing","status_label":"Preparing sessions","detail":"You’re signed in. Agent Relay is preparing your local sessions.","can_install":false,"can_repair":false,"can_uninstall":true})).unwrap()
}

#[test]
fn preparation_reports_waiting_without_sign_in_or_setup_actions() {
    let tool = preparing();
    for health in [None, Some("ready")] {
        let shown = Presentation::new(&tool, health);
        assert_eq!(shown.status, "Preparing sessions");
        assert_eq!(shown.summary.as_deref(), Some(tool.detail.as_str()));
        assert_eq!(shown.action, None);
        assert_eq!(shown.action_label(), None);
    }
}

#[test]
fn preparation_does_not_hide_a_real_account_problem() {
    let tool = preparing();
    let shown = Presentation::new(&tool, Some("sign_in_required"));
    assert_eq!(shown.status, "Sign in required");
    assert_eq!(shown.action, Some(NextAction::Account));
    assert_eq!(
        Presentation::new(&tool, Some("offline")).action,
        Some(NextAction::Check)
    );
}
