//! The Open button over the answers of `agent_relay_desktop_core::session_open`
//! (GTK calls its `plan`, `open` and `release_launch` in process, off the
//! main thread). This decides what the button shows and what a click
//! sends; every sentence and button title it shows comes from the engine:
//! an answer's own words, or its fixed words ([`open_strings`]) when no
//! answer could be had, so the Mac shows the same. It never decides
//! whether a session may be launched: the engine does, on every click. One
//! open or release per session runs at a time. The Mac's
//! `SessionOpenModel` is the reference.

use agent_relay_desktop_core::session_open::contract::{
    Action, Choice, Destination, OpenResult, Outcome, PlanView, Request, Strings,
};
use agent_relay_desktop_core::session_open::wording;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

/// The providers the engine opens sessions of.
pub const OPENABLE_SOURCES: [&str; 4] = ["claude", "codex", "grok", "opencode"];

/// How often a shown button is looked at again while the window is active:
/// the Mac's `SessionOpenModel.pollInterval`. A look is also taken when a
/// session is shown, when the window becomes active, and after every open
/// or release (their own answers).
pub const PLAN_EVERY: Duration = Duration::from_secs(30);

/// The engine's fixed words: the same table every answer carries and the
/// Mac falls back to (`shared/session-open-strings.json`).
pub fn open_strings() -> Strings {
    wording::strings()
}

/// What the person chose, sent back with the next request. A pending
/// launch is released by its own call, never by a flag here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenResend {
    pub use_default_terminal: bool,
    pub confirmed: bool,
    pub take_over: bool,
    pub selected: Option<String>,
}

/// One session as the engine names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget {
    pub source: String,
    pub session_id: String,
    /// What the session list shows; informational to the engine.
    pub directory: Option<String>,
}

impl OpenTarget {
    pub fn new(source: &str, session_id: &str, directory: &str) -> OpenTarget {
        OpenTarget {
            source: source.into(),
            session_id: session_id.into(),
            directory: (!directory.is_empty()).then(|| directory.into()),
        }
    }

    /// The request for one look or click. `app_data_dir` holds
    /// `desktop.json`; `default_terminal` is the person's terminal choice in
    /// Relay, `None` for the system's default terminal; `prefer_terminal`
    /// their "Open with: Terminal" for this provider.
    pub fn request(
        &self,
        app_data_dir: &str,
        default_terminal: Option<&str>,
        prefer_terminal: bool,
        resend: &OpenResend,
    ) -> Request {
        Request {
            source: self.source.clone(),
            session_id: self.session_id.clone(),
            app_data_dir: app_data_dir.into(),
            directory: self.directory.clone(),
            use_default_terminal: resend.use_default_terminal,
            prefer_terminal,
            default_terminal: default_terminal.map(str::to_owned),
            confirmed: resend.confirmed,
            launch_gone: false,
            take_over: resend.take_over,
            selected: resend.selected.clone(),
        }
    }
}

/// The app's data directory: the one that holds `desktop.json` (`prefs`).
/// A relative or rootless path is passed on as it is; the engine refuses it
/// with its own words.
pub fn open_app_data_dir(prefs: &Path) -> String {
    prefs
        .parent()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a click asks for, given the answer the button shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenClick {
    /// Run `open` with these choices.
    Open(OpenResend),
    /// Ask which of `choices`, then open with the chosen id as `selected`.
    Choose {
        base: OpenResend,
        title: String,
        message: String,
        choices: Vec<Choice>,
    },
    /// Ask `question` under `title`, then open with `confirmed` (or, for a
    /// take-over, with `take_over`); the confirming button says `title`.
    Confirm {
        base: OpenResend,
        title: String,
        question: String,
        take_over: bool,
    },
    /// The button is unavailable: nothing is sent.
    Nothing,
}

impl OpenClick {
    /// The click `plan` asks for, building on `base`. Without a plan yet
    /// nothing is sent: the button waits for its first answer.
    pub fn of(plan: Option<&PlanView>, base: OpenResend) -> OpenClick {
        let Some(plan) = plan.filter(|plan| plan.enabled) else {
            return OpenClick::Nothing;
        };
        let confirm = |take_over: bool| OpenClick::Confirm {
            base: base.clone(),
            title: plan.title.clone(),
            question: plan.question.clone().unwrap_or_else(|| {
                if take_over {
                    plan.strings.take_over_detail.clone()
                } else {
                    plan.message.clone().unwrap_or_default()
                }
            }),
            take_over,
        };
        match plan.action {
            Action::Choose => OpenClick::Choose {
                base,
                title: plan.title.clone(),
                message: plan.message.clone().unwrap_or_default(),
                choices: plan.choices.clone(),
            },
            Action::Confirm => confirm(false),
            // Stopping the background owner is asked first, as on the Mac.
            Action::TakeOver => confirm(true),
            Action::Unavailable => OpenClick::Nothing,
            Action::Focus
            | Action::ShowApplication
            | Action::OpenLink
            | Action::Resume
            | Action::Attach => OpenClick::Open(base),
        }
    }

    /// The question an open stopped at: a choice or confirmation the engine
    /// needs before it dispatches anything, asked straight away.
    pub fn after(result: &OpenResult, sent: &OpenResend) -> OpenClick {
        let asks = matches!(result.plan.action, Action::Choose | Action::Confirm);
        if result.outcome != Outcome::Refused || !asks {
            return OpenClick::Nothing;
        }
        // What the person already chose is sent again with the reply.
        OpenClick::of(Some(&result.plan), sent.clone())
    }

    /// `base` with the person's choice of `id`.
    pub fn chosen(base: &OpenResend, id: &str) -> OpenResend {
        OpenResend {
            selected: Some(id.into()),
            ..base.clone()
        }
    }

    /// `base`, confirmed: `confirmed`, or `take_over` for a take-over.
    pub fn confirmed(base: &OpenResend, take_over: bool) -> OpenResend {
        OpenResend {
            confirmed: base.confirmed || !take_over,
            take_over: base.take_over || take_over,
            ..base.clone()
        }
    }
}

/// What the Open controls show for one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenView {
    pub title: String,
    pub sensitive: bool,
    /// No answer yet for this session.
    pub checking: bool,
    pub opening: bool,
    pub releasing: bool,
    /// The button shows the terminal icon (`destination` is `terminal`).
    pub terminal: bool,
    /// The answer's sentence under the button (or, before the first
    /// answer, the engine's "checking" words).
    pub reason: Option<String>,
    /// What the last click did, or why no answer could be had.
    pub notice: Option<String>,
    /// An earlier launch is unresolved: ask whether it was closed.
    pub pending_launch: bool,
    /// Offer resuming in the default terminal.
    pub offer_default: bool,
    /// The fixed words for the controls around the button.
    pub strings: Strings,
}

/// The Open button's answers and clicks, per session (`LocalSessionRow::key`).
#[derive(Debug, Default)]
pub struct SessionOpens {
    plans: HashMap<String, PlanView>,
    notices: HashMap<String, String>,
    /// Why the last look had no answer.
    read_errors: HashMap<String, String>,
    /// An open failed where resuming in the default terminal would work.
    permits_default: HashSet<String>,
    planning: HashMap<String, u64>,
    opening: HashSet<String>,
    releasing: HashSet<String>,
    /// Bumped by each open or release: a look begun before it is older
    /// than its answer.
    epochs: HashMap<String, u64>,
}

/// A look begun for one session; its answer is kept only if no open
/// overtook it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanTicket(u64);

impl SessionOpens {
    fn epoch(&self, key: &str) -> u64 {
        self.epochs.get(key).copied().unwrap_or_default()
    }

    fn busy(&self, key: &str) -> bool {
        self.opening.contains(key) || self.releasing.contains(key)
    }

    /// The fixed words: the last answer's, else this build's.
    fn strings(&self, key: &str) -> Strings {
        self.plans
            .get(key)
            .map_or_else(open_strings, |plan| plan.strings.clone())
    }

    /// Starts a look at `key`; `None` while one, an open or a release runs.
    pub fn begin_plan(&mut self, key: &str) -> Option<PlanTicket> {
        if self.busy(key) || self.planning.contains_key(key) {
            return None;
        }
        let epoch = self.epoch(key);
        self.planning.insert(key.into(), epoch);
        Some(PlanTicket(epoch))
    }

    /// The look `ticket` answered `plan` (`None`: it ended without one,
    /// which is said and keeps what was shown).
    pub fn planned(&mut self, key: &str, ticket: PlanTicket, plan: Option<PlanView>) {
        if self.planning.get(key) == Some(&ticket.0) {
            self.planning.remove(key);
        }
        if ticket.0 != self.epoch(key) {
            return;
        }
        match plan {
            Some(plan) => {
                self.read_errors.remove(key);
                self.plans.insert(key.into(), plan);
            }
            None => {
                let said = self.strings(key).plan_failed;
                self.read_errors.insert(key.into(), said);
            }
        }
    }

    fn begin(&mut self, key: &str) {
        *self.epochs.entry(key.into()).or_default() += 1;
        self.planning.remove(key);
        self.notices.remove(key);
        self.permits_default.remove(key);
    }

    /// Starts an open of `key`; false while one or a release already runs.
    pub fn begin_open(&mut self, key: &str) -> bool {
        if self.busy(key) {
            return false;
        }
        self.opening.insert(key.into());
        self.begin(key);
        true
    }

    /// The open of `key` answered `result` (`None`: it ended without an
    /// answer, so whether it opened is unknown: that is said, the button
    /// works again, and the caller looks again; it is never retried). The
    /// question it stopped at, if any, is asked next.
    pub fn opened(
        &mut self,
        key: &str,
        sent: &OpenResend,
        result: Option<OpenResult>,
    ) -> OpenClick {
        self.opening.remove(key);
        let Some(result) = result else {
            let said = self.strings(key).open_unknown;
            self.notices.insert(key.into(), said);
            return OpenClick::Nothing;
        };
        let next = OpenClick::after(&result, sent);
        if !result.message.is_empty() {
            self.notices.insert(key.into(), result.message);
        }
        if matches!(result.outcome, Outcome::Refused | Outcome::Failed)
            && result.plan.can_resume_default
        {
            self.permits_default.insert(key.into());
        }
        self.read_errors.remove(key);
        self.plans.insert(key.into(), result.plan);
        next
    }

    /// Starts releasing the pending launch of `key`; false while an open
    /// or a release already runs.
    pub fn begin_release(&mut self, key: &str) -> bool {
        if self.busy(key) {
            return false;
        }
        self.releasing.insert(key.into());
        self.begin(key);
        true
    }

    /// The release of `key` answered `plan` (`None`: no answer, which is
    /// said; the caller looks again). Nothing was opened either way.
    pub fn released(&mut self, key: &str, plan: Option<PlanView>) {
        self.releasing.remove(key);
        match plan {
            Some(plan) => {
                self.read_errors.remove(key);
                self.plans.insert(key.into(), plan);
            }
            None => {
                let said = self.strings(key).plan_failed;
                self.notices.insert(key.into(), said);
            }
        }
    }

    /// Forgets what was shown for `key`, as selecting a session does. An
    /// open or release still running keeps its place.
    pub fn forget(&mut self, key: &str) {
        if self.busy(key) {
            return;
        }
        self.plans.remove(key);
        self.notices.remove(key);
        self.read_errors.remove(key);
        self.permits_default.remove(key);
        self.planning.remove(key);
        *self.epochs.entry(key.into()).or_default() += 1;
    }

    pub fn is_opening(&self, key: &str) -> bool {
        self.opening.contains(key)
    }

    pub fn plan(&self, key: &str) -> Option<&PlanView> {
        self.plans.get(key)
    }

    /// What a click on the button of `key` asks for now.
    pub fn click(&self, key: &str) -> OpenClick {
        if self.busy(key) {
            return OpenClick::Nothing;
        }
        OpenClick::of(self.plan(key), OpenResend::default())
    }

    pub fn view(&self, key: &str) -> OpenView {
        let opening = self.is_opening(key);
        let releasing = self.releasing.contains(key);
        let plan = self.plan(key);
        let strings = self.strings(key);
        let pending_launch = plan.is_some_and(|plan| {
            plan.pending_launch && plan.reason_code.as_deref() == Some("launch_pending")
        });
        let reason = match plan {
            Some(plan) => plan.message.clone(),
            None => Some(strings.checking.clone()),
        };
        // Said once: a refusal's sentence is often the new answer's too.
        let notice = self
            .notices
            .get(key)
            .or_else(|| self.read_errors.get(key))
            .filter(|notice| reason.as_ref() != Some(*notice))
            .cloned();
        let title = match plan {
            _ if opening => strings.opening.clone(),
            Some(plan) => plan.title.clone(),
            None => strings.open_session.clone(),
        };
        let offer_default = plan.is_some_and(|plan| {
            plan.can_resume_default
                && !pending_launch
                && (plan.action == Action::Unavailable || self.permits_default.contains(key))
        });
        OpenView {
            title,
            sensitive: !opening && !releasing && plan.is_some_and(|plan| plan.enabled),
            checking: plan.is_none(),
            opening,
            releasing,
            terminal: plan.is_some_and(|plan| plan.destination == Destination::Terminal),
            reason,
            notice,
            pending_launch,
            offer_default,
            strings,
        }
    }
}

#[cfg(test)]
mod tests;
