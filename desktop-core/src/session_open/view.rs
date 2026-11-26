//! A core [`Decision`] as the button the apps draw, and an engine report as
//! what one click did.
use session_navigation::engine::OpenReport;
use session_navigation::evidence::Fact;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{App, BootId, Precision as CorePrecision, ProcessIdentity};
use session_navigation::plan::{
    Action as CoreAction, ActionKind, Decision, Outcome as CoreOutcome, Plan, SessionState, Step,
    Target,
};

use super::contract::{
    Action, Choice, Destination, OpenResult, Outcome, PlanView, Precision, State,
};
use super::wording::{self, titles, unverified, Local, Words};

pub fn state(state: SessionState) -> State {
    match state {
        SessionState::InvalidId => State::InvalidId,
        SessionState::Unknown => State::Unknown,
        SessionState::NotFound => State::NotFound,
        SessionState::LiveLocatable => State::Live,
        SessionState::LiveMultiple => State::LiveMultiple,
        SessionState::LiveNotLocatable => State::LiveNotLocatable,
        SessionState::LiveHeadless => State::LiveHeadless,
        SessionState::Stopped => State::Stopped,
        SessionState::Archived => State::Archived,
        SessionState::LaunchPending => State::LaunchPending,
    }
}

pub fn precision(precision: CorePrecision) -> Precision {
    match precision {
        CorePrecision::ExactConversation => Precision::ExactConversation,
        CorePrecision::RightWindow => Precision::RightWindow,
        CorePrecision::AppOnly => Precision::AppOnly,
    }
}

fn opens_link(action: &CoreAction) -> bool {
    action
        .steps
        .iter()
        .any(|s| matches!(s, Step::OpenUrl { .. }))
}

pub fn action(action: &CoreAction) -> Action {
    match action.kind {
        ActionKind::FocusPane => Action::Focus,
        ActionKind::OpenExisting | ActionKind::Reopen if opens_link(action) => Action::OpenLink,
        ActionKind::OpenExisting | ActionKind::Reopen | ActionKind::BringForward => {
            Action::ShowApplication
        }
        ActionKind::Resume => Action::Resume,
        ActionKind::TakeOver => Action::TakeOver,
        ActionKind::Attach => Action::Attach,
    }
}

/// The app or terminal an action reaches, by its steps.
fn place(action: &CoreAction) -> Option<&'static str> {
    action.steps.iter().rev().find_map(|step| match step {
        Step::FocusPane { terminal, .. } | Step::Launch { terminal, .. } => {
            Some(wording::terminal_name(*terminal))
        }
        Step::OpenUrl { bundle, .. } => App::from_bundle_id(bundle).map(wording::app_name),
        Step::ActivateApp { app } => Some(wording::app_name(*app)),
        Step::Stop { .. } | Step::AwaitExit { .. } => None,
    })
}

/// Where `action` takes the person, by its last step that goes somewhere.
pub fn destination(action: &CoreAction) -> Destination {
    let last = action.steps.iter().rev().find_map(|step| match step {
        Step::FocusPane { .. } | Step::Launch { .. } => Some(Destination::Terminal),
        Step::OpenUrl { .. } => Some(Destination::DesktopApp),
        Step::ActivateApp { app } if app.terminal().is_some() => Some(Destination::Terminal),
        Step::ActivateApp { .. } => Some(Destination::DesktopApp),
        Step::Stop { .. } | Step::AwaitExit { .. } => None,
    });
    last.unwrap_or(Destination::None)
}

/// The destination of a whole plan: one action's, the choices' when they
/// all agree, otherwise none.
fn plan_destination(plan: &Plan) -> Destination {
    match plan {
        Plan::Act(action) => destination(action),
        Plan::Confirm(confirmation) => destination(&confirmation.then),
        Plan::Choose(actions) => {
            let mut kinds = actions.iter().map(destination);
            let first = kinds.next().unwrap_or(Destination::None);
            if kinds.all(|kind| kind == first) {
                first
            } else {
                Destination::None
            }
        }
        Plan::Unavailable(_) => Destination::None,
    }
}

/// The text of the question a `confirm` or `take_over` button asks first.
fn question(view: &PlanView) -> Option<String> {
    let text = match view.action {
        Action::Confirm => view.message.clone().unwrap_or_default(),
        Action::TakeOver => view
            .message
            .clone()
            .unwrap_or_else(|| view.strings.take_over_detail.clone()),
        _ => return None,
    };
    Some(
        std::iter::once(text)
            .chain(view.elsewhere.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The button label, worded as the apps' old labels were.
pub fn title(action: &CoreAction) -> String {
    let at = place(action);
    match self::action(action) {
        Action::Focus => format!("Open in {}", at.unwrap_or("original terminal")),
        Action::ShowApplication => format!("Show {}", at.unwrap_or("application")),
        Action::OpenLink => format!("Open in {}", at.unwrap_or("application")),
        Action::Resume => format!("Resume in {}", at.unwrap_or("Terminal")),
        Action::TakeOver => format!("Stop and resume in {}", at.unwrap_or("Terminal")),
        Action::Attach => format!("Attach in {}", at.unwrap_or("Terminal")),
        Action::Choose => titles::CHOOSE.to_owned(),
        Action::Confirm | Action::Unavailable => titles::UNAVAILABLE.to_owned(),
    }
}

fn app_id(app: App) -> &'static str {
    match app {
        App::ClaudeDesktop => "claude_desktop",
        App::CodexDesktop => "codex_desktop",
        App::OpenCodeDesktop => "opencode_desktop",
        other => other.terminal().map_or("unknown", wording::terminal_id),
    }
}

pub fn choice_id(target: &Target) -> String {
    match target {
        Target::Pane(p) => format!("pane:{}:{}:{}", p.pid, p.start, p.boot.0),
        Target::Desktop(app) => format!("desktop:{}", app_id(*app)),
    }
}

/// The target a [`choice_id`] names.
pub fn target(id: &str) -> Option<Target> {
    if let Some(app) = id.strip_prefix("desktop:") {
        return App::ALL
            .into_iter()
            .find(|a| a.terminal().is_none() && app_id(*a) == app)
            .map(Target::Desktop);
    }
    let mut parts = id.strip_prefix("pane:")?.split(':');
    let identity = ProcessIdentity {
        pid: parts.next()?.parse().ok()?,
        start: parts.next()?.parse().ok()?,
        boot: BootId(parts.next()?.parse().ok()?),
    };
    parts.next().is_none().then_some(Target::Pane(identity))
}

fn tty(pane: &PaneRef) -> Option<&str> {
    match pane {
        PaneRef::Tty { tty } => Some(tty),
        PaneRef::TmuxPane { outer, .. } => tty(outer),
        _ => None,
    }
}

fn label(action: &CoreAction) -> String {
    let name = place(action).unwrap_or("Unknown location");
    let pane = action.steps.iter().find_map(|step| match step {
        Step::FocusPane { pane, .. } => tty(pane),
        _ => None,
    });
    match pane {
        Some(tty) => format!("{name} ({tty})"),
        None => name.to_owned(),
    }
}

fn base(state: State, action: Action, title: String, words: Option<Words>) -> PlanView {
    PlanView {
        state,
        action,
        title,
        enabled: !matches!(action, Action::Unavailable),
        reason_code: words.map(|w| w.0.to_owned()),
        message: words.map(|w| w.1.to_owned()),
        precision: None,
        destination: Destination::None,
        question: None,
        can_resume_default: false,
        pending_launch: false,
        choices: Vec::new(),
        elsewhere: Vec::new(),
        strings: wording::strings(),
    }
}

/// A request refused before the engine ran.
pub fn refused(reason: Local) -> PlanView {
    base(
        State::Unsupported,
        Action::Unavailable,
        titles::UNAVAILABLE.to_owned(),
        Some(wording::local(reason)),
    )
}

fn acting(state: State, action: &CoreAction) -> PlanView {
    let mut view = base(
        state,
        self::action(action),
        title(action),
        action.note.map(wording::reason),
    );
    view.precision = Some(precision(action.precision));
    view
}

/// The button for `decision`. `pending` is the snapshot's reservation;
/// `default` the decision the same look gives with `use_default_terminal`.
pub fn plan(decision: &Decision, pending: &Fact<bool>, default: Option<&Decision>) -> PlanView {
    let state = state(decision.state);
    let mut view = match &decision.plan {
        Plan::Act(action) => acting(state, action),
        Plan::Choose(actions) => {
            let mut view = base(
                state,
                Action::Choose,
                titles::CHOOSE.to_owned(),
                Some(wording::local(Local::SeveralLocations)),
            );
            view.choices = actions
                .iter()
                .filter_map(|a| {
                    a.target.as_ref().map(|t| Choice {
                        id: choice_id(t),
                        label: label(a),
                    })
                })
                .collect();
            view
        }
        Plan::Confirm(confirmation) => {
            let mut view = acting(state, &confirmation.then);
            view.action = Action::Confirm;
            let (code, message) = wording::reason(confirmation.reason);
            view.reason_code = Some(code.to_owned());
            view.message = Some(message.to_owned());
            view.elsewhere = confirmation
                .elsewhere
                .iter()
                .map(|e| {
                    let name = e.terminal.map_or("Another process", wording::terminal_name);
                    e.tty
                        .as_ref()
                        .map_or(name.to_owned(), |t| format!("{name} ({t})"))
                })
                .collect();
            view
        }
        Plan::Unavailable(reason) => base(
            state,
            Action::Unavailable,
            titles::UNAVAILABLE.to_owned(),
            Some(wording::reason(*reason)),
        ),
    };
    view.destination = plan_destination(&decision.plan);
    view.question = question(&view);
    let released = decision.release_reservation;
    view.pending_launch = !released && matches!(pending, Fact::Known(true));
    view.can_resume_default = default.is_some_and(|d| {
        d.action().is_some_and(|a| a.kind == ActionKind::Resume) && d.plan != decision.plan
    });
    view
}

fn dispatched(action: Option<&CoreAction>) -> &'static str {
    match action.map(self::action) {
        Some(Action::Focus) => unverified::FOCUS,
        Some(Action::ShowApplication) => unverified::SHOW,
        Some(Action::OpenLink) => unverified::LINK,
        Some(Action::Attach) => unverified::ATTACH,
        _ => unverified::RESUME,
    }
}

/// What a choice or confirmation the engine stopped at asks.
fn stopped_at(decision: &Decision) -> Words {
    match &decision.plan {
        Plan::Confirm(confirmation) => wording::reason(confirmation.reason),
        _ => wording::local(Local::SeveralLocations),
    }
}

/// What a click did, with the button `after` it.
pub fn open(report: &OpenReport, after: PlanView) -> OpenResult {
    let action = report.decision.action();
    let said = |outcome, (code, message): Words| (outcome, Some(code), message);
    let (outcome, code, message) = match &report.outcome {
        CoreOutcome::Verified => (
            Outcome::Verified,
            None,
            action.map_or(wording::NOTHING_STARTED, |a| {
                wording::precision(a.precision)
            }),
        ),
        CoreOutcome::DispatchedUnverified => {
            (Outcome::DispatchedUnverified, None, dispatched(action))
        }
        CoreOutcome::Failed(reason) => said(Outcome::Failed, wording::reason(*reason)),
        CoreOutcome::Blocked(reason) => said(Outcome::Refused, wording::reason(*reason)),
        CoreOutcome::NeedsChoice => said(Outcome::Refused, stopped_at(&report.decision)),
    };
    OpenResult {
        outcome,
        precision: action.map(|a| precision(a.precision)),
        reason_code: code.map(str::to_owned),
        message: message.to_owned(),
        plan: after,
    }
}

/// `after`, saying why the request around it was refused.
pub fn refused_after((code, message): Words, mut after: PlanView) -> PlanView {
    after.reason_code = Some(code.to_owned());
    after.message = Some(message.to_owned());
    after
}

/// A click refused before the engine ran.
pub fn refused_open((code, message): Words, after: PlanView) -> OpenResult {
    OpenResult {
        outcome: Outcome::Refused,
        precision: None,
        reason_code: Some(code.to_owned()),
        message: message.to_owned(),
        plan: after,
    }
}
