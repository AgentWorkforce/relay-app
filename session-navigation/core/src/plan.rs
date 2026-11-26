//! Plans are data. A plan says what to dispatch, how precise the result would
//! be, and which observation would prove it worked.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hosts::PaneRef;
use crate::model::{App, Precision, ProcessIdentity, Reason, Runtime, TerminalKind};

/// A process to start: an argv vector, never a shell string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
}

/// Where and how a launched command runs, beyond its argv.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchContext {
    /// The session's own runtime: `Native`, or `Wsl` (the executor runs
    /// `wsl.exe -d <distro> --cd <cwd> -- <program> <args…>`). Never
    /// `Foreign`.
    pub runtime: Runtime,
    /// The Windows Terminal profile to open the window with (`wt new-tab
    /// --profile`), preserved from the session's own window.
    pub profile: Option<String>,
}

impl LaunchContext {
    pub fn native() -> Self {
        Self {
            runtime: Runtime::Native,
            profile: None,
        }
    }
}

/// One dispatchable step. There is deliberately no step that types text into
/// a pane or pre-fills a prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Focus `pane` of `terminal`, which hosted `process` when observed. For
    /// a window ref, raise that window; for a tmux pane, select it through
    /// its client, then focus the client's own pane (`outer`) in `terminal`.
    /// The executor re-checks every identity the ref carries first.
    FocusPane {
        terminal: TerminalKind,
        pane: PaneRef,
        process: ProcessIdentity,
    },
    /// Open a provider URL; `bundle` is the app expected to handle it.
    OpenUrl { url: String, bundle: String },
    /// Bring an application forward (launching it if needed).
    ActivateApp { app: App },
    /// Start `command` in a new window of `terminal`, in `context`.
    Launch {
        terminal: TerminalKind,
        command: Command,
        context: LaunchContext,
    },
    /// Ask a background owner to stop (take-over only).
    Stop { process: ProcessIdentity },
    /// Observe that `process` exited and nothing replaced it before going on.
    AwaitExit { process: ProcessIdentity },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// Focus a live pane.
    FocusPane,
    /// Open a desktop conversation that still exists there.
    OpenExisting,
    /// Revive an archived desktop conversation through a provider link.
    Reopen,
    /// Start a resume command in a terminal.
    Resume,
    /// Stop a background owner, verify the exit, then resume.
    TakeOver,
    /// Bring an app forward at reduced precision.
    BringForward,
    /// Open a new terminal window attached to a detached tmux session that
    /// hosts the session. Starts no agent process.
    Attach,
}

/// What must be observed afterwards to call the plan verified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostCondition {
    PaneFocused {
        terminal: TerminalKind,
        pane: PaneRef,
        process: ProcessIdentity,
    },
    ConversationShown {
        app: App,
    },
    /// A fresh decision focuses the session in a pane of `terminal`, found
    /// by tty and the kernel process table only. Never by the Ghostty
    /// terminal's `pid` (the wrapping `/usr/bin/login`, not the command) nor
    /// its `working directory` (empty right after launch; native N18).
    RunningIn {
        terminal: TerminalKind,
    },
    AppFrontmost {
        app: App,
    },
    /// A client of the tmux server at `socket` (still `server`) shows
    /// `pane`, which still holds `process`, and that client's pane is
    /// frontmost in `terminal`.
    TmuxAttached {
        terminal: TerminalKind,
        socket: String,
        server: ProcessIdentity,
        pane: String,
        process: ProcessIdentity,
    },
}

/// What a person can pick when several live locations exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Pane(ProcessIdentity),
    Desktop(App),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
    pub steps: Vec<Step>,
    pub precision: Precision,
    pub post: PostCondition,
    /// Why precision was reduced, when it was.
    pub note: Option<Reason>,
    /// The step may create a new provider session from an existing transcript.
    pub import: bool,
    pub target: Option<Target>,
}

impl Action {
    /// Whether this action may start an agent process for the session.
    pub fn starts_process(&self) -> bool {
        matches!(
            self.kind,
            ActionKind::Reopen | ActionKind::Resume | ActionKind::TakeOver
        )
    }

    /// Whether this action may make some process start writing the session:
    /// opening an existing desktop conversation can make the app load it.
    pub fn may_start_writer(&self) -> bool {
        self.starts_process() || self.kind == ActionKind::OpenExisting
    }
}

/// Another process that may be showing the session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elsewhere {
    pub process: ProcessIdentity,
    pub terminal: Option<TerminalKind>,
    pub tty: Option<String>,
}

/// An action that is safe only if the person confirms the session is not
/// open in the processes named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub reason: Reason,
    pub elsewhere: Vec<Elsewhere>,
    /// What runs once the request carries `confirm_not_open_elsewhere`.
    pub then: Action,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plan {
    Act(Action),
    Choose(Vec<Action>),
    /// `ConfirmNotOpenElsewhere`: ask the person, then proceed.
    Confirm(Confirmation),
    Unavailable(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    InvalidId,
    Unknown,
    NotFound,
    LiveLocatable,
    LiveMultiple,
    LiveNotLocatable,
    LiveHeadless,
    Stopped,
    Archived,
    LaunchPending,
}

impl SessionState {
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::LiveLocatable | Self::LiveMultiple | Self::LiveNotLocatable | Self::LiveHeadless
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub state: SessionState,
    pub plan: Plan,
    /// The snapshot shows an earlier launch resolved: release its reservation.
    pub release_reservation: bool,
}

impl Decision {
    pub fn new(state: SessionState, plan: Plan) -> Self {
        Self {
            state,
            plan,
            release_reservation: false,
        }
    }

    pub fn unavailable(state: SessionState, reason: Reason) -> Self {
        Self::new(state, Plan::Unavailable(reason))
    }

    pub fn action(&self) -> Option<&Action> {
        match &self.plan {
            Plan::Act(action) => Some(action),
            _ => None,
        }
    }
}

/// Standing preferences of the person.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// The terminal the person explicitly chose in Relay, if they did.
    /// `None` defers to the operating system's default
    /// ([`crate::evidence::Snapshot::default_terminal`]); there is no
    /// built-in terminal.
    pub default_terminal: Option<TerminalKind>,
    /// Allow plans that import a transcript as a new desktop session.
    pub allow_import: bool,
    /// The person prefers a terminal to the provider's desktop app: a
    /// stopped session the plan would open in Codex (a desktop-started,
    /// unarchived thread) or revive in Claude (an archived desktop record),
    /// or would refuse to open there, is resumed with the provider's CLI in
    /// a terminal instead, when that resume is possible; otherwise the plan
    /// is unchanged (the preference never turns a plan into a refusal).
    /// The terminal is chosen as for any resume
    /// ([`crate::decide::common::terminal_choice`]): a known last-seen
    /// terminal host is never overridden. It never changes the plan for a
    /// live session (a take-over included) and never resumes a conversation
    /// an unarchived Claude desktop record holds (the app may revive it: a
    /// second writer). Grok and OpenCode ignore it.
    pub prefer_terminal: bool,
}

/// One click on Open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub prefs: Preferences,
    /// Explicitly asked to stop a background owner and resume interactively.
    pub take_over: bool,
    /// Explicitly asked to resume in the default terminal.
    pub use_default_terminal: bool,
    /// The person confirmed an earlier launch is gone.
    pub confirm_launch_gone: bool,
    /// A location picked from an earlier choice.
    pub selected: Option<Target>,
    /// The person confirmed the session is not open in the processes an
    /// earlier `Plan::Confirm` named.
    pub confirm_not_open_elsewhere: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Verified,
    DispatchedUnverified,
    Failed(Reason),
    Blocked(Reason),
    NeedsChoice,
}

/// What an executor reports. Dispatch is never proof of success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dispatch {
    Dispatched,
    /// A launch opened a new window whose pane has this tty (Ghostty's `new
    /// window with configuration` reports it within ~1.5 s, VERIFIED 1.4.0,
    /// native N17; Terminal's `do script` returns the new tab).
    Opened {
        tty: String,
    },
    Refused,
    Failed(Reason),
}
