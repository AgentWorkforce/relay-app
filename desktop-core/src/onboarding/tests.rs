use super::*;
use std::collections::VecDeque;

fn ready() -> Value {
    json!({"state":"signed_in", "site_url":"https://agentrelay.com", "workspace_id":"workspace-a", "auth_health":"ready"})
}
fn pending_account() -> Value {
    json!({"state":"pending_approval", "phase":"waiting_for_device_approval", "site_url":"https://agentrelay.com", "verification_url":"https://agentrelay.com/device", "verification_url_complete":"https://agentrelay.com/device?code=ABCD-EFGH", "user_code":"ABCD-EFGH", "interval":1, "device_code":"SECRET_DEVICE_CODE", "access_token":"SECRET_ACCESS_TOKEN"})
}
fn options(args: &[&str]) -> Options {
    Options::try_parse_from(std::iter::once("setup").chain(args.iter().copied())).unwrap()
}
struct Fake {
    account: Value,
    statuses: VecDeque<Value>,
    sign_in: Value,
    tools: Value,
    uploader: Value,
    settings: Value,
    workspace: Value,
    calls: Vec<(&'static str, Value)>,
    elapsed: Duration,
    failed: Option<&'static str>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            account: ready(),
            statuses: VecDeque::new(),
            sign_in: pending_account(),
            tools: json!({"onboarding_complete":true,"account_health":"ready","tools":[
                {"id":"codex","detected":true,"supported":true,"selected":true,"state":"configured"},
                {"id":"claude","detected":true,"supported":true,"selected":false,"state":"not_installed"},
                {"id":"cursor","detected":false,"supported":false,"selected":false,"state":"external_setup"},
                {"id":"capy","detected":false,"supported":false,"selected":false,"state":"external_setup"}
            ]}),
            uploader: json!({"sharing_mode":"all","running":true,"paused":false,"stage":"watching","directory":"PRIVATE_TRANSCRIPT_PATH","sessions":{"private":"SECRET_HISTORY"}}),
            settings: json!({"cursor_direct_delivery":false}),
            workspace: json!({"verification":"verified"}),
            calls: Vec::new(),
            elapsed: Duration::ZERO,
            failed: None,
        }
    }
}
impl Backend for Fake {
    fn call(&mut self, method: &'static str, params: Value) -> Result<Value, ()> {
        self.calls.push((method, params.clone()));
        if self.failed == Some(method) {
            return Err(());
        }
        match method {
            "account.status" => {
                if let Some(report) = self.statuses.pop_front() {
                    self.account = report;
                }
                Ok(self.account.clone())
            }
            "account.sign_in" => {
                self.account = self.sign_in.clone();
                Ok(self.account.clone())
            }
            "tools.status" | "tools.check" => Ok(self.tools.clone()),
            "tools.select" => {
                for tool in self.tools["tools"].as_array_mut().unwrap() {
                    tool["selected"] =
                        json!(params["tool_ids"].as_array().unwrap().contains(&tool["id"]));
                    if tool["selected"] == true {
                        tool["state"] = json!("configured");
                    }
                }
                self.tools["onboarding_complete"] = json!(true);
                Ok(self.tools.clone())
            }
            "uploader.status" => Ok(self.uploader.clone()),
            "uploader.sharing.set" => {
                self.uploader["sharing_mode"] = params["mode"].clone();
                Ok(json!({"sharing_mode":params["mode"]}))
            }
            "settings.get" => Ok(self.settings.clone()),
            "settings.set" => {
                for (key, value) in params.as_object().unwrap() {
                    self.settings[key] = value.clone();
                }
                Ok(self.settings.clone())
            }
            "workspace.verify" => Ok(self.workspace.clone()),
            "uploader.start" | "account.select_workspace" => Ok(json!({})),
            other => panic!("unexpected method {other}"),
        }
    }
    fn pause(&mut self, duration: Duration) {
        self.elapsed += duration;
    }
    fn elapsed(&self) -> Duration {
        self.elapsed
    }
    fn start_service(&mut self) -> bool {
        false
    }
}
#[derive(Default)]
struct Console {
    interactive: bool,
    answers: VecDeque<String>,
    events: Vec<Value>,
}
impl Interface for Console {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn event(&mut self, event: &'static str, mut fields: Value) {
        fields["event"] = json!(event);
        self.events.push(fields);
    }
    fn prompt(&mut self, _: &str) -> Result<String, ()> {
        assert!(self.interactive, "noninteractive setup prompted");
        self.answers.pop_front().ok_or(())
    }
}
impl Console {
    fn result(&self) -> &Value {
        self.events
            .iter()
            .rev()
            .find(|event| event["event"] == "result")
            .unwrap()
    }
    fn assert_private(&self) {
        let output = serde_json::to_string(&self.events).unwrap();
        for secret in [
            "SECRET_DEVICE_CODE",
            "SECRET_ACCESS_TOKEN",
            "PRIVATE_TRANSCRIPT_PATH",
            "SECRET_HISTORY",
            "RAW_BACKEND_FAILURE",
        ] {
            assert!(!output.contains(secret), "private data printed");
        }
    }
}
#[test]
fn existing_login_and_preferences_are_preserved_without_flags() {
    let mut core = Fake::default();
    let mut ui = Console::default();
    assert_eq!(
        setup(
            &mut core,
            &mut ui,
            &options(&["--json", "--non-interactive"])
        )
        .unwrap(),
        0
    );
    assert!(!core.calls.iter().any(|(method, _)| matches!(
        *method,
        "account.sign_in" | "tools.select" | "uploader.sharing.set"
    )));
    assert_eq!(ui.result()["sharing"], "all");
    ui.assert_private();
}
#[test]
fn new_noninteractive_setup_returns_public_device_instructions_without_waiting() {
    let mut core = Fake {
        account: json!({"state":"signed_out"}),
        ..Fake::default()
    };
    let mut ui = Console::default();
    assert_eq!(setup(&mut core, &mut ui, &options(&["--json"])).unwrap(), 2);
    let request = &core
        .calls
        .iter()
        .find(|(method, _)| *method == "account.sign_in")
        .unwrap()
        .1;
    assert_eq!(request["flow"], "device");
    assert_eq!(request["sharing"], "selected");
    assert_eq!(request["acknowledge_uninspected_schedules"], false);
    assert_eq!(ui.result()["user_code"], "ABCD-EFGH");
    assert!(!core
        .calls
        .iter()
        .any(|(method, _)| *method == "tools.select"));
    ui.assert_private();
}
#[test]
fn pending_device_login_is_resumed_without_creating_another_attempt() {
    let mut core = Fake {
        statuses: VecDeque::from([pending_account(), ready()]),
        ..Fake::default()
    };
    let mut ui = Console::default();
    assert_eq!(
        setup(
            &mut core,
            &mut ui,
            &options(&["--wait", "--non-interactive"])
        )
        .unwrap(),
        0
    );
    assert!(!core
        .calls
        .iter()
        .any(|(method, _)| *method == "account.sign_in"));
    ui.assert_private();
}
#[test]
fn explicit_provider_and_upload_choices_are_applied_after_approval() {
    let mut core = Fake {
        statuses: VecDeque::from([json!({"state":"signed_out"}), ready()]),
        ..Fake::default()
    };
    core.tools["onboarding_complete"] = json!(false);
    for tool in core.tools["tools"].as_array_mut().unwrap() {
        tool["selected"] = json!(false);
    }
    core.uploader["sharing_mode"] = json!("selected");
    let mut ui = Console::default();
    let opts = options(&[
        "--providers",
        "codex,claude",
        "--sharing",
        "new",
        "--json",
        "--wait",
    ]);
    assert_eq!(setup(&mut core, &mut ui, &opts).unwrap(), 0);
    assert_eq!(
        provider_ids(&core.tools),
        BTreeSet::from(["claude".into(), "codex".into()])
    );
    assert_eq!(core.uploader["sharing_mode"], "new");
    ui.assert_private();
}
#[test]
fn an_enforced_upload_policy_is_neither_asked_nor_changed() {
    let mut core = Fake::default();
    core.uploader["sharing_mode"] = json!("new");
    core.uploader["enforced_settings"] = json!({"sharing_mode":"new"});
    for (args, answers) in [
        (vec!["--sharing", "all", "--non-interactive"], vec![]),
        (vec![], vec![String::new(), "all".to_string()]),
    ] {
        let mut ui = Console {
            interactive: args.is_empty(),
            answers: answers.into(),
            ..Console::default()
        };
        assert_eq!(setup(&mut core, &mut ui, &options(&args)).unwrap(), 0);
        assert!(!core
            .calls
            .iter()
            .any(|(method, _)| *method == "uploader.sharing.set"));
        assert!(ui.events.iter().any(|event| event["event"] == "sharing"
            && event["message"] == "Upload policy is new. Managed by your organization."));
        // The interactive run asked for providers only.
        let unasked: Vec<String> = ui.answers.into();
        let expected: &[&str] = if args.is_empty() { &["all"] } else { &[] };
        assert_eq!(unasked, expected);
    }
}
#[test]
fn external_provider_requests_do_not_disconnect_or_install_local_mcp() {
    for connected in [false, true] {
        let mut core = Fake::default();
        if connected {
            core.tools["tools"][3]["state"] = json!("configured");
        }
        let mut ui = Console::default();
        let exit = setup(&mut core, &mut ui, &options(&["--providers", "capy"])).unwrap();
        assert_eq!(exit, if connected { 0 } else { 2 });
        assert!(!core
            .calls
            .iter()
            .any(|(method, _)| *method == "tools.select"));
        assert_eq!(provider_ids(&core.tools), BTreeSet::from(["codex".into()]));
        ui.assert_private();
    }
}
#[test]
fn cursor_uses_only_an_advertised_public_setting() {
    let mut core = Fake::default();
    let mut ui = Console::default();
    assert_eq!(
        setup(&mut core, &mut ui, &options(&["--providers", "cursor"])).unwrap(),
        0
    );
    assert_eq!(core.settings["cursor_direct_delivery"], true);
    assert!(!core
        .calls
        .iter()
        .any(|(method, _)| *method == "tools.select"));
    core.settings = json!({});
    let mut ui = Console::default();
    assert_eq!(
        setup(&mut core, &mut ui, &options(&["--providers", "cursor"])).unwrap(),
        2
    );
}
#[test]
fn denied_approval_and_backend_errors_have_static_safe_reasons() {
    let mut core = Fake {
        statuses: VecDeque::from([
            pending_account(),
            json!({"state":"denied","error":"SECRET_ACCESS_TOKEN"}),
        ]),
        ..Fake::default()
    };
    let mut ui = Console::default();
    assert_eq!(
        setup(&mut core, &mut ui, &options(&["--wait"]))
            .unwrap_err()
            .code,
        "approval_denied"
    );
    ui.assert_private();
    let mut core = Fake {
        failed: Some("tools.status"),
        ..Fake::default()
    };
    assert_eq!(
        setup(&mut core, &mut ui, &options(&[])).unwrap_err().code,
        "core_request_failed"
    );
    ui.assert_private();
}
#[test]
fn schedule_acknowledgement_is_never_implied_by_automation() {
    let mut core = Fake {
        account: json!({"state":"signed_out"}),
        sign_in: json!({"state":"error","action":"acknowledge_uninspected_schedules"}),
        ..Fake::default()
    };
    let mut ui = Console::default();
    assert_eq!(setup(&mut core, &mut ui, &options(&["--json"])).unwrap(), 2);
    assert_eq!(ui.result()["code"], "schedule_acknowledgement_required");
    let mut core = Fake {
        account: json!({"state":"signed_out"}),
        sign_in: ready(),
        ..Fake::default()
    };
    assert_eq!(
        setup(
            &mut core,
            &mut ui,
            &options(&["--acknowledge-uninspected-schedules"])
        )
        .unwrap(),
        0
    );
    assert_eq!(
        core.calls
            .iter()
            .find(|(method, _)| *method == "account.sign_in")
            .unwrap()
            .1["acknowledge_uninspected_schedules"],
        true
    );
}
#[test]
fn timeout_keeps_the_core_attempt_instead_of_cancelling_or_stopping_it() {
    let mut core = Fake {
        account: pending_account(),
        ..Fake::default()
    };
    let mut ui = Console::default();
    assert_eq!(
        setup(&mut core, &mut ui, &options(&["--wait", "--timeout", "1"])).unwrap(),
        2
    );
    assert_eq!(ui.result()["code"], "setup_timeout");
    assert!(!core.calls.iter().any(|(method, _)| matches!(
        *method,
        "account.sign_in" | "account.sign_in.cancel" | "daemon.shutdown"
    )));
}
#[test]
fn help_and_invalid_arguments_finish_before_rpc_work() {
    let error = Options::try_parse_from(["setup", "--help"]).unwrap_err();
    assert_eq!(error.exit_code(), 0);
    assert!(Options::try_parse_from(["setup", "--timeout", "0"]).is_err());
    assert!(!error.to_string().contains("--site-url"));
    assert!(Options::try_parse_from(["setup", "--token", "SECRET_ACCESS_TOKEN"]).is_err());
}

/// `--site-url` is gone: it still parses (hidden) so setup can say why,
/// as a result event naming the site this core is built for, before any
/// call reaches the core.
#[test]
fn a_site_url_is_refused_with_why_before_any_rpc() {
    let mut core = Fake::default();
    let mut ui = Console::default();
    let exit = setup(
        &mut core,
        &mut ui,
        &options(&[
            "--site-url",
            "http://localhost:3100",
            "--json",
            "--non-interactive",
        ]),
    )
    .unwrap();
    assert_eq!(exit, 1, "a usage error is not action_required (2)");
    assert!(core.calls.is_empty(), "{:?}", core.calls);
    let result = ui.result();
    assert_eq!(result["status"], "error");
    assert_eq!(result["code"], "site_url_gone");
    let message = result["message"].as_str().unwrap();
    assert!(message.starts_with("--site-url is gone: this core is built for "));
    assert!(message.contains(built_in_site_url()));
    assert!(message.contains("fixed when the core is built"));
    assert!(!message.contains("http://localhost:3100"));
}

#[test]
fn interactive_wizard_accepts_explicit_choices_after_device_approval() {
    let mut core = Fake {
        statuses: VecDeque::from([pending_account(), ready()]),
        ..Fake::default()
    };
    let mut ui = Console {
        interactive: true,
        answers: VecDeque::from(["codex,claude".into(), "selected".into()]),
        ..Console::default()
    };
    assert_eq!(setup(&mut core, &mut ui, &options(&[])).unwrap(), 0);
    assert!(ui.answers.is_empty());
    assert_eq!(
        provider_ids(&core.tools),
        BTreeSet::from(["codex".into(), "claude".into()])
    );
    assert_eq!(core.uploader["sharing_mode"], "selected");
    ui.assert_private();
}

#[test]
fn unhealthy_results_require_action_without_private_error_details() {
    for broken in [
        "account",
        "workspace",
        "provider",
        "uploader",
        "claude_cloud",
    ] {
        let mut core = Fake::default();
        match broken {
            "account" => core.tools["account_health"] = json!("sign_in_required"),
            "workspace" => {
                core.workspace = json!({"verification":"rejected","detail":"SECRET_HISTORY"})
            }
            "uploader" => core.uploader["stage"] = json!("needs_attention"),
            "claude_cloud" => {
                core.uploader["claude_cloud"] = json!({"state":"blocked","reason":"SECRET_HISTORY","keychain_error_code":-25293})
            }
            _ => core.tools["tools"][0]["state"] = json!("action_required"),
        }
        let mut ui = Console::default();
        assert_eq!(setup(&mut core, &mut ui, &options(&["--json"])).unwrap(), 2);
        assert_eq!(ui.result()["status"], "action_required");
        ui.assert_private();
    }
}

#[test]
fn workspace_mismatch_and_missing_service_do_not_mutate_preferences() {
    let mut core = Fake::default();
    let mut ui = Console::default();
    assert_eq!(
        setup(
            &mut core,
            &mut ui,
            &options(&["--workspace", "workspace-b", "--sharing", "new"])
        )
        .unwrap(),
        2
    );
    assert_eq!(core.calls.len(), 1);
    assert_eq!(ui.result()["code"], "different_workspace");
    core.failed = Some("account.status");
    assert_eq!(
        setup(&mut core, &mut ui, &options(&["--start-service"])).unwrap(),
        2
    );
    assert_eq!(ui.result()["code"], "core_not_running");
}
