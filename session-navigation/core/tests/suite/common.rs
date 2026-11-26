//! Shared helpers: a fully installed, permissioned world with no launch delay,
//! and accessors that read ids back from truth so assertions can name exact
//! URLs, argv and targets.

use session_navigation::engine;
use session_navigation::model::{
    App, Precision, ProcessIdentity, Reason, SessionRef, TerminalKind,
};
use session_navigation::plan::{ActionKind, Decision, OpenRequest, Plan, Preferences, Step};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::events::dir;
use session_navigation::sim::{Event, World, WorldVars};

pub fn vars() -> WorldVars {
    WorldVars {
        claude_resume_link_unarchives: true,
        claude_registry_rewritten_on_clear: true,
        claude_continue_starts_process: true,
        codex_tui_holds_lock: true,
        desktop_view_readable: true,
        ..WorldVars::default()
    }
}

pub fn world(vars: WorldVars) -> World {
    let mut w = World::new(7, vars);
    for app in App::ALL {
        if let Some(a) = w.apps.get_mut(&app) {
            a.installed = true;
        }
        w.automation.insert(app, true);
    }
    w.launch_delay = 0;
    // No Codex "Update available" menu unless a test asks for it (N12).
    w.codex.update_available = false;
    w
}

pub struct T {
    pub w: World,
    pub s: SessionRef,
}

pub fn start_with(vars: WorldVars, host: StartHost) -> T {
    let mut w = world(vars);
    let s = w.start_session(host, dir(0)).expect("session starts");
    w.apply(&Event::Discover);
    T { w, s }
}

pub fn start(host: StartHost) -> T {
    start_with(vars(), host)
}

pub fn request() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Preferences::default()
        },
        ..OpenRequest::default()
    }
}

impl T {
    pub fn apply(&mut self, event: Event) -> &mut Self {
        self.w.apply(&event);
        self
    }

    pub fn plan(&mut self) -> Decision {
        self.plan_with(&request())
    }

    pub fn plan_with(&mut self, request: &OpenRequest) -> Decision {
        engine::plan(&mut self.w, &self.s, request).1
    }

    /// Index of this session in the catalog (for pick-based events).
    pub fn pick(&self) -> u32 {
        self.w
            .catalog
            .iter()
            .position(|c| c == &self.s)
            .expect("catalogued") as u32
    }

    pub fn local_id(&self) -> String {
        self.w
            .claude
            .desktop
            .values()
            .find(|d| d.holds_live(&self.s.id))
            .map(|d| d.local.clone())
            .expect("desktop session")
    }

    pub fn owner(&self) -> (ProcessIdentity, String) {
        let p = self
            .w
            .procs
            .values()
            .find(|p| p.hosts.as_ref() == Some(&self.s) && p.tty.is_some())
            .expect("terminal owner");
        let identity = ProcessIdentity {
            pid: p.pid,
            start: p.start,
            boot: session_navigation::model::BootId(self.w.boot),
        };
        (identity, p.tty.clone().unwrap())
    }

    pub fn cwd(&self) -> String {
        dir(0).to_owned()
    }
}

pub fn act(d: &Decision) -> &session_navigation::plan::Action {
    match &d.plan {
        Plan::Act(a) => a,
        other => panic!("expected an action, got {other:?}"),
    }
}

pub fn expect_url(d: &Decision, kind: ActionKind, url: &str, app: App) {
    let a = act(d);
    assert_eq!(a.kind, kind, "{d:?}");
    assert_eq!(
        a.steps,
        vec![Step::OpenUrl {
            url: url.to_owned(),
            bundle: app.bundle_id().to_owned()
        }]
    );
    assert_eq!(a.precision, Precision::ExactConversation);
}

pub fn expect_focus(
    d: &Decision,
    terminal: TerminalKind,
    owner: &(ProcessIdentity, String),
    precision: Precision,
) {
    let a = act(d);
    assert_eq!(a.kind, ActionKind::FocusPane, "{d:?}");
    assert_eq!(
        a.steps,
        vec![Step::FocusPane {
            terminal,
            pane: session_navigation::hosts::PaneRef::Tty {
                tty: owner.1.clone(),
            },
            process: owner.0
        }]
    );
    assert_eq!(a.precision, precision);
}

pub fn expect_launch(
    d: &Decision,
    terminal: TerminalKind,
    program: &str,
    args: &[&str],
    cwd: &str,
) {
    let a = act(d);
    assert_eq!(a.kind, ActionKind::Resume, "{d:?}");
    let [Step::Launch {
        terminal: t,
        command,
        ..
    }] = a.steps.as_slice()
    else {
        panic!("expected one launch: {d:?}");
    };
    assert_eq!(*t, terminal);
    // Plans carry the resolved absolute executable, never a bare name.
    assert!(command.program.starts_with('/'), "{command:?}");
    assert_eq!(command.program.rsplit('/').next(), Some(program));
    assert_eq!(
        command.args,
        args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(command.cwd, cwd);
    assert!(command.env.is_empty());
}

pub fn expect_bring(d: &Decision, app: App, note: Reason) {
    let a = act(d);
    assert_eq!(a.kind, ActionKind::BringForward, "{d:?}");
    assert_eq!(a.steps, vec![Step::ActivateApp { app }]);
    assert_eq!(a.precision, Precision::AppOnly);
    assert_eq!(a.note, Some(note));
}

pub fn expect_unavailable(d: &Decision, reason: Reason) {
    assert_eq!(d.plan, Plan::Unavailable(reason), "{d:?}");
}

pub fn expect_choose(d: &Decision, n: usize) {
    match &d.plan {
        Plan::Choose(list) => assert_eq!(list.len(), n, "{d:?}"),
        other => panic!("expected a choice, got {other:?}"),
    }
}

pub fn person_resume(t: &mut T) {
    let pick = t.pick();
    t.apply(Event::PersonResume { pick });
}
