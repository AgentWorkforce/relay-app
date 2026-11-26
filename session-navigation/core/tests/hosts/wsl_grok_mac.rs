use super::*;

#[test]
fn a_stopped_wsl_session_resumes_in_its_distribution_with_its_profile() {
    let mut w = world(Platform::Windows, vars());
    let s = start(
        &mut w,
        Provider::OpenCode,
        TerminalKind::WindowsTerminal,
        Mux::None,
        true,
    );
    assert!(w.wsl_sessions.contains(&s));
    let snap_live = engine::plan(&mut w, &s, &plain()).0;
    assert!(matches!(
        snap_live.runtime,
        Fact::Known(Runtime::Wsl { .. })
    ));
    // Live inside WSL: no window can be tied to it.
    let live = open(&mut w, &s, &plain());
    assert_ne!(
        live.decision.action().map(|a| a.kind),
        Some(ActionKind::FocusPane)
    );
    let panes: Vec<u64> = w.panes.keys().copied().collect();
    for p in panes {
        w.close_pane(p);
    }
    w.profiles
        .insert(s.clone(), "{11111111-2222-3333-4444-555555555555}".into());
    let mut request = plain();
    request.prefs.default_terminal = Some(TerminalKind::WindowsTerminal);
    let r = open(&mut w, &s, &request);
    let Some(Step::Launch {
        context, command, ..
    }) = r.decision.action().and_then(|a| a.steps.first())
    else {
        panic!("expected a launch: {:?}", r.decision);
    };
    assert!(matches!(context.runtime, Runtime::Wsl { .. }));
    assert_eq!(
        context.profile.as_deref(),
        Some("{11111111-2222-3333-4444-555555555555}")
    );
    assert!(
        command.program.starts_with('/'),
        "the distribution's own path"
    );
}

#[test]
fn grok_live_stopped_child_and_missing() {
    let mut w = world(Platform::MacOs, vars());
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Ghostty,
        Mux::None,
        false,
    );
    let live = open(&mut w, &s, &plain());
    let a = live.decision.action().expect("focus");
    assert_eq!(a.kind, ActionKind::FocusPane);
    assert_eq!(a.precision, Precision::RightWindow);
    let panes: Vec<u64> = w.panes.keys().copied().collect();
    for p in panes {
        w.close_pane(p);
    }
    let stopped = open(&mut w, &s, &plain());
    let Some(Step::Launch { command, .. }) =
        stopped.decision.action().and_then(|a| a.steps.first())
    else {
        panic!("expected a resume: {:?}", stopped.decision);
    };
    assert_eq!(command.args, vec!["--resume".to_owned(), s.id.clone()]);
    let pick = w.catalog.iter().position(|c| *c == s).unwrap() as u32;
    w.apply(&Event::SpawnChild { pick });
    let child = w.catalog.last().cloned().unwrap();
    let r = open(&mut w, &child, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::ChildSession));
    let missing = SessionRef::new(Provider::Grok, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
    let r = open(&mut w, &missing, &plain());
    assert_eq!(r.decision.plan, Plan::Unavailable(Reason::SessionNotFound));
}

#[test]
fn grok_resume_next_to_another_grok_in_the_directory_asks_first() {
    let mut w = world(Platform::MacOs, vars());
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Ghostty,
        Mux::None,
        false,
    );
    let pids: Vec<u32> = w
        .procs
        .values()
        .filter(|p| p.hosts.as_ref() == Some(&s))
        .map(|p| p.pid)
        .collect();
    for pid in pids {
        w.kill(pid, session_navigation::sim::truth::Exit::Normal);
    }
    let _other = start(
        &mut w,
        Provider::Grok,
        TerminalKind::Ghostty,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    assert!(
        matches!(r.decision.plan, Plan::Confirm(_)),
        "{:?}",
        r.decision
    );
}

#[test]
fn a_tty_reported_before_its_first_process_keeps_the_launch_pending() {
    let mut w = world(Platform::MacOs, vars());
    let s = start(
        &mut w,
        Provider::Claude,
        TerminalKind::Ghostty,
        Mux::None,
        false,
    );
    let panes: Vec<u64> = w.panes.keys().copied().collect();
    for p in panes {
        w.close_pane(p);
    }
    w.apply(&Event::SlowStart { on: true });
    let first = open(&mut w, &s, &plain());
    assert_eq!(
        first.decision.action().map(|a| a.kind),
        Some(ActionKind::Resume)
    );
    assert!(matches!(
        first.executed.last(),
        Some((_, session_navigation::plan::Dispatch::Opened { .. }))
    ));
    // The window reported its tty; nothing runs on it yet.
    let again = open(&mut w, &s, &plain());
    assert_eq!(again.outcome, Outcome::Blocked(Reason::LaunchPending));
    w.apply(&Event::Tick { seconds: 1 });
    let later = open(&mut w, &s, &plain());
    assert_eq!(
        later.decision.action().map(|a| a.kind),
        Some(ActionKind::FocusPane)
    );
    let launches = w
        .log
        .iter()
        .filter(|e| {
            matches!(
                e,
                session_navigation::sim::world::LogEntry::LaunchDispatched { open: Some(_), .. }
            )
        })
        .count();
    assert_eq!(launches, 1);
}

#[test]
fn mac_terminals_without_an_adapter_only_come_forward() {
    let mut w = world(Platform::MacOs, vars());
    w.hosts.api.insert(TerminalKind::WezTerm, false);
    let s = start(
        &mut w,
        Provider::Grok,
        TerminalKind::WezTerm,
        Mux::None,
        false,
    );
    let r = open(&mut w, &s, &plain());
    let a = r.decision.action().expect("bring forward");
    assert_eq!(a.kind, ActionKind::BringForward);
    assert_eq!(a.precision, Precision::AppOnly);
    assert_eq!(a.note, Some(Reason::HostSettingDisabled));
}
