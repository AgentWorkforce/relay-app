use super::*;
use std::collections::BTreeSet;

fn ids(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn with_absent(
    mut report: app_server::McpReloadReport,
    absent: &[&str],
) -> app_server::McpReloadReport {
    report.absent = ids(absent);
    report
}

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
        // One automatic failure is retried sooner; proofs wait the cooldown.
        let retry = if first.is_err() { 5 } else { 30 };
        runtime.complete("current", generation, first);
        assert!(runtime.stamp.is_none());
        assert!(runtime
            .prepare("current", true, now + Duration::from_secs(retry - 1))
            .is_none());
        let (generation, reload) = runtime
            .prepare("current", true, now + Duration::from_secs(retry))
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
fn a_check_that_starts_no_attempt_leaves_maintenance_lenient() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding();
    // Codex is not selected and nothing needs cleaning up: no attempt starts.
    app.recheck_runtime(&alice);
    let runtime = app.runtime.lock().unwrap();
    assert!(runtime.attempted.is_none(), "no attempt started");
    assert!(runtime.scope.is_some(), "the check reached prepare");
    assert!(!runtime.check_requested);
    assert!(!runtime.checking);
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
        let mut runtime = app.runtime.lock().unwrap();
        // Only Check connection reports an unreachable daemon.
        runtime.checking = result.is_err();
        runtime.complete(&scope, generation, result);
        drop(runtime);
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

fn runtime() -> Runtime {
    Runtime {
        enabled: true,
        ..Default::default()
    }
}

#[test]
fn automatic_failure_keeps_setup_saved_and_retries_once_sooner() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, _) = runtime.prepare("current", true, now).unwrap();
    runtime.complete("current", generation, Err(()));
    assert!(
        !runtime.unhealthy,
        "a slow or absent daemon is not a problem"
    );
    assert!(runtime.stamp.is_none(), "nothing was verified");
    assert!(runtime
        .detail
        .as_deref()
        .unwrap()
        .contains("could not reach"));
    let (generation, reload) = runtime
        .prepare("current", true, now + Duration::from_secs(5))
        .unwrap();
    assert!(reload);
    runtime.complete("current", generation, Err(()));
    assert!(!runtime.unhealthy);
    // The second consecutive failure falls back to the usual cooldown.
    let later = now + Duration::from_secs(5);
    assert!(runtime
        .prepare("current", true, later + Duration::from_secs(29))
        .is_none());
    assert!(runtime
        .prepare("current", true, later + Duration::from_secs(30))
        .is_some());
}

#[test]
fn automatic_failure_keeps_a_pending_cleanup() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, _) = runtime.prepare("installed", true, now).unwrap();
    runtime.complete("installed", generation, Ok(report(&["a"], &["a"])));
    let (generation, _) = runtime.prepare("removed", false, now).unwrap();
    runtime.complete("removed", generation, Err(()));
    assert!(runtime.cleanup, "the unload is retried");
    assert!(runtime
        .prepare("removed", false, now + Duration::from_secs(5))
        .is_some());
}

#[test]
fn explicit_check_reports_an_unreachable_daemon() {
    let mut runtime = runtime();
    runtime.check_requested = true;
    let (generation, _) = runtime.prepare("current", true, Instant::now()).unwrap();
    runtime.complete("current", generation, Err(()));
    assert!(runtime.unhealthy);
    let detail = runtime.detail.as_deref().unwrap();
    assert!(detail.contains("could not reach"));
    assert!(!detail.contains("try again automatically"));
}

#[test]
fn chats_open_before_setup_do_not_make_maintenance_unhealthy() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, reload) = runtime.prepare("current", true, now).unwrap();
    assert!(reload);
    // The server works in a newer chat, so older chats without it are
    // grandfathered rather than a problem.
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(
            report(&["new", "old-a", "old-b"], &["new"]),
            &["old-a", "old-b"],
        )),
    );
    assert!(!runtime.unhealthy);
    assert_eq!(runtime.stamp.as_deref(), Some("current"));
    assert_eq!(runtime.grandfathered, ids(&["old-a", "old-b"]));
    let detail = runtime.detail.as_deref().unwrap();
    assert!(detail.contains("1 of 3"));
    assert!(detail.contains("2 chats were open before Agent Relay was set up"));
    // Only grandfathered chats left: nothing verified, still Setup saved.
    let (generation, reload) = runtime
        .prepare("current", true, now + Duration::from_secs(30))
        .unwrap();
    assert!(!reload);
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(
            report(&["old-a", "old-b"], &[]),
            &["old-a", "old-b"],
        )),
    );
    assert!(!runtime.unhealthy);
    assert!(runtime.stamp.is_none(), "nothing verified: Setup saved");
    // Later maintenance reads instead of restarting servers in every chat.
    let (generation, reload) = runtime
        .prepare("current", true, now + Duration::from_secs(60))
        .unwrap();
    assert!(
        !reload,
        "only grandfathered chats lack tools; read, don't reload"
    );
    // A reloaded older chat leaves the grandfathered set.
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(
            report(&["new", "old-a", "old-b"], &["new", "old-a"]),
            &["old-b"],
        )),
    );
    assert_eq!(runtime.grandfathered, ids(&["old-b"]));
    assert!(runtime
        .detail
        .as_deref()
        .unwrap()
        .contains("1 chat was open before Agent Relay was set up"));
    // A new scope reloads again.
    assert!(
        runtime
            .prepare("rotated", true, now + Duration::from_secs(90))
            .unwrap()
            .1
    );
}

#[test]
fn a_broken_server_absent_from_every_chat_is_not_grandfathered() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, reload) = runtime.prepare("rotated", true, now).unwrap();
    assert!(reload);
    runtime.complete(
        "rotated",
        generation,
        Ok(with_absent(
            report(&["old-a", "old-b"], &[]),
            &["old-a", "old-b"],
        )),
    );
    assert!(runtime.unhealthy, "a server verified nowhere may be broken");
    assert!(runtime.grandfathered.is_empty());
    assert!(!runtime.baseline);
    assert!(runtime.stamp.is_none());
    assert!(!runtime
        .detail
        .as_deref()
        .unwrap()
        .contains("open before Agent Relay"));
    // The next attempt reloads and can still take the baseline.
    let (generation, reload) = runtime
        .prepare("rotated", true, now + Duration::from_secs(30))
        .unwrap();
    assert!(reload);
    runtime.complete(
        "rotated",
        generation,
        Ok(with_absent(
            report(&["new", "old-a", "old-b"], &["new"]),
            &["old-a", "old-b"],
        )),
    );
    assert!(!runtime.unhealthy);
    assert!(runtime.baseline);
    assert_eq!(runtime.grandfathered, ids(&["old-a", "old-b"]));
}

#[test]
fn a_grandfathered_chat_with_the_server_unverified_is_judged() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, _) = runtime.prepare("current", true, now).unwrap();
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(report(&["seen", "old"], &["seen"]), &["old"])),
    );
    assert_eq!(runtime.grandfathered, ids(&["old"]));
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(30))
        .unwrap();
    // "old" now has the server but it is not verified there.
    runtime.complete(
        "current",
        generation,
        Ok(report(&["seen", "old"], &["seen"])),
    );
    assert!(runtime.grandfathered.is_empty());
    assert!(runtime.unhealthy, "a partial chat is judged, not ignored");
}

#[test]
fn a_failure_after_a_problem_keeps_the_problem_and_its_detail() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, _) = runtime.prepare("current", true, now).unwrap();
    runtime.complete("current", generation, Ok(report(&["a", "b"], &["a"])));
    assert!(runtime.unhealthy);
    let last = runtime.detail.clone().unwrap();
    let expected = format!("{last} Agent Relay will try again automatically.");
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(30))
        .unwrap();
    runtime.complete("current", generation, Err(()));
    assert!(runtime.unhealthy, "a busy daemon proves nothing either way");
    assert_eq!(runtime.detail.as_deref(), Some(expected.as_str()));
    // A second failure does not repeat the retry note.
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(35))
        .unwrap();
    runtime.complete("current", generation, Err(()));
    assert!(runtime.unhealthy);
    assert_eq!(runtime.detail.as_deref(), Some(expected.as_str()));
}

#[test]
fn a_new_chat_without_tools_after_grandfathering_is_a_problem() {
    let mut runtime = runtime();
    let now = Instant::now();
    let (generation, _) = runtime.prepare("current", true, now).unwrap();
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(report(&["seen", "old"], &["seen"]), &["old"])),
    );
    assert!(!runtime.unhealthy);
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(30))
        .unwrap();
    runtime.complete(
        "current",
        generation,
        Ok(with_absent(
            report(&["new", "old", "seen"], &["seen"]),
            &["new", "old"],
        )),
    );
    assert!(
        runtime.unhealthy,
        "chats opened after setup must load the tools"
    );
    assert_eq!(runtime.grandfathered, ids(&["old"]));
}

#[test]
fn explicit_check_judges_grandfathered_chats_again() {
    let mut runtime = runtime();
    let now = Instant::now();
    let seen_and_old = || Ok(with_absent(report(&["seen", "old"], &["seen"]), &["old"]));
    let (generation, _) = runtime.prepare("current", true, now).unwrap();
    runtime.complete("current", generation, seen_and_old());
    assert!(!runtime.unhealthy);
    runtime.check_requested = true;
    runtime.attempted = None;
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(1))
        .unwrap();
    assert!(runtime.grandfathered.is_empty());
    runtime.complete("current", generation, seen_and_old());
    assert!(
        runtime.unhealthy,
        "Check connection reports chats without tools"
    );
    // Later maintenance keeps judging them until they are reloaded.
    let (generation, _) = runtime
        .prepare("current", true, now + Duration::from_secs(31))
        .unwrap();
    runtime.complete("current", generation, seen_and_old());
    assert!(runtime.unhealthy);
}

#[test]
fn a_reload_rereads_unsettled_chats_once_without_reloading_again() {
    let calls = std::cell::RefCell::new(Vec::new());
    let paused = std::cell::Cell::new(None);
    let result = settle_attempt(
        true,
        |report| report.loaded != report.verified,
        |reload, timeout| {
            calls.borrow_mut().push((reload, timeout));
            Ok(if calls.borrow().len() == 1 {
                report(&["a", "b"], &["a"])
            } else {
                report(&["a", "b"], &["a", "b"])
            })
        },
        |pause| paused.set(Some(pause)),
    )
    .unwrap();
    assert_eq!(result.verified, ids(&["a", "b"]));
    let calls = calls.into_inner();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].0 && !calls[1].0, "the second read never reloads");
    assert_eq!(paused.get(), Some(SETTLE_PAUSE));
    assert!(calls[0].1 + SETTLE_PAUSE <= ATTEMPT_BUDGET + SETTLE_PAUSE);
    assert!(SETTLE_PAUSE + calls[1].1 <= ATTEMPT_BUDGET);
}

#[test]
fn settled_unreloaded_or_failed_attempts_are_not_reread() {
    for (reload, first, unsettled) in [
        (true, Ok(report(&["a"], &["a"])), false),
        (false, Ok(report(&["a"], &[])), true),
        (true, Err(()), true),
    ] {
        let mut calls = 0;
        let expected = first.clone();
        let result = settle_attempt(
            reload,
            |_| unsettled,
            |_, _| {
                calls += 1;
                first.clone()
            },
            |_| panic!("no pause without a reread"),
        );
        assert_eq!(result, expected);
        assert_eq!(calls, 1);
    }
    // A failed second read fails the attempt; the first read may be stale.
    let mut calls = 0;
    let result = settle_attempt(
        true,
        |_| true,
        |_, _| {
            calls += 1;
            if calls == 1 {
                Ok(report(&["a"], &[]))
            } else {
                Err(())
            }
        },
        |_| {},
    );
    assert_eq!(result, Err(()));
}

#[test]
fn grandfathered_chats_do_not_trigger_a_reread() {
    let mut runtime = runtime();
    runtime.desired = true;
    runtime.grandfathered = ids(&["old"]);
    assert!(!runtime.unsettled(&with_absent(report(&["old"], &[]), &["old"])));
    assert!(runtime.unsettled(&report(&["old", "new"], &[])));
}
