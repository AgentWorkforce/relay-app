//! The JSON both apps exchange with [`super::plan`] and [`super::open`]
//! (GTK in process, macOS through `agent-relay-probe session-open`). Every
//! name is snake_case and every value listed here is stable: the apps
//! branch on `action`, `state`, `outcome` and `reason_code`, never on
//! `title` or `message`.
use serde::{Deserialize, Serialize};

/// One look at, or one click on, a session's Open button.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// `claude`, `codex`, `grok` or `opencode`.
    pub source: String,
    /// The provider's id. Claude: the CLI session id (a UUID), or the
    /// desktop app's `local_…` id, which is mapped to the CLI id its record
    /// names (refused when no single record maps it).
    pub session_id: String,
    /// The app's own data directory, absolute: launch reservations live in
    /// `<app_data_dir>/session-navigation/`, and the pending-resume files
    /// earlier versions wrote there are honoured. macOS:
    /// `AppInfo.dataDirectory`; GTK: the directory of `desktop.json`.
    pub app_data_dir: String,
    /// The directory the session list shows. Informational only: the
    /// engine reads the working directory from the provider's own records.
    #[serde(default)]
    pub directory: Option<String>,
    /// The person asked to resume in the default terminal rather than the
    /// terminal the session was last seen in.
    #[serde(default)]
    pub use_default_terminal: bool,
    /// The person's standing "Open with: Terminal" choice for this
    /// provider (macOS `sessionOpening.<source>`, GTK Settings → Opening
    /// sessions): a stopped Codex thread started in the desktop app, or an
    /// archived Claude desktop session, resumes in a terminal instead of
    /// the app when it can. Never changes a running session, and never
    /// overrides the terminal the session was last seen in. Claude and
    /// Codex only; ignored for Grok and OpenCode.
    #[serde(default)]
    pub prefer_terminal: bool,
    /// The person's terminal choice in Relay ([`TERMINAL_IDS`]); `None`
    /// defers to the operating system's default terminal.
    #[serde(default)]
    pub default_terminal: Option<String>,
    /// The person confirmed the session is not open in the places a
    /// `confirm` answer named.
    #[serde(default)]
    pub confirmed: bool,
    /// The person confirmed an earlier launch (`pending_launch`) was closed
    /// or stopped. On `open` it releases the reservation and opens in one
    /// click; on `plan` it only shows what a click would then do (nothing
    /// is written). The apps call `release` instead and open separately.
    #[serde(default)]
    pub launch_gone: bool,
    /// The person asked to stop a background owner and resume here.
    #[serde(default)]
    pub take_over: bool,
    /// A [`Choice::id`] from an earlier `choose` answer.
    #[serde(default)]
    pub selected: Option<String>,
}

/// The values [`Request::default_terminal`] accepts.
pub const TERMINAL_IDS: [&str; 11] = [
    "ghostty",
    "apple_terminal",
    "iterm2",
    "kitty",
    "wezterm",
    "alacritty",
    "gnome_terminal",
    "konsole",
    "xterm",
    "windows_terminal",
    "console_host",
];

/// What the Open button would do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Select the session's live pane or window.
    Focus,
    /// Bring the application forward; its exact pane cannot be selected.
    ShowApplication,
    /// Send the provider's session link to its desktop app.
    OpenLink,
    /// Start the provider's resume command in a new terminal window.
    Resume,
    /// Stop a background owner, prove it exited, then resume.
    TakeOver,
    /// Open a terminal window attached to the detached tmux session that
    /// holds it. Starts no agent.
    Attach,
    /// Several live places: pick one of `choices`.
    Choose,
    /// Ask the person `message`, then send the request with `confirmed`.
    Confirm,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Running in one place Relay can return to.
    Live,
    LiveMultiple,
    LiveNotLocatable,
    LiveHeadless,
    Stopped,
    Archived,
    NotFound,
    LaunchPending,
    Unknown,
    InvalidId,
    /// The request never reached the engine (unknown source, bad request).
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    ExactConversation,
    RightWindow,
    AppOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// A fresh observation proved the post-condition.
    Verified,
    /// Sent; nothing observable proved it worked (and nothing proved it
    /// failed).
    DispatchedUnverified,
    /// Nothing was dispatched: unavailable, needs a choice or confirmation,
    /// already being opened, or Automation was declined.
    Refused,
    /// A step failed or the wrong thing came forward.
    Failed,
}

/// Where a click on the button takes the person, for its icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// A terminal: a pane focused, a window opened, or a terminal app
    /// brought forward.
    Terminal,
    /// A provider's desktop app (Claude, Codex, OpenCode).
    DesktopApp,
    /// Nothing (unavailable), or places of both kinds (`choose`).
    #[default]
    None,
}

/// Every word the apps show around the button that no single answer
/// carries: the button before and while an answer is awaited, the
/// follow-up controls and dialogs, and what to say when no answer could be
/// had at all. The same table is in every [`PlanView`]; `session-open
/// strings` prints it, and `shared/session-open-strings.json` is that
/// output (the macOS app's fallback copy, for when its helper is missing,
/// is tested against the file).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strings {
    /// The button before its first answer.
    pub open_session: String,
    /// Said while the first answer is awaited.
    pub checking: String,
    /// The button while an open runs.
    pub opening: String,
    /// The button offered when `can_resume_default` applies.
    pub resume_default: String,
    /// The button offered for a pending launch.
    pub resolve_launch: String,
    /// The pending-launch question, its explanation and its two answers.
    pub launch_question: String,
    pub launch_detail: String,
    pub launch_closed: String,
    pub launch_wait: String,
    /// The declining button of the confirm and take-over questions.
    pub cancel: String,
    /// The take-over question's text when the answer has no `message`.
    pub take_over_detail: String,
    /// macOS: the app bundle has no session helper.
    pub helper_missing: String,
    /// macOS: the helper refused the request (exit 2).
    pub rejected: String,
    /// No answer to a look (`plan`) or a release: nothing was opened.
    pub plan_failed: String,
    /// No answer to an open (timed out, the helper or worker died, or the
    /// answer was unreadable): whether the session opened is unknown.
    pub open_unknown: String,
    /// The open could not be started at all: nothing was opened.
    pub open_not_started: String,
    /// Another open (or release) of this session is running.
    pub already_opening: String,
}

impl Default for Strings {
    fn default() -> Self {
        super::wording::strings()
    }
}

/// One live place a `choose` answer offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Opaque; send it back as [`Request::selected`].
    pub id: String,
    /// "Ghostty", "Ghostty (/dev/ttys004)", "Claude", …
    pub label: String,
}

/// The Open button's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub state: State,
    pub action: Action,
    /// The button label.
    pub title: String,
    pub enabled: bool,
    /// Why the plan is refused or reduced, or what a `choose` / `confirm`
    /// asks; absent for a plain action.
    pub reason_code: Option<String>,
    /// One sentence for the person, paired with `reason_code`.
    pub message: Option<String>,
    /// The precision the action would reach.
    pub precision: Option<Precision>,
    /// Where a click goes: the button shows the terminal icon exactly when
    /// this is `terminal`.
    #[serde(default)]
    pub destination: Destination,
    /// For `confirm` and `take_over`: the question to ask before sending
    /// `confirmed` / `take_over`, with `elsewhere` listed one per line. Its
    /// confirming button says `title`; the other says `strings.cancel`.
    #[serde(default)]
    pub question: Option<String>,
    /// The same request with `use_default_terminal` would resume.
    pub can_resume_default: bool,
    /// A launch reservation for this session is unresolved: ask
    /// `strings.launch_question`, then call `release` (it only clears the
    /// reservation; the person then clicks Open).
    pub pending_launch: bool,
    pub choices: Vec<Choice>,
    /// For `confirm`: the places the session may already be open.
    pub elsewhere: Vec<String>,
    /// The apps' fixed words ([`Strings`]); this build's own when an
    /// answer has none.
    #[serde(default)]
    pub strings: Strings,
}

/// What one click did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenResult {
    pub outcome: Outcome,
    /// What the dispatched action reaches when it works.
    pub precision: Option<Precision>,
    pub reason_code: Option<String>,
    /// One sentence for the person.
    pub message: String,
    /// The button after the attempt, from a fresh look.
    pub plan: PlanView,
}
