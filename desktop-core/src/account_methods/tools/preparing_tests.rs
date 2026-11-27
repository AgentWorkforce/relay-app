use super::*;
use crate::history::uploader::{ProcessRunner, Uploader};
use agent_relay_node::relay::daemon::Config;
use agent_relay_node::relay::paths::CorePaths;
use agent_relay_node::relay::state::{AccountPerson, AccountStatus, SignInState};
use std::path::{Path, PathBuf};

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";

struct Fixture {
    root: tempfile::TempDir,
    account: Arc<Account>,
    core: Arc<Core>,
    installer: Arc<Installer>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        std::fs::create_dir_all(home.join(".agent-relay")).unwrap();
        std::fs::write(
            home.join(".agent-relay/tool-integrations.json"),
            r#"["codex","claude","grok"]"#,
        )
        .unwrap();
        let local = agent_relay_node::relay::local::LocalConfig::from_env(&|_| None, home);
        let installer = Arc::new(Installer::new(
            home.into(),
            local.codex,
            local.claude,
            local.opencode,
        ));
        let account = Arc::new(Account::new(
            &Default::default(),
            &home.join("app-data"),
            Arc::new(Uploader::new(Arc::new(ProcessRunner::new(
                home.join("no-probe"),
            )))),
        ));
        assert!(account.mcp_config.set(installer.clone()).is_ok());
        let (core, _signals) = Core::new(Config {
            paths: CorePaths::for_socket(home.join("core.sock")).unwrap(),
            home: home.into(),
            app_data_dir: home.join("app-data"),
            env: Default::default(),
            version: "0.0.0-test".into(),
            host_name: "Test Desk.local".into(),
            rediscover_every: std::time::Duration::from_secs(3600),
            push_timing: Default::default(),
            delivery_timing: Default::default(),
        });
        account.attach(&core);
        Self {
            root,
            account,
            core,
            installer,
        }
    }

    fn state(&self, account: AccountStatus) {
        self.core.hub.update(|status| status.account = account);
    }

    async fn call(&self, method: &str, params: Value) -> ToolsReport {
        let value = super::call(&self.account, method, &params)
            .await
            .unwrap()
            .unwrap();
        serde_json::from_value(value).unwrap()
    }

    fn assert_no_provider_credentials_or_configuration(&self) {
        for path in [
            ".codex/config.toml",
            ".claude.json",
            ".claude/settings.json",
            "app-data/mcp-provider-credentials.json",
        ] {
            assert!(
                !self.root.path().join(path).exists(),
                "unexpected provider write: {path}"
            );
        }
    }
}

fn preparing() -> AccountStatus {
    AccountStatus {
        state: SignInState::Preparing,
        phase: Some("preparing_local_sessions".into()),
        site_url: Some("https://relay.test".into()),
        workspace_id: Some(WORKSPACE.into()),
        person: Some(AccountPerson {
            id: Some("person-a".into()),
            name: None,
            email: None,
            avatar_url: None,
        }),
        ..AccountStatus::default()
    }
}

fn codex(report: &ToolsReport) -> &crate::contract::tool_integrations::ToolIntegration {
    report.tools.iter().find(|tool| tool.id == "codex").unwrap()
}

fn files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn collect(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut found = Vec::new();
    collect(root, root, &mut found);
    found.sort();
    found
}

#[tokio::test]
async fn verified_preparation_status_is_read_only_and_does_not_claim_connected() {
    let fixture = Fixture::new().await;
    for health in [None, Some("ready")] {
        let mut status = preparing();
        status.auth_health = health.map(str::to_string);
        fixture.state(status);
        let before = files(fixture.root.path());
        let report = fixture.call("tools.status", serde_json::json!({})).await;
        for id in ["codex", "claude", "grok"] {
            let tool = report.tools.iter().find(|tool| tool.id == id).unwrap();
            assert_eq!(tool.state, "preparing", "{id}/{health:?}");
            assert_eq!(tool.status_label, "Preparing sessions");
            assert!(!tool.can_install && !tool.can_repair);
            assert!(tool.detail.contains("You’re signed in"));
        }
        assert_eq!(
            files(fixture.root.path()),
            before,
            "tools.status must not write provider setup or credentials"
        );
        fixture.assert_no_provider_credentials_or_configuration();
    }
}

#[tokio::test]
async fn status_preparation_requires_complete_verified_local_preparation() {
    let fixture = Fixture::new().await;
    let mut cases = Vec::new();
    for state in [
        SignInState::SignedOut,
        SignInState::PendingApproval,
        SignInState::Error,
    ] {
        let mut status = preparing();
        status.state = state;
        cases.push(status);
    }
    for phase in [
        None,
        Some("opening_browser"),
        Some("waiting_for_device_approval"),
    ] {
        let mut status = preparing();
        status.phase = phase.map(str::to_string);
        cases.push(status);
    }
    let mut status = preparing();
    status.person = None;
    cases.push(status);
    let mut status = preparing();
    status.person.as_mut().unwrap().id = None;
    cases.push(status);
    let mut status = preparing();
    status.person.as_mut().unwrap().id = Some(" ".into());
    cases.push(status);
    let mut status = preparing();
    status.workspace_id = None;
    cases.push(status);
    let mut status = preparing();
    status.workspace_id = Some("not-a-workspace".into());
    cases.push(status);
    let mut status = preparing();
    status.site_url = None;
    cases.push(status);
    let mut status = preparing();
    status.site_url = Some("file:///not-cloud".into());
    cases.push(status);
    let mut status = preparing();
    status.error = Some("setup failed".into());
    cases.push(status);
    let mut status = preparing();
    status.auth_error = Some("authentication refused".into());
    cases.push(status);
    for health in [
        "sign_in_required",
        "workspace_unavailable",
        "offline",
        "checking",
    ] {
        let mut status = preparing();
        status.auth_health = Some(health.into());
        cases.push(status);
    }
    for status in cases {
        fixture.state(status.clone());
        let report = fixture.call("tools.status", serde_json::json!({})).await;
        assert_eq!(
            codex(&report).state,
            "signed_out",
            "invalid preparation: {status:?}"
        );
    }
    let mut signed_in = preparing();
    signed_in.state = SignInState::SignedIn;
    signed_in.phase = None;
    fixture.state(signed_in);
    let report = fixture.call("tools.status", serde_json::json!({})).await;
    assert_ne!(codex(&report).state, "signed_out");
    assert_ne!(codex(&report).state, "preparing");
}

#[tokio::test]
async fn preparation_status_keeps_an_actual_provider_login_requirement() {
    let fixture = Fixture::new().await;
    fixture.state(preparing());
    fixture.installer.set_provider_login_required(
        &Binding {
            site_url: "https://relay.test".into(),
            workspace_id: WORKSPACE.into(),
            account_id: "person-a".into(),
        },
        vec!["grok".into()],
    );
    let report = fixture.call("tools.status", serde_json::json!({})).await;
    let grok = report.tools.iter().find(|tool| tool.id == "grok").unwrap();
    assert_eq!(grok.state, "signed_out");
}

#[tokio::test]
async fn preparation_status_does_not_authorize_setup_select_refresh_or_recheck() {
    let fixture = Fixture::new().await;
    fixture.state(preparing());
    assert_eq!(
        codex(&fixture.call("tools.status", serde_json::json!({})).await).state,
        "preparing"
    );
    let mut calls = vec![
        ("tools.check", serde_json::json!({})),
        ("tools.select", serde_json::json!({"tool_ids":["codex"]})),
    ];
    for action in ["install", "repair", "reinstall", "enable"] {
        calls.push((
            "tools.manage",
            serde_json::json!({"tool_id":"codex", "action":action}),
        ));
    }
    for (method, params) in calls {
        let report = fixture.call(method, params.clone()).await;
        assert_eq!(
            codex(&report).state,
            "signed_out",
            "mutation/check used report-only readiness: {method}/{params}"
        );
        fixture.assert_no_provider_credentials_or_configuration();
    }
}
