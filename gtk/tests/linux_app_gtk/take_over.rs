use super::*;
use agent_relay_app::models::{LocalSessionRow, SPAWN_ENDING};

pub(super) fn ending_spawned_agent(runtime: &tokio::runtime::Runtime) {
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let case = Case::with(runtime, move |core| {
        let sessions: Vec<_> = ["worker", "other"]
            .into_iter()
            .map(|name| {
                local_session(
                    "claude-terminal",
                    name,
                    name,
                    true,
                    json!({"name": name, "address": format!("{name}@desk"), "added_by_session": false,
                        "spawn": {"harness": "claude", "can_take_over": true, "taken_over": false}}),
                )
            })
            .collect();
        core.answer(
            "sessions.list",
            check::<method::SessionsList>(json!({"sessions": sessions})),
        );
        core.answer(
            "agents.end",
            Answer::with(move |params| {
                let gate = gate.clone();
                async move {
                    if params["name"] == "worker" {
                        gate.notified().await;
                    }
                    Ok(json!({}))
                }
            }),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| case.app.with_state(|state| state.local_sessions.len() == 2));
    case.app.go(Page::Sessions);
    let open = |name: &str| {
        let relay = case.app.with_state(|state| {
            state
                .local_sessions
                .iter()
                .find(|session| session.target.id == name)
                .cloned()
        });
        case.app.open_local_session(LocalSessionRow {
            upload: None,
            relay,
        });
    };
    open("worker");
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    assert!(!page.details.is_active(), "Session details start closed");
    page.details.set_active(true);
    assert!(page.join.end.is_visible());
    assert!(page.join.end.is_sensitive());
    assert!(
        !page.receive.is_sensitive(),
        "Receive messages cannot silently end an owned spawned agent"
    );

    page.join.end.emit_clicked();
    case.pump(|case| case.called("agents.end").len() == 1);
    assert_eq!(case.called("agents.end")[0], json!({"name": "worker"}));
    let confirmation = case.desktop.asked.borrow().last().unwrap().clone();
    assert_eq!(confirmation.title, "End @worker?");
    assert_eq!(confirmation.action, "End Agent");
    assert!(confirmation.destructive);

    open("other");
    assert!(
        !page.details.is_active(),
        "Selecting a session closes Details"
    );
    page.details.set_active(true);
    assert!(
        page.join.end.is_sensitive(),
        "ending one agent must not block another"
    );
    page.join.end.emit_clicked();
    case.pump(|case| case.called("agents.end").len() == 2);
    assert_eq!(case.called("agents.end")[1], json!({"name": "other"}));
    case.pump(|_| page.join.end.is_sensitive());
    open("worker");
    page.details.set_active(true);
    assert!(!page.join.end.is_sensitive());
    assert_eq!(page.join.end.label().as_deref(), Some(SPAWN_ENDING));
    release.notify_one();
    case.pump(|_| page.join.end.is_sensitive());
}

pub(super) fn switching_transcripts(runtime: &tokio::runtime::Runtime) {
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let case = Case::with(runtime, move |core| {
        let sessions: Vec<_> = ["first", "second"].into_iter().map(|name| {
            let mut session = local_session("claude-terminal", name, name, true,
                json!({"name": name, "address": format!("{name}@desk"), "added_by_session": false,
                    "spawn": {"harness": "claude", "can_take_over": true, "taken_over": false}}));
            session["directory"] = json!("/work/api");
            session
        }).collect();
        core.answer(
            "sessions.list",
            check::<method::SessionsList>(json!({"sessions": sessions})),
        );
        let mut agent = agents()["agents"][0].clone();
        agent["name"] = json!("first");
        agent["target"] = json!({"kind": "claude-terminal", "id": "first"});
        agent["spawn"] = json!({"harness": "claude", "holds_slot": true,
            "node": {"state": "unknown"}, "process": {"state": "running"},
            "work": {"state": "working"}, "unfinished": null});
        core.answer(
            "agents.list",
            check::<method::AgentsList>(json!({"agents": [agent]})),
        );
        core.answer(
            "agents.take_over",
            Answer::with(move |params| {
                let gate = gate.clone();
                async move {
                    if params["interrupt"] != true {
                        gate.notified().await;
                    }
                    Err(RpcError::new(ErrorKind::Busy, "Still working"))
                }
            }),
        );
    });
    case.set_state(workspace_snapshot());
    case.pump(|case| {
        case.app
            .with_state(|state| state.local_sessions.len() == 2 && state.local_agents.len() == 1)
    });
    case.app.go(Page::Sessions);
    let open = |name: &str| {
        let relay = case.app.with_state(|state| {
            state
                .local_sessions
                .iter()
                .find(|s| s.target.id == name)
                .cloned()
        });
        case.app.open_local_session(LocalSessionRow {
            upload: None,
            relay,
        });
        assert!(
            !case
                .app
                .shell()
                .unwrap()
                .pages
                .sessions
                .transcript
                .details
                .is_active(),
            "Opening another conversation resets details"
        );
        case.app
            .shell()
            .unwrap()
            .pages
            .sessions
            .transcript
            .details
            .set_active(true);
    };
    open("first");
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    page.join.take_over.emit_clicked();
    case.pump(|case| case.called("agents.take_over").len() == 1);
    assert!(page.join.interrupt.is_visible());
    open("second");
    assert!(!page.join.interrupt.is_visible());
    // Even a queued click from the previous transcript remains pinned to first.
    page.join.interrupt.emit_clicked();
    case.pump(|case| case.called("agents.take_over").len() == 2);
    assert_eq!(case.called("agents.take_over")[1]["name"], "first");
    assert_eq!(case.called("agents.take_over")[1]["interrupt"], true);
    release.notify_one();
    case.pump(|_| page.join.take_over.is_sensitive());
    assert!(!labels(&page.sidebar)
        .iter()
        .any(|label| label.contains("Still working")));
    open("first");
    assert!(labels(&page.sidebar)
        .iter()
        .any(|label| label.contains("Still working")));
    open("second");
    let mut leaked = false;
    walk(page.sidebar.upcast_ref(), &mut |widget| {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            leaked |= label.is_visible() && label.text().contains("Still working");
        }
    });
    assert!(
        !leaked,
        "opening another transcript hides the previous session's error"
    );
    open("first");
    assert!(labels(&page.sidebar)
        .iter()
        .any(|label| label.contains("Still working")));
}
