//! The engine's ports as this app runs them: a platform adapter, plus what
//! only the app knows (legacy pending resumes, the provider store history
//! recorded), and the generic plan and open cycles the tests drive with
//! fakes.
use std::collections::BTreeMap;

use session_navigation::decide::decide;
use session_navigation::engine;
use session_navigation::evidence::{Fact, Reservation, Snapshot};
use session_navigation::model::{Provider, SessionRef, Tool};
use session_navigation::plan::{Decision, Dispatch, OpenRequest, Step};
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};
use session_navigation_native::system::System;
use session_navigation_native::ProviderPaths;

use super::contract::{OpenResult, PlanView};
use super::legacy;
use super::view;

/// An adapter the engine can drive.
pub trait Ports: Observe + Execute + Reservations {}
impl<T: Observe + Execute + Reservations> Ports for T {}

/// [`Ports`] around a platform adapter `P`, over the same system `S`.
pub struct Navigator<P, S> {
    pub inner: P,
    sys: S,
    /// The app's data directory, where the legacy files live.
    app_dir: String,
    /// The legacy keys of this session (as given, and as the engine names it).
    keys: Vec<String>,
    /// The provider-store paths history recorded for the session's process.
    recorded: Fact<BTreeMap<String, String>>,
    /// The provider homes the adapter searched, by environment key.
    searched: BTreeMap<String, String>,
}

/// The homes an adapter searches, by the key that would select another.
pub fn searched(paths: &ProviderPaths) -> BTreeMap<String, String> {
    [
        ("CLAUDE_CONFIG_DIR", &paths.claude_home),
        ("CODEX_HOME", &paths.codex_home),
        ("CODEX_SQLITE_HOME", &paths.codex_sqlite_home),
        ("GROK_HOME", &paths.grok_home),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.clone()))
    .collect()
}

fn tool(provider: Provider) -> Tool {
    match provider {
        Provider::Claude => Tool::ClaudeCli,
        Provider::Codex => Tool::CodexCli,
        Provider::OpenCode => Tool::OpenCodeCli,
        Provider::Grok => Tool::GrokCli,
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let trim = |p: &str| p.trim_end_matches(['/', '\\']).to_owned();
    if cfg!(windows) {
        trim(a).eq_ignore_ascii_case(&trim(b))
    } else {
        trim(a) == trim(b)
    }
}

impl<P, S: System> Navigator<P, S> {
    pub fn new(
        inner: P,
        sys: S,
        app_dir: &str,
        keys: Vec<String>,
        recorded: Fact<BTreeMap<String, String>>,
        searched: BTreeMap<String, String>,
    ) -> Self {
        Self {
            inner,
            sys,
            app_dir: app_dir.to_owned(),
            keys,
            recorded,
            searched,
        }
    }

    /// The adapter only searched the default homes. When the session's
    /// process was recorded with another store, what it found (or did not)
    /// is not that store's: the environment is unknown, and a launch is
    /// refused rather than opened in the wrong store.
    fn check_environment(&self, snapshot: &mut Snapshot) {
        let Fact::Known(recorded) = &self.recorded else {
            return;
        };
        let keys = tool(snapshot.session.provider).environment_keys();
        let differs = recorded
            .iter()
            .filter(|(key, _)| keys.contains(&key.as_str()))
            .any(|(key, path)| {
                let found = snapshot.environment.known().and_then(|env| env.get(key));
                let expected = found.or_else(|| self.searched.get(key));
                !expected.is_some_and(|expected| same_path(expected, path))
            });
        if differs {
            snapshot.environment = Fact::Unknown;
        }
    }

    /// A legacy pending resume stands in for a missing reservation.
    fn overlay_legacy(&self, snapshot: &mut Snapshot) {
        if snapshot.reservation != Fact::Known(None) {
            return;
        }
        snapshot.reservation = match legacy::pending(&self.sys, &self.app_dir, &self.keys) {
            Fact::Known(false) => Fact::Known(None),
            Fact::Known(true) => Fact::Known(Some(legacy::reservation(&snapshot.session))),
            Fact::Unknown => Fact::Unknown,
        };
    }

    /// A live owner resolves a legacy pending resume, as the apps did.
    pub fn settle_legacy(&self, decision: &Decision) {
        if decision.state.is_live() {
            // A failed rewrite leaves the entry blocking, as before.
            let _ = legacy::take(&self.sys, &self.app_dir, &self.keys);
        }
    }
}

impl<P: Observe, S: System> Observe for Navigator<P, S> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        let mut snapshot = self.inner.observe(session);
        self.check_environment(&mut snapshot);
        self.overlay_legacy(&mut snapshot);
        snapshot
    }
}

impl<P: Execute, S> Execute for Navigator<P, S> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.inner.execute(step)
    }
}

impl<P: Reservations, S: System> Reservations for Navigator<P, S> {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        self.inner.reserve(reservation)
    }

    /// Releasing also clears a legacy entry: the engine released what the
    /// snapshot showed, which may have been one.
    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        self.inner.release(session)?;
        legacy::take(&self.sys, &self.app_dir, &self.keys).map_err(|_| StoreError)
    }
}

fn pending(snapshot: &Snapshot) -> Fact<bool> {
    match &snapshot.reservation {
        Fact::Known(r) => Fact::Known(r.is_some()),
        Fact::Unknown => Fact::Unknown,
    }
}

/// The button: one look and `decide`, nothing else.
pub fn plan<P: Observe>(ports: &mut P, session: &SessionRef, request: &OpenRequest) -> PlanView {
    let (snapshot, decision) = engine::plan(ports, session, request);
    let default = (!request.use_default_terminal).then(|| {
        let mut asked = *request;
        asked.use_default_terminal = true;
        decide(&snapshot, &asked)
    });
    view::plan(&decision, &pending(&snapshot), default.as_ref())
}

/// The request a fresh button look uses after a click: no one-time answers.
fn plain(request: &OpenRequest) -> OpenRequest {
    OpenRequest {
        prefs: request.prefs,
        use_default_terminal: request.use_default_terminal,
        ..OpenRequest::default()
    }
}

/// The person said an earlier launch was closed or stopped: release its
/// reservation (the engine's, or a legacy pending resume) and nothing else,
/// then look again. Nothing is opened; the person clicks Open next. With no
/// reservation on record this is only the look. `Err` carries the fresh
/// look when the reservation could not be released.
pub fn release<P: Ports, S: System>(
    ports: &mut Navigator<P, S>,
    session: &SessionRef,
    request: &OpenRequest,
) -> Result<PlanView, PlanView> {
    let snapshot = ports.observe(&session.normalized());
    let released = match snapshot.reservation {
        Fact::Known(Some(_)) => ports.release(&session.normalized()),
        Fact::Known(None) | Fact::Unknown => Ok(()),
    };
    let after = plan(ports, session, &plain(request));
    released.map(|()| after.clone()).map_err(|_| after)
}

/// One click: the whole engine cycle, then a fresh look for the button.
pub fn open<P: Ports, S: System>(
    ports: &mut Navigator<P, S>,
    session: &SessionRef,
    request: &OpenRequest,
) -> OpenResult {
    let report = engine::open(ports, session, request);
    ports.settle_legacy(&report.decision);
    let after = plan(ports, session, &plain(request));
    view::open(&report, after)
}
