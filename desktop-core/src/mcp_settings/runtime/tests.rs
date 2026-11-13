use super::*;
use std::collections::BTreeSet;

fn report(loaded: &[&str], verified: &[&str]) -> app_server::McpReloadReport {
    app_server::McpReloadReport {
        absent: BTreeSet::new(),
        loaded: loaded
            .iter()
            .map(|id| id.to_string())
            .collect::<BTreeSet<_>>(),
        verified: verified
            .iter()
            .map(|id| id.to_string())
            .collect::<BTreeSet<_>>(),
    }
}

#[test]
fn initial_failures_partial_and_empty_proofs_retry_reload_after_cooldown() {
    for first in [
        Err(()),
        Ok(report(&[], &[])),
        Ok(report(&["a", "b"], &["a"])),
    ] {
        let mut runtime = Runtime {
            enabled: true,
            ..Default::default()
        };
        let now = Instant::now();
        let (generation, reload) = runtime.prepare("current", true, now).unwrap();
        assert!(reload);
        runtime.complete("current", generation, first);
        assert!(runtime.stamp.is_none());
        assert!(runtime
            .prepare("current", true, now + Duration::from_secs(29))
            .is_none());
        let (generation, reload) = runtime
            .prepare("current", true, now + Duration::from_secs(30))
            .unwrap();
        assert!(
            reload,
            "a failed native initialization must receive another reload"
        );
        runtime.complete("current", generation, Ok(report(&["a", "b"], &["a", "b"])));
        assert_eq!(runtime.stamp.as_deref(), Some("current"));
        assert!(
            !runtime
                .prepare("current", true, now + Duration::from_secs(60))
                .unwrap()
                .1,
            "healthy unchanged connections only need status inspection"
        );
    }
}

#[test]
fn lost_runtime_proof_reenables_reload_and_invalidated_attempts_cannot_publish() {
    let mut runtime = Runtime {
        enabled: true,
        ..Default::default()
    };
    let now = Instant::now();
    let (generation, _) = runtime.prepare("account-a", true, now).unwrap();
    runtime.complete("account-a", generation, Ok(report(&["a"], &["a"])));
    let (generation, reload) = runtime
        .prepare("account-a", true, now + Duration::from_secs(30))
        .unwrap();
    assert!(!reload);
    runtime.complete("account-a", generation, Ok(report(&["a"], &[])));
    assert!(
        runtime
            .prepare("account-a", true, now + Duration::from_secs(60))
            .unwrap()
            .1
    );
    runtime.invalidate();
    let (new_generation, reload) = runtime
        .prepare("account-a", true, now + Duration::from_secs(60))
        .unwrap();
    assert!(reload);
    assert_ne!(new_generation, generation);
    runtime.complete("account-a", generation, Ok(report(&["a"], &["a"])));
    assert!(
        runtime.detail.is_none(),
        "late pre-sign-out results cannot restore old proof"
    );
}

fn binding() -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace-a".into(),
        account_id: "account-a".into(),
    }
}
fn installer(home: &Path) -> Installer {
    Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
    .with_live_reload()
}

#[test]
fn explicit_runtime_check_retries_inside_the_maintenance_cooldown() {
    let root = tempfile::tempdir().unwrap();
    let mut app = installer(root.path());
    app.codex.executable = None;
    let alice = binding();
    app.select(vec!["codex".into()], Some(&alice), None)
        .unwrap();
    let command = crate::probe_copy::destination(root.path());
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    fs::write(&command, "synthetic helper").unwrap();
    agent_relay_node::platform::fs::set_mode(&command, 0o700).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        codex::render(
            "",
            &alice.endpoint().unwrap(),
            &alice.helper(&command).unwrap(),
            &mut |_| true,
        )
        .unwrap(),
    )
    .unwrap();
    app.reconcile_runtime(&alice);
    let before = app.runtime.lock().unwrap().attempted.unwrap();
    app.reconcile_runtime(&alice);
    assert_eq!(app.runtime.lock().unwrap().attempted.unwrap(), before);
    app.recheck_runtime(&alice);
    assert!(app.runtime.lock().unwrap().attempted.unwrap() > before);
    assert!(app.runtime_problem("codex", Some(&alice), true).is_some());
}

#[test]
fn cached_details_are_bound_to_account_workspace_site_selection_and_config() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding();
    let scope = app.runtime_scope(&alice, true).unwrap();
    {
        let mut runtime = app.runtime.lock().unwrap();
        let (generation, _) = runtime.prepare(&scope, true, Instant::now()).unwrap();
        runtime.complete(&scope, generation, Ok(report(&["a"], &["a"])));
    }
    assert!(app.runtime_detail("codex", Some(&alice), true).is_some());
    let mut other = alice.clone();
    other.account_id = "account-b".into();
    assert!(app.runtime_detail("codex", Some(&other), true).is_none());
    other = alice.clone();
    other.workspace_id = "workspace-b".into();
    assert!(app.runtime_detail("codex", Some(&other), true).is_none());
    other = alice.clone();
    other.site_url = "https://preview.agentrelay.com".into();
    assert!(app.runtime_detail("codex", Some(&other), true).is_none());
    assert!(app.runtime_detail("codex", Some(&alice), false).is_none());
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        "model = 'synthetic-change'\n",
    )
    .unwrap();
    assert!(app.runtime_detail("codex", Some(&alice), true).is_none());
}

#[test]
fn signed_out_refresh_discards_positive_proof_before_same_account_signs_in() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding();
    let scope = app.runtime_scope(&alice, true).unwrap();
    {
        let mut runtime = app.runtime.lock().unwrap();
        let (generation, _) = runtime.prepare(&scope, true, Instant::now()).unwrap();
        runtime.complete(&scope, generation, Ok(report(&["a"], &["a"])));
    }
    app.refresh(None);
    assert!(app.runtime_detail("codex", Some(&alice), true).is_none());
    let mut runtime = app.runtime.lock().unwrap();
    assert!(runtime.prepare(&scope, true, Instant::now()).unwrap().1);
}

#[tokio::test]
async fn account_sign_out_immediately_discards_proof_without_a_periodic_refresh() {
    use crate::account::Account;
    use crate::history::uploader::{ProcessRunner, Uploader};
    use agent_relay_node::relay::{
        daemon::{Config, Core, Plugins},
        paths::CorePaths,
    };
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let app = Arc::new(installer(home));
    let alice = binding();
    let scope = app.runtime_scope(&alice, true).unwrap();
    {
        let mut runtime = app.runtime.lock().unwrap();
        let (generation, _) = runtime.prepare(&scope, true, Instant::now()).unwrap();
        runtime.complete(&scope, generation, Ok(report(&["a"], &["a"])));
    }
    let account = Arc::new(
        Account::new(
            &Default::default(),
            &home.join("app-data"),
            Arc::new(Uploader::new(Arc::new(ProcessRunner::new(
                home.join("unused-synthetic-probe"),
            )))),
        )
        .with_site(&alice.site_url)
        .unwrap(),
    );
    let _ = account.mcp_config.set(app.clone());
    let (core, _) = Core::with_plugins(
        Config {
            paths: CorePaths::for_socket(home.join("run/core.sock")).unwrap(),
            home: home.into(),
            app_data_dir: home.join("app-data"),
            env: Default::default(),
            version: "test".into(),
            host_name: "test".into(),
            rediscover_every: Duration::from_secs(3600),
            push_timing: Default::default(),
            delivery_timing: Default::default(),
        },
        Plugins {
            extensions: vec![Arc::new(crate::account::plugin::AccountMethods(account.clone()))],
            account: Some(account.clone().into_plugin()),
            ..Default::default()
        },
    );
    core.hub
        .update(|status| status.account.state = agent_relay_node::relay::state::SignInState::SignedIn);
    account.sign_out().await.unwrap();
    // Simulate the same account returning immediately, before any core refresh.
    core.hub
        .update(|status| status.account.state = agent_relay_node::relay::state::SignInState::SignedIn);
    assert!(app.runtime_detail("codex", Some(&alice), true).is_none());
    assert!(
        app.runtime
            .lock()
            .unwrap()
            .prepare(&scope, true, Instant::now())
            .unwrap()
            .1
    );
}

#[test]
fn provider_health_never_turns_green_for_failed_partial_or_unattempted_runtime() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding();
    let command = crate::probe_copy::destination(root.path());
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    fs::write(&command, "synthetic helper").unwrap();
    agent_relay_node::platform::fs::set_mode(&command, 0o700).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    let helper = alice.helper(&command).unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        codex::render("", &alice.endpoint().unwrap(), &helper, &mut |_| true).unwrap(),
    )
    .unwrap();
    app.select(vec!["codex".into()], Some(&alice), None)
        .unwrap();
    let row = || {
        app.report(Some(&alice), Some("ready".into()))
            .tools
            .into_iter()
            .find(|tool| tool.id == "codex")
            .unwrap()
    };
    assert_eq!(row().state, "installed");
    assert_eq!(row().status_label, "Setup saved");
    let scope = app.runtime_scope(&alice, true).unwrap();
    let generation = app
        .runtime
        .lock()
        .unwrap()
        .prepare(&scope, true, Instant::now())
        .unwrap()
        .0;
    for result in [Err(()), Ok(report(&["a", "b"], &["a"]))] {
        app.runtime
            .lock()
            .unwrap()
            .complete(&scope, generation, result);
        let tool = row();
        assert_eq!(tool.state, "action_required");
        assert_eq!(tool.status_label, "MCP not ready");
        assert!(tool.can_repair && tool.can_uninstall);
    }
    app.runtime
        .lock()
        .unwrap()
        .complete(&scope, generation, Ok(report(&[], &[])));
    assert_eq!(row().state, "installed");
    assert!(row().detail.contains("No loaded chats"));
    app.runtime
        .lock()
        .unwrap()
        .complete(&scope, generation, Ok(report(&["a"], &["a"])));
    assert_eq!(row().state, "configured");
    assert!(row().detail.contains("1 of 1"));
}

#[test]
fn uninstall_verifies_absence_and_retries_until_every_loaded_chat_removes_tools() {
    let mut runtime = Runtime {
        enabled: true,
        ..Default::default()
    };
    let now = Instant::now();
    let (generation, _) = runtime.prepare("installed", true, now).unwrap();
    runtime.complete(
        "installed",
        generation,
        Ok(report(&["a", "b"], &["a", "b"])),
    );
    let (generation, reload) = runtime.prepare("removed", false, now).unwrap();
    assert!(reload);
    let mut partial = report(&["a", "b"], &["b"]);
    partial.absent.insert("a".into());
    runtime.complete("removed", generation, Ok(partial));
    assert!(runtime.unhealthy);
    assert!(runtime.cleanup);
    let (generation, reload) = runtime
        .prepare("removed", false, now + Duration::from_secs(30))
        .unwrap();
    assert!(reload);
    let mut removed = report(&["a", "b"], &[]);
    removed.absent = removed.loaded.clone();
    runtime.complete("removed", generation, Ok(removed));
    assert!(!runtime.unhealthy);
    assert!(!runtime.cleanup);
    assert!(runtime
        .detail
        .as_deref()
        .unwrap()
        .contains("absent from 2 of 2"));
    assert!(runtime
        .prepare("removed", false, now + Duration::from_secs(60))
        .is_none());
}

#[test]
fn explicit_removal_cleans_up_without_a_recorded_desire() {
    let mut runtime = Runtime {
        enabled: true,
        ..Default::default()
    };
    let now = Instant::now();
    // A restarted core never recorded `desired`, so nothing would clean up.
    assert!(runtime.prepare("removed", false, now).is_none());
    runtime.removed = true;
    let (generation, _) = runtime
        .prepare("removed", false, now + Duration::from_secs(1))
        .expect("Disconnect asks running chats to drop Relay tools");
    assert!(runtime.cleanup);
    runtime.complete("removed", generation, Ok(report(&[], &[])));
    assert!(!runtime.cleanup);
    // A fresh scope after removal cleans up too.
    let mut runtime = Runtime {
        enabled: true,
        removed: true,
        ..Default::default()
    };
    assert!(runtime.prepare("other", false, now).is_some());
    assert!(runtime.cleanup);
}
