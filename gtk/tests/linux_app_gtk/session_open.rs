//! The transcript's Open button over a fake engine: the app's Desktop seam
//! stands in for `agent_relay_desktop_core::session_open`, so no case looks
//! at or opens anything on the machine. Every word checked is the answer's.
use super::*;
use agent_relay_app::models::{open_strings, LocalSessionRow};
use agent_relay_app::ui::desktop::{Picked, SessionOpened, SessionPlanned};
use agent_relay_desktop_core::session_open::contract::{OpenResult, PlanView, Request};

type PlanAnswer = Rc<dyn Fn(&Request) -> PlanView>;
type OpenAnswer = Rc<dyn Fn(&Request) -> Option<OpenResult>>;

/// The engine as the app sees it. By default a look never answers (the
/// button keeps "Open session" and sends nothing), an open dispatches a
/// resume, and a release answers a resumable plan.
#[derive(Default)]
pub(super) struct FakeEngine {
    pub plan: RefCell<Option<PlanAnswer>>,
    pub open: RefCell<Option<OpenAnswer>>,
    pub plans: RefCell<Vec<Request>>,
    pub opens: RefCell<Vec<Request>>,
    pub releases: RefCell<Vec<Request>>,
    pub open_delay: Cell<Duration>,
    /// What each choice asked: its title, sentence and labels.
    pub picks: RefCell<Vec<(String, String, Vec<String>)>>,
    pub pick_answer: Cell<Option<usize>>,
}

pub(super) fn plan(action: &str, extra: Value) -> PlanView {
    let mut plan = json!({
        "state": "stopped", "action": action, "title": "Resume in Terminal",
        "enabled": action != "unavailable", "reason_code": null, "message": null,
        "precision": "right_window", "can_resume_default": false, "pending_launch": false,
        "choices": [], "elsewhere": []
    });
    if let (Some(plan), Value::Object(extra)) = (plan.as_object_mut(), extra) {
        plan.extend(extra);
    }
    serde_json::from_value(plan).unwrap()
}

pub(super) fn opened(outcome: &str, message: &str, after: PlanView) -> OpenResult {
    serde_json::from_value(json!({"outcome": outcome, "precision": null,
        "reason_code": null, "message": message, "plan": after}))
    .unwrap()
}

const SENT: &str =
    "Sent the resume request for this session. The provider has not confirmed it opened.";

impl FakeEngine {
    pub fn answer_plans(&self, answer: impl Fn(&Request) -> PlanView + 'static) {
        *self.plan.borrow_mut() = Some(Rc::new(answer));
    }

    pub fn answer_opens(&self, answer: impl Fn(&Request) -> Option<OpenResult> + 'static) {
        *self.open.borrow_mut() = Some(Rc::new(answer));
    }

    pub fn plan_session(&self, request: Request) -> SessionPlanned {
        self.plans.borrow_mut().push(request.clone());
        match self.plan.borrow().clone() {
            Some(answer) => {
                let plan = answer(&request);
                Box::pin(async move { Some(plan) })
            }
            None => Box::pin(std::future::pending()),
        }
    }

    pub fn open_session(&self, request: Request) -> SessionOpened {
        self.opens.borrow_mut().push(request.clone());
        let result = match self.open.borrow().clone() {
            Some(answer) => answer(&request),
            None => Some(opened(
                "dispatched_unverified",
                SENT,
                plan("resume", json!({})),
            )),
        };
        let delay = self.open_delay.get();
        Box::pin(async move {
            if !delay.is_zero() {
                glib::timeout_future(delay).await;
            }
            result
        })
    }

    pub fn release_session(&self, request: Request) -> SessionPlanned {
        self.releases.borrow_mut().push(request);
        let plan = plan("resume", json!({"title": "Resume in Kitty"}));
        Box::pin(async move { Some(plan) })
    }

    pub fn pick(&self, title: &str, detail: &str, options: Vec<String>) -> Picked {
        self.picks
            .borrow_mut()
            .push((title.into(), detail.into(), options));
        let answer = self.pick_answer.get();
        Box::pin(async move { answer })
    }
}

fn row(source: &str, id: &str) -> LocalSessionRow {
    LocalSessionRow {
        upload: Some(
            serde_json::from_value(upload_row(
                source,
                id,
                "Session",
                "/w/api",
                false,
                "not_shared",
            ))
            .unwrap(),
        ),
        relay: None,
    }
}

/// Whether a visible label under the transcript reads `text`.
fn shown(case: &Case, text: &str) -> bool {
    let shell = case.app.shell().unwrap();
    let mut found = false;
    walk(
        shell.pages.sessions.transcript.root.upcast_ref(),
        &mut |widget| {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                found |= label.is_visible() && label.text() == text;
            }
        },
    );
    found
}

/// The fallback and pending-launch controls, when visible.
fn visible_button(case: &Case, label: &str) -> Option<gtk::Button> {
    let shell = case.app.shell().unwrap();
    buttons(&shell.pages.sessions.transcript.root, label)
        .into_iter()
        .find(|button| button.is_visible())
}

pub(super) fn session_open_asks_the_engine_and_draws_its_answer(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.desktop.engine.answer_plans(|request| {
        assert_eq!(
            request.source, "opencode",
            "OpenCode sessions are opened too"
        );
        plan(
            "resume",
            json!({"title": "Resume in Kitty", "message": "This may open a second view."}),
        )
    });
    case.sign_in();
    case.app.open_local_session(row("opencode", "ses_1"));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    case.pump(|_| labels(&page.open) == vec!["Resume in Kitty".to_string()]);
    assert!(shown(&case, "This may open a second view."));
    let look = case.desktop.engine.plans.borrow()[0].clone();
    assert_eq!(look.session_id, "ses_1");
    assert_eq!(look.directory.as_deref(), Some("/w/api"));
    assert_eq!(
        std::path::Path::new(&look.app_data_dir),
        case.prefs_path.parent().unwrap()
    );
    // No terminal is chosen in Relay here: the system's default is used.
    assert_eq!(look.default_terminal, None);
    assert!(!look.use_default_terminal && !look.confirmed && !look.launch_gone && !look.take_over);
    assert!(!look.prefer_terminal, "OpenCode has no Terminal preference");
    // Refresh looks again; Open asks the engine to act, with nothing extra.
    page.refresh.emit_clicked();
    case.pump(|case| case.desktop.engine.plans.borrow().len() == 2);
    page.open.emit_clicked();
    case.pump(|_| shown(&case, SENT));
    let open = case.desktop.engine.opens.borrow()[0].clone();
    assert_eq!(open, look);
    assert_eq!(labels(&page.open), vec!["Resume in Terminal".to_string()]);
    // The engine keeps reservations: the app writes no pending-resume file.
    assert!(!case
        .prefs_path
        .with_file_name("pending-session-resumes.json")
        .exists());
    assert!(case.called("uploader.sessions.include").is_empty());
}

pub(super) fn session_open_runs_one_open_at_a_time_and_keeps_each_answer_with_its_session(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    case.desktop
        .engine
        .open_delay
        .set(Duration::from_millis(150));
    case.sign_in();
    case.app.open_local_session(row("claude", "one"));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    // Before its first answer the button says so and sends nothing.
    assert_session_open_action(&page.open, "claudecode", &open_strings().open_session);
    assert!(!page.open.is_sensitive());
    assert!(shown(&case, &open_strings().checking));
    page.open.emit_clicked();
    case.settle();
    assert!(case.desktop.engine.opens.borrow().is_empty());
    case.desktop
        .engine
        .answer_plans(|_| plan("resume", json!({})));
    case.app.open_local_session(row("claude", "one"));
    case.pump(|_| page.open.is_sensitive());
    page.open.emit_clicked();
    assert!(!page.open.is_sensitive(), "disabled while opening");
    assert_eq!(labels(&page.open), vec![open_strings().opening]);
    page.open.emit_clicked();
    page.open.emit_clicked();
    case.settle();
    assert_eq!(case.desktop.engine.opens.borrow().len(), 1);
    // Another session shown meanwhile is not drawn with the first's answer.
    case.app.open_local_session(row("claude", "two"));
    std::thread::sleep(Duration::from_millis(180));
    case.settle();
    assert!(!shown(&case, SENT));
    case.pump(|_| page.open.is_sensitive());
    page.open.emit_clicked();
    case.pump(|_| shown(&case, SENT));
    let opens = case.desktop.engine.opens.borrow();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[1].session_id, "two");
    drop(opens);
    case.pump(|_| page.open.is_sensitive());
    assert_session_open_action(&page.open, "claudecode", "Resume in Terminal");
}

pub(super) fn session_open_without_an_answer_shows_the_button_again(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    let failed = "The request could not be sent to the application. No new session was started.";
    case.desktop
        .engine
        .answer_plans(|_| plan("resume", json!({"title": "Resume in Kitty"})));
    case.desktop.engine.answer_opens(move |request| {
        (request.session_id == "said").then(|| opened("failed", failed, plan("resume", json!({}))))
    });
    case.sign_in();
    case.app.open_local_session(row("claude", "lost"));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    case.pump(|_| page.open.is_sensitive());
    page.open.emit_clicked();
    // The worker ended without an answer: said in the engine's words, the
    // button works again, it is looked at again, and nothing is retried.
    case.pump(|_| shown(&case, &open_strings().open_unknown));
    case.pump(|case| case.desktop.engine.plans.borrow().len() == 2);
    case.pump(|_| page.open.is_sensitive());
    assert_session_open_action(&page.open, "claudecode", "Resume in Kitty");
    assert_eq!(case.desktop.engine.opens.borrow().len(), 1);
    assert!(!shown(&case, SENT));
    case.app.open_local_session(row("claude", "said"));
    case.pump(|_| page.open.is_sensitive());
    page.open.emit_clicked();
    case.pump(|_| shown(&case, failed));
    assert!(page.open.is_sensitive());
    assert!(case.desktop.opened.borrow().is_empty());
}

pub(super) fn session_open_offers_the_default_terminal_only_when_the_engine_does(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    let refused = "No default terminal is set. Choose one in Settings, then try again.";
    case.desktop.engine.answer_plans(|_| {
        plan("unavailable", json!({"title": "Session opening unavailable", "can_resume_default": true,
            "reason_code": "host_unsupported", "message": "Agent Relay cannot open sessions in this terminal. No new session was started."}))
    });
    case.desktop.engine.answer_opens(move |_| {
        Some(opened(
            "refused",
            refused,
            plan(
                "unavailable",
                json!({"title": "Session opening unavailable"}),
            ),
        ))
    });
    case.sign_in();
    case.app.open_local_session(row("claude", "one"));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    case.pump(|case| visible_button(case, "Resume in default terminal").is_some());
    assert!(!page.open.is_sensitive());
    page.open.emit_clicked();
    case.settle();
    assert!(
        case.desktop.engine.opens.borrow().is_empty(),
        "an unavailable answer sends nothing"
    );
    visible_button(&case, "Resume in default terminal")
        .unwrap()
        .emit_clicked();
    case.pump(|_| shown(&case, refused));
    assert!(case.desktop.engine.opens.borrow()[0].use_default_terminal);
    assert!(
        visible_button(&case, "Resume in default terminal").is_none(),
        "the fresh answer no longer offers it"
    );
}

pub(super) fn session_open_releases_an_earlier_launch_only_when_told_and_opens_nothing(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    let pending = "A resume request was already sent. The provider has not confirmed it opened. Check the earlier launch before trying again.";
    case.desktop.engine.answer_plans(move |_| {
        plan("unavailable", json!({"state": "launch_pending", "title": "Session opening unavailable",
            "pending_launch": true, "reason_code": "launch_pending", "message": pending, "can_resume_default": true}))
    });
    case.sign_in();
    case.app.open_local_session(row("claude", "one"));
    case.pump(|_| shown(&case, pending));
    let words = open_strings();
    assert!(visible_button(&case, &words.resume_default).is_none());
    case.desktop.answer.set(false);
    visible_button(&case, &words.resolve_launch)
        .unwrap()
        .emit_clicked();
    case.pump(|case| !case.desktop.asked.borrow().is_empty());
    let asked = case.desktop.asked.borrow().last().unwrap().clone();
    assert_eq!(asked.title, words.launch_question);
    assert_eq!(asked.detail, words.launch_detail);
    assert_eq!(asked.action, words.launch_closed);
    assert_eq!(asked.cancel.as_deref(), Some(words.launch_wait.as_str()));
    case.settle();
    assert!(
        case.desktop.engine.releases.borrow().is_empty(),
        "keeping waiting sends nothing"
    );
    case.desktop.answer.set(true);
    visible_button(&case, &words.resolve_launch)
        .unwrap()
        .emit_clicked();
    case.pump(|case| !case.desktop.engine.releases.borrow().is_empty());
    let release = case.desktop.engine.releases.borrow()[0].clone();
    assert!(!release.launch_gone && !release.use_default_terminal);
    // The release's own answer is shown; nothing was opened.
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    case.pump(|_| labels(&page.open) == vec!["Resume in Kitty".to_string()]);
    assert!(case.desktop.engine.opens.borrow().is_empty());
    page.open.emit_clicked();
    case.pump(|case| !case.desktop.engine.opens.borrow().is_empty());
    assert!(!case.desktop.engine.opens.borrow()[0].launch_gone);
}

pub(super) fn session_open_shows_the_terminal_icon_for_a_terminal_and_sends_the_preference(
    runtime: &tokio::runtime::Runtime,
) {
    let case = Case::new(runtime);
    case.desktop.engine.answer_plans(|request| {
        let destination = if request.session_id == "term" {
            "terminal"
        } else {
            "desktop_app"
        };
        plan(
            "resume",
            json!({"title": "Somewhere", "destination": destination}),
        )
    });
    case.sign_in();
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    let terminal_icon = || {
        let mut found = false;
        walk(page.open.upcast_ref(), &mut |widget| {
            found |= widget.widget_name() == "session-open-terminal";
        });
        found
    };
    case.app.open_local_session(row("codex", "term"));
    case.pump(|_| page.open.is_sensitive());
    assert!(
        terminal_icon(),
        "a terminal destination shows the terminal icon"
    );
    case.app.open_local_session(row("codex", "app"));
    case.pump(|_| page.open.is_sensitive() && !terminal_icon());
    assert_session_open_action(&page.open, "codex", "Somewhere");
    // "Automatic" by default: no preference is sent.
    assert!(case
        .desktop
        .engine
        .plans
        .borrow()
        .iter()
        .all(|look| !look.prefer_terminal));
}

pub(super) fn session_open_asks_the_questions_an_answer_needs(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    let choices = json!([{"id": "pane:101:500:7", "label": "Kitty (/dev/pts/4)"},
                         {"id": "pane:102:501:7", "label": "Kitty (/dev/pts/5)"}]);
    let choose = plan(
        "choose",
        json!({"state": "live_multiple", "title": "Open running session",
        "reason_code": "several_locations", "choices": choices,
        "message": "This session is open in more than one place. Choose where to open it."}),
    );
    let after = choose.clone();
    case.desktop.engine.answer_plans(move |_| after.clone());
    let confirm = plan(
        "confirm",
        json!({"title": "Resume in Kitty", "reason_code": "possibly_open_elsewhere",
        "message": "This session may already be open in another process. Confirm it is not open there before resuming it here.",
        "elsewhere": ["Kitty (/dev/pts/9)"],
        "question": "This session may already be open in another process. Confirm it is not open there before resuming it here.\nKitty (/dev/pts/9)"}),
    );
    let asks = confirm.clone();
    case.desktop.engine.answer_opens(move |request| {
        Some(match (&request.selected, request.confirmed) {
            (Some(_), false) => opened("refused", asks.message.as_deref().unwrap(), asks.clone()),
            _ => opened(
                "verified",
                "Opened this session.",
                plan("focus", json!({"title": "Open in Kitty"})),
            ),
        })
    });
    case.sign_in();
    case.app.open_local_session(row("claude", "one"));
    let shell = case.app.shell().unwrap();
    let page = &shell.pages.sessions.transcript;
    case.pump(|_| labels(&page.open) == vec!["Open running session".to_string()]);
    // Declining the choice sends nothing.
    page.open.emit_clicked();
    case.pump(|case| case.desktop.engine.picks.borrow().len() == 1);
    let (title, detail, labels_asked) = case.desktop.engine.picks.borrow()[0].clone();
    assert_eq!(title, "Open running session");
    assert_eq!(detail, choose.message.clone().unwrap());
    assert_eq!(
        labels_asked,
        vec!["Kitty (/dev/pts/4)", "Kitty (/dev/pts/5)"]
    );
    case.settle();
    assert!(case.desktop.engine.opens.borrow().is_empty());
    // The choice is sent back; the confirmation it stops at is asked next.
    case.desktop.engine.pick_answer.set(Some(1));
    page.open.emit_clicked();
    case.pump(|case| case.desktop.engine.opens.borrow().len() == 2);
    let opens = case.desktop.engine.opens.borrow().clone();
    assert_eq!(opens[0].selected.as_deref(), Some("pane:102:501:7"));
    assert!(opens[1].confirmed);
    assert_eq!(
        opens[1].selected.as_deref(),
        Some("pane:102:501:7"),
        "the choice is kept with the reply"
    );
    let asked = case.desktop.asked.borrow().last().unwrap().clone();
    assert_eq!(
        asked.title, "Resume in Kitty",
        "the Mac's dialog: the button's title"
    );
    assert_eq!(asked.detail, confirm.question.clone().unwrap());
    assert_eq!(asked.action, "Resume in Kitty");
    assert_eq!(
        asked.cancel.as_deref(),
        Some(open_strings().cancel.as_str())
    );
    case.pump(|_| shown(&case, "Opened this session."));
    assert_session_open_action(&page.open, "claudecode", "Open in Kitty");
    // Stopping a background owner is asked first (as on the Mac), and sent
    // only when the person confirms.
    case.desktop
        .engine
        .answer_plans(|_| plan("take_over", json!({"title": "Stop and resume in Kitty"})));
    case.app.open_local_session(row("claude", "two"));
    case.pump(|_| labels(&page.open) == vec!["Stop and resume in Kitty".to_string()]);
    assert!(opens.iter().all(|open| !open.take_over));
    case.desktop.answer.set(false);
    page.open.emit_clicked();
    case.pump(|case| case.desktop.asked.borrow().len() == 2);
    let asked = case.desktop.asked.borrow().last().unwrap().clone();
    assert_eq!(asked.title, "Stop and resume in Kitty");
    assert_eq!(asked.detail, open_strings().take_over_detail);
    assert!(asked.destructive);
    case.settle();
    assert_eq!(
        case.desktop.engine.opens.borrow().len(),
        2,
        "declined: nothing sent"
    );
    case.desktop.answer.set(true);
    page.open.emit_clicked();
    case.pump(|case| case.desktop.engine.opens.borrow().len() == 3);
    let taken = case.desktop.engine.opens.borrow()[2].clone();
    assert!(taken.take_over && !taken.confirmed);
}
