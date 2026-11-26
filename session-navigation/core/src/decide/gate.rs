//! Version gating. Every adapter assumption was verified against specific
//! versions; on any other version the plan degrades to its most conservative
//! form and says why.
use crate::evidence::Snapshot;
use crate::model::{App, Component, Precision, Reason, Tool};
use crate::plan::{Action, ActionKind, Decision, Plan, PostCondition, Step, Target};

/// Terminal versions whose scripting dictionary was read (Ghostty.sdef 1.4.0,
/// facts Claude D / Codex 20) and exercised natively on 1.4.0: `every
/// terminal whose tty is` finds exactly one live tty and none for an unknown
/// one, `focus` is read back as the front window's focused terminal, and
/// `new window with configuration` reports its tty within ~1.5 s (native
/// N16, N17). Terminal.app's version was not recorded by the research, so it
/// is not gated; see README.
const TERMINAL_VERSIONS: &[(Component, &str)] = &[(Component::App(App::Ghostty), "1.4.0")];

/// What a plan depends on. A terminal's version matters for its macOS
/// scripting dictionary only; elsewhere its pane API or window manager is
/// modelled by world variables, not versions.
fn components(action: &Action, snap: &Snapshot) -> Vec<Component> {
    let mac = snap.platform == crate::model::Platform::MacOs;
    let mut out = Vec::new();
    for step in &action.steps {
        match step {
            Step::FocusPane { terminal, .. } => {
                if mac {
                    out.push(Component::App(terminal.app()));
                }
            }
            Step::OpenUrl { bundle, .. } => {
                if let Some(app) = App::from_handler(snap.platform, bundle) {
                    out.push(Component::App(app));
                }
            }
            Step::Launch {
                terminal, command, ..
            } => {
                if mac {
                    out.push(Component::App(terminal.app()));
                }
                if let Some(tool) = Tool::from_executable(&command.program) {
                    out.push(Component::Tool(tool));
                }
            }
            Step::ActivateApp { .. } | Step::Stop { .. } | Step::AwaitExit { .. } => {}
        }
    }
    out
}

fn version_of(snap: &Snapshot, component: Component) -> Option<String> {
    match component {
        Component::App(app) => snap
            .apps
            .known()?
            .iter()
            .find(|a| a.app == app)
            .and_then(|a| a.version.clone()),
        Component::Tool(tool) => snap
            .tools
            .iter()
            .find(|t| t.tool == tool)
            .and_then(|t| t.version.known().cloned().flatten()),
    }
}

fn verified(snap: &Snapshot, component: Component, tables: [&[(Component, &str)]; 2]) -> bool {
    let entries: Vec<&str> = tables
        .iter()
        .flat_map(|t| t.iter())
        .filter(|(c, _)| *c == component)
        .map(|(_, v)| *v)
        .collect();
    if entries.is_empty() {
        return true;
    }
    version_of(snap, component).is_some_and(|v| entries.contains(&v.as_str()))
}

fn app_of(component: Component) -> Option<App> {
    match component {
        Component::App(app) => Some(app),
        Component::Tool(_) => None,
    }
}

/// Focus and links degrade to bringing the app forward; anything that starts
/// a process becomes unavailable.
fn degrade(
    action: Action,
    snap: &Snapshot,
    provider: &[(Component, &str)],
) -> Result<Action, Reason> {
    let unverified: Vec<Component> = components(&action, snap)
        .into_iter()
        .filter(|c| !verified(snap, *c, [provider, TERMINAL_VERSIONS]))
        .collect();
    if unverified.is_empty() || action.kind == ActionKind::BringForward {
        return Ok(action);
    }
    if action.starts_process() {
        return Err(Reason::UnverifiedVersion);
    }
    let app = components(&action, snap)
        .into_iter()
        .find_map(app_of)
        .ok_or(Reason::UnverifiedVersion)?;
    let installed = snap
        .apps
        .known()
        .is_some_and(|apps| apps.iter().any(|a| a.app == app && a.installed));
    if !installed {
        return Err(Reason::UnverifiedVersion);
    }
    Ok(Action {
        kind: ActionKind::BringForward,
        steps: vec![Step::ActivateApp { app }],
        precision: Precision::AppOnly,
        post: PostCondition::AppFrontmost { app },
        note: Some(Reason::UnverifiedVersion),
        import: false,
        target: Some(Target::Desktop(app)),
    })
}

pub fn apply(decision: Decision, snap: &Snapshot, provider: &[(Component, &str)]) -> Decision {
    let state = decision.state;
    match decision.plan {
        Plan::Act(action) => match degrade(action, snap, provider) {
            Ok(action) => Decision::new(state, Plan::Act(action)),
            Err(reason) => Decision::unavailable(state, reason),
        },
        Plan::Choose(actions) => {
            let mut kept: Vec<Action> = Vec::new();
            for action in actions {
                if let Ok(action) = degrade(action, snap, provider) {
                    if !kept.contains(&action) {
                        kept.push(action);
                    }
                }
            }
            if kept.len() > 1 {
                return Decision::new(state, Plan::Choose(kept));
            }
            match kept.pop() {
                Some(action) => Decision::new(state, Plan::Act(action)),
                None => Decision::unavailable(state, Reason::UnverifiedVersion),
            }
        }
        // A confirmation is needed only for the action it guards; a degraded
        // (bring-forward) action needs none.
        Plan::Confirm(mut confirm) => match degrade(confirm.then.clone(), snap, provider) {
            Ok(action) if action.kind == ActionKind::BringForward => {
                Decision::new(state, Plan::Act(action))
            }
            Ok(action) => {
                confirm.then = action;
                Decision::new(state, Plan::Confirm(confirm))
            }
            Err(reason) => Decision::unavailable(state, reason),
        },
        plan => Decision::new(state, plan),
    }
}
