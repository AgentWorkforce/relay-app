use agent_relay_desktop_core::session_locations::{
    planner::{resolve, Platform},
    SessionLocationReport, SessionOpenRequest,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    name: String,
    platform: String,
    request: SessionOpenRequest,
    report: SessionLocationReport,
    expected: Value,
}

#[test]
fn shared_session_navigation_scenarios() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../shared/session-navigation-cases.json"
    ))
    .unwrap();
    for case in cases {
        let platform = match case.platform.as_str() {
            "macos" => Platform::MacOS,
            "windows" => Platform::Windows,
            "linux" => Platform::Linux,
            value => panic!("Unknown test platform: {value}"),
        };
        let plan = resolve(&case.request, &case.report, platform);
        let actual = serde_json::to_value(&plan).unwrap();
        for key in ["action", "url", "command", "can_resume_default"] {
            if let Some(expected) = case.expected.get(key) {
                assert_eq!(&actual[key], expected, "{}: {key}", case.name);
            }
        }
        if let Some(count) = case.expected["choices"].as_u64() {
            assert_eq!(plan.choices.len(), count as usize, "{}", case.name);
        }
        if let Some(pid) = case.expected["pid"].as_i64() {
            assert_eq!(plan.location.as_ref().map(|l| l.pid), Some(pid as i32), "{}", case.name);
        }
        if case.expected["no_live_location"] == true {
            assert!(plan.location.is_none(), "{}: dead origin cannot be a live focus target", case.name);
        }
    }
}
