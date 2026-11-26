//! "Open session": return a person to a coding-agent session, from the
//! app's own process. The rules are `session-navigation` (observe → decide
//! → act → verify); the platform adapter is `session-navigation-native`;
//! this module adds what only the app knows (where background discovery
//! saw the session, the reservations earlier app versions wrote) and the
//! words both apps show. It runs in the app, never in the core daemon: a
//! background service cannot raise windows on Windows, and macOS must
//! attribute terminal-scripting permission to the app. GTK calls [`plan`],
//! [`open`] and [`release_launch`] directly; the macOS app runs
//! `agent-relay-probe session-open plan|open|release` (the helper it
//! bundles) as its own child, request JSON on stdin, one answer object on
//! stdout ([`command`]). `session-open strings` prints the fixed words
//! ([`contract::Strings`]) with no request; `shared/session-open-strings.json`
//! is its output, and the macOS app's no-answer fallback is tested against
//! it.
//!
//! # Request (`contract::Request`, JSON)
//!
//! ```json
//! {"source": "codex", "session_id": "01900000-0000-7000-8000-000000000002",
//!  "app_data_dir": "/Users/me/Library/Application Support/com.agentrelay.desktop",
//!  "directory": "/Users/me/code/project", "use_default_terminal": false,
//!  "prefer_terminal": false, "default_terminal": "ghostty",
//!  "confirmed": false, "launch_gone": false, "take_over": false,
//!  "selected": null}
//! ```
//!
//! Only `source`, `session_id` and `app_data_dir` are required; unknown
//! fields are refused. `default_terminal` is one of
//! [`contract::TERMINAL_IDS`] or absent (the system's default terminal).
//! `prefer_terminal` is the person's per-provider "Open with: Terminal"
//! (see [`contract::Request::prefer_terminal`]).
//!
//! # Plan answer (`contract::PlanView`)
//!
//! ```json
//! {"state": "stopped", "action": "resume", "title": "Resume in Ghostty",
//!  "enabled": true, "reason_code": null, "message": null,
//!  "precision": "right_window", "destination": "terminal", "question": null,
//!  "can_resume_default": false, "pending_launch": false, "choices": [],
//!  "elsewhere": [], "strings": {"open_session": "Open session", "...": "..."}}
//! ```
//!
//! - `action`: `focus`, `show_application`, `open_link`, `resume`,
//!   `take_over`, `attach`, `choose`, `confirm`, `unavailable`.
//! - `state`: `live`, `live_multiple`, `live_not_locatable`,
//!   `live_headless`, `stopped`, `archived`, `not_found`, `launch_pending`,
//!   `unknown`, `invalid_id`, `unsupported`.
//! - `precision`: `exact_conversation`, `right_window`, `app_only`.
//! - `destination`: `terminal`, `desktop_app`, `none`: the terminal icon is
//!   shown exactly for `terminal`.
//! - `question`: for `confirm` and `take_over`, the dialog text (with
//!   `elsewhere` one per line); its buttons are `title` and
//!   `strings.cancel`.
//! - `strings`: the fixed words ([`contract::Strings`]), the same in every
//!   answer.
//! - `reason_code` / `message`: one code and sentence from
//!   [`wording`] (each core `Reason`, plus `invalid_request`,
//!   `unsupported_source`, `unknown_terminal`, `already_opening`,
//!   `desktop_id_unmapped`, `several_locations`, `home_unavailable`).
//! - `choose`: `choices` `[{id, label}]`; send one `id` back as `selected`.
//!   `confirm`: ask `message` (naming `elsewhere`), then send `confirmed`.
//!   `pending_launch` with `reason_code` `launch_pending`: ask
//!   `strings.launch_question`, then call `release` with the same request
//!   (it only releases the reservation and answers a fresh plan; the person
//!   then clicks Open). `launch_gone` on `open` still releases and opens in
//!   one call; the apps do not use it.
//!
//! # Open answer (`contract::OpenResult`)
//!
//! ```json
//! {"outcome": "dispatched_unverified", "precision": "right_window",
//!  "reason_code": null,
//!  "message": "Sent the resume request for this session. The provider has not confirmed it opened.",
//!  "plan": {"state": "launch_pending", "action": "unavailable", "...": "..."}}
//! ```
//!
//! `outcome`: `verified`, `dispatched_unverified`, `refused` (nothing was
//! dispatched), `failed`. `plan` is the button after the attempt, from a
//! fresh look.
//!
//! [`plan`] is read-only and prompt-free: one look (macOS: no terminal
//! AppleScript, so no Automation prompt) and `decide`; it writes nothing.
//! [`open`] blocks for the whole cycle (up to several seconds): run it off
//! the UI thread. A second open of the same session while one runs, in this
//! process or another helper, is refused (`already_opening`), and so is a
//! release while an open runs (and the reverse).
pub mod command;
pub mod contract;
pub(crate) mod guard;
pub mod history;
pub mod legacy;
pub mod native;
pub mod navigator;
pub mod session;
pub mod view;
pub mod wording;

use std::path::{Path, PathBuf};

use session_navigation::model::SessionRef;
use session_navigation::plan::{OpenRequest, Preferences};
use session_navigation::ports::Observe;
use session_navigation_native::system::System;
use session_navigation_native::{ProviderPaths, RealSystem};

use contract::{OpenResult, PlanView, Request};
use history::StoreHistory;
use navigator::{Navigator, Ports};
use wording::Local;

/// A request checked and resolved to the engine's terms.
pub struct Prepared {
    pub session: SessionRef,
    pub request: OpenRequest,
    /// The legacy pending-resume keys this session may be filed under.
    pub keys: Vec<String>,
    /// The ids the history store may know it by.
    pub ids: Vec<String>,
}

fn valid_dir(dir: &str) -> bool {
    !dir.is_empty() && !dir.chars().any(char::is_control) && Path::new(dir).is_absolute()
}

/// Check `request` and name its session as the engine does.
pub fn prepare<S: System>(
    sys: &S,
    paths: &ProviderPaths,
    request: &Request,
) -> Result<Prepared, Local> {
    if !valid_dir(&request.app_data_dir) {
        return Err(Local::InvalidRequest);
    }
    let default_terminal = session::terminal(request.default_terminal.as_deref())?;
    let selected = match request.selected.as_deref() {
        None => None,
        Some(id) => Some(view::target(id).ok_or(Local::InvalidRequest)?),
    };
    let session = session::resolve(
        sys,
        &paths.claude_desktop_sessions_dir(),
        &request.source,
        &request.session_id,
    )?;
    let normalized = session.normalized();
    let mut ids = vec![
        request.session_id.clone(),
        session.id.clone(),
        normalized.id,
    ];
    ids.dedup();
    let keys = ids
        .iter()
        .map(|id| legacy::key(&request.source, id))
        .collect();
    Ok(Prepared {
        session,
        request: OpenRequest {
            prefs: Preferences {
                default_terminal,
                allow_import: false,
                prefer_terminal: request.prefer_terminal,
            },
            take_over: request.take_over,
            use_default_terminal: request.use_default_terminal,
            confirm_launch_gone: request.launch_gone,
            selected,
            confirm_not_open_elsewhere: request.confirmed,
        },
        keys,
        ids,
    })
}

/// The core's probe directory, which holds the session-locations store.
pub fn history_root() -> Option<PathBuf> {
    agent_relay_node::relay::paths::home()
        .ok()
        .map(|home| home.join(".agentworkforce/probe"))
}

fn load_history(root: Option<&Path>, prepared: &Prepared) -> StoreHistory {
    let ids: Vec<&str> = prepared.ids.iter().map(String::as_str).collect();
    match root {
        Some(root) => StoreHistory::load(root, &prepared.session, &ids),
        None => StoreHistory {
            session: Some(prepared.session.clone()),
            ..StoreHistory::default()
        },
    }
}

fn wrap<P, S: System>(
    inner: P,
    sys: S,
    request: &Request,
    prepared: &Prepared,
    history: &StoreHistory,
    paths: &ProviderPaths,
) -> Navigator<P, S> {
    Navigator::new(
        inner,
        sys,
        &request.app_data_dir,
        prepared.keys.clone(),
        history::environment(history.latest.as_ref()),
        navigator::searched(paths),
    )
}

/// [`plan`] over any adapter: `make` builds it from the session's history.
pub fn plan_with<P: Observe, S: System + Clone>(
    sys: S,
    paths: &ProviderPaths,
    history_root: Option<&Path>,
    request: &Request,
    make: impl FnOnce(StoreHistory) -> P,
) -> PlanView {
    let prepared = match prepare(&sys, paths, request) {
        Ok(prepared) => prepared,
        Err(reason) => return view::refused(reason),
    };
    let history = load_history(history_root, &prepared);
    let inner = make(history.clone());
    let mut ports = wrap(inner, sys, request, &prepared, &history, paths);
    navigator::plan(&mut ports, &prepared.session, &prepared.request)
}

/// [`open`] over any adapter.
pub fn open_with<P: Ports, S: System + Clone>(
    sys: S,
    paths: &ProviderPaths,
    history_root: Option<&Path>,
    request: &Request,
    make: impl FnOnce(StoreHistory) -> P,
) -> OpenResult {
    let prepared = match prepare(&sys, paths, request) {
        Ok(prepared) => prepared,
        Err(reason) => return view::refused_open(wording::local(reason), view::refused(reason)),
    };
    let history = load_history(history_root, &prepared);
    let inner = make(history.clone());
    let mut ports = wrap(inner, sys, request, &prepared, &history, paths);
    let locks = Path::new(&request.app_data_dir).join("session-navigation/opening");
    let refusal = match guard::begin(&locks, &prepared.session) {
        Ok(_opening) => return navigator::open(&mut ports, &prepared.session, &prepared.request),
        Err(guard::Refusal::Busy) => wording::local(Local::AlreadyOpening),
        Err(guard::Refusal::Store) => store_failed(),
    };
    let after = navigator::plan(&mut ports, &prepared.session, &prepared.request);
    view::refused_open(refusal, after)
}

/// [`release_launch`] over any adapter.
pub fn release_with<P: Ports, S: System + Clone>(
    sys: S,
    paths: &ProviderPaths,
    history_root: Option<&Path>,
    request: &Request,
    make: impl FnOnce(StoreHistory) -> P,
) -> PlanView {
    let prepared = match prepare(&sys, paths, request) {
        Ok(prepared) => prepared,
        Err(reason) => return view::refused(reason),
    };
    let history = load_history(history_root, &prepared);
    let inner = make(history.clone());
    let mut ports = wrap(inner, sys, request, &prepared, &history, paths);
    let locks = Path::new(&request.app_data_dir).join("session-navigation/opening");
    let refusal = match guard::begin(&locks, &prepared.session) {
        Ok(_releasing) => {
            match navigator::release(&mut ports, &prepared.session, &prepared.request) {
                Ok(after) => return after,
                Err(after) => return view::refused_after(store_failed(), after),
            }
        }
        Err(guard::Refusal::Busy) => wording::local(Local::AlreadyOpening),
        Err(guard::Refusal::Store) => store_failed(),
    };
    let after = navigator::plan(&mut ports, &prepared.session, &prepared.request);
    view::refused_after(refusal, after)
}

fn store_failed() -> wording::Words {
    wording::reason(session_navigation::model::Reason::ReservationStoreFailed)
}

/// The person said the earlier launch was closed or stopped: release the
/// session's launch reservation (a pending resume an earlier app version
/// wrote included) and answer the button from a fresh look. Opens nothing;
/// one at a time with [`open`] for the same session (a release while one
/// runs answers `already_opening`). Prompt-free, like [`plan`].
pub fn release_launch(request: &Request) -> PlanView {
    if !valid_dir(&request.app_data_dir) {
        return view::refused(Local::InvalidRequest);
    }
    let Some(cfg) = native::config(&request.app_data_dir) else {
        return view::refused(Local::HomeUnavailable);
    };
    let paths = cfg.common.paths.clone();
    let root = history_root();
    release_with(RealSystem, &paths, root.as_deref(), request, |history| {
        native::ports(RealSystem, cfg, history, native::PLAN_IS_PASSIVE)
    })
}

/// The Open button's state for `request`: read-only and prompt-free.
pub fn plan(request: &Request) -> PlanView {
    if !valid_dir(&request.app_data_dir) {
        return view::refused(Local::InvalidRequest);
    }
    let Some(cfg) = native::config(&request.app_data_dir) else {
        return view::refused(Local::HomeUnavailable);
    };
    let paths = cfg.common.paths.clone();
    let root = history_root();
    plan_with(RealSystem, &paths, root.as_deref(), request, |history| {
        native::ports(RealSystem, cfg, history, native::PLAN_IS_PASSIVE)
    })
}

/// One click on Open: observe, decide, act, verify. Blocking.
pub fn open(request: &Request) -> OpenResult {
    if !valid_dir(&request.app_data_dir) {
        let reason = Local::InvalidRequest;
        return view::refused_open(wording::local(reason), view::refused(reason));
    }
    let Some(cfg) = native::config(&request.app_data_dir) else {
        let reason = Local::HomeUnavailable;
        return view::refused_open(wording::local(reason), view::refused(reason));
    };
    let paths = cfg.common.paths.clone();
    let root = history_root();
    open_with(RealSystem, &paths, root.as_deref(), request, |history| {
        native::ports(RealSystem, cfg, history, false)
    })
}

#[cfg(test)]
mod tests;
