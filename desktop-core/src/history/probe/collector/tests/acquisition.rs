//! The collector's acquisition pass: only a sign-in scoped to the exact
//! account the job delivers to reaches the issuer, and what the pass
//! settles is applied to the held batches in the queue.
use super::passes::reconcile_signed_in;
use super::*;
use crate::history::acquisition::contract::{
    ClaimRequest, ClaimResponse, InstallationRequest, InstallationResponse, RebindRequest,
    RebindResponse, StatusRequest, StatusResponse,
};
use crate::history::acquisition::{
    self, hold_reason, AcquisitionIssuer, IssuerError, Secret, Store,
};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Enrols `droid` only and cannot be reached for anything else; counts
/// every call.
struct Issuer {
    calls: Rc<Cell<usize>>,
}
impl Issuer {
    fn unreachable<T>(&self) -> std::result::Result<T, IssuerError> {
        self.calls.set(self.calls.get() + 1);
        Err(IssuerError::Transport("unreachable".into()))
    }
}
impl AcquisitionIssuer for Issuer {
    fn enrolled_sources(&self) -> std::result::Result<Option<BTreeSet<String>>, IssuerError> {
        self.calls.set(self.calls.get() + 1);
        Ok(Some(BTreeSet::from(["droid".to_owned()])))
    }
    fn register(
        &self,
        _: &Secret,
        _: &InstallationRequest,
    ) -> std::result::Result<InstallationResponse, IssuerError> {
        self.unreachable()
    }
    fn claim(
        &self,
        _: &Secret,
        _: &ClaimRequest,
    ) -> std::result::Result<ClaimResponse, IssuerError> {
        self.unreachable()
    }
    fn rebind(
        &self,
        _: &Secret,
        _: &RebindRequest,
    ) -> std::result::Result<RebindResponse, IssuerError> {
        self.unreachable()
    }
    fn status(
        &self,
        _: &Secret,
        _: &StatusRequest,
    ) -> std::result::Result<StatusResponse, IssuerError> {
        self.unreachable()
    }
    fn sign_in(&self) -> String {
        "sign-in".into()
    }
}

fn config(delivery_account: String, job_id: String) -> Config {
    Config {
        version: 1,
        site_url: "https://synthetic.invalid".into(),
        account_id: "account".into(),
        account_email: None,
        account_name: None,
        account_avatar_url: None,
        workspace_name: None,
        org_id: "org".into(),
        workspace_id: "workspace".into(),
        history_url: "invalid synthetic URL".into(),
        cloud_history_origin: None,
        delivery_account,
        job_id,
        include_existing: false,
        sharing_mode: None,
        acknowledge_uninspected_schedules: false,
    }
}

fn auth(org: &str, workspace: &str) -> cloud::StoredAuth {
    cloud::StoredAuth {
        base_url: "https://history.synthetic.invalid".into(),
        access_token: "token".into(),
        access_token_expires_at: None,
        refresh_token: None,
        org_id: Some(org.into()),
        workspace_id: Some(workspace.into()),
    }
}

/// A queue with one held batch per `(source, session, reason)`.
fn queue(directory: &Path, held: &[(&str, &str, &str)]) -> (Connection, Config) {
    let conn = delivery::open_for_capture(&directory.join("history.db")).unwrap();
    let job = delivery::create_job(&conn, &job_config(false), 0).unwrap();
    for (n, (source, session, reason)) in held.iter().enumerate() {
        conn.execute(
            "INSERT INTO delivery_batches(id,job_id,state,payload,records,bytes,journal_end,created_ms,held_source,held_session,held_reason) VALUES(?1,?2,'pending','{}',1,80,1,10,?3,?4,?5)",
            rusqlite::params![format!("held-{n}"), job.job_id, source, session, reason],
        )
        .unwrap();
    }
    let config = config(job.config.account_id, job.job_id);
    (conn, config)
}

fn held_reasons(conn: &Connection) -> Vec<(String, String, String)> {
    delivery::held_sessions(conn)
        .unwrap()
        .into_iter()
        .map(|held| (held.source, held.session_id, held.reason))
        .collect()
}

/// One acquisition pass; how many issuer calls it made. The issuer is made
/// only when it is called.
fn pass(conn: &Connection, config: &Config, auth: cloud::StoredAuth, store: &Store) -> usize {
    let calls = Rc::new(Cell::new(0));
    let made = Cell::new(false);
    let held = delivery::held_sessions(conn).unwrap();
    reconcile_signed_in(
        conn,
        config,
        auth,
        &held,
        |_| {
            made.set(true);
            Issuer {
                calls: calls.clone(),
            }
        },
        store,
        0,
    );
    assert_eq!(made.get(), calls.get() > 0);
    calls.get()
}

#[test]
fn the_exact_account_releases_and_relabels_held_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("acquisition"));
    let (conn, config) = queue(
        directory.path(),
        &[
            ("claude", "free", hold_reason::NO_GRANT),
            ("droid", "expired", acquisition::contract::GRANT_EXPIRED),
        ],
    );
    assert!(pass(&conn, &config, auth("org", "workspace"), &store) > 0);
    // `claude` is not enrolled: released without an issuer call for it. The
    // expired grant stays held, now as renewal unsupported.
    assert_eq!(
        held_reasons(&conn),
        vec![(
            "droid".to_owned(),
            "expired".to_owned(),
            hold_reason::RENEWAL_UNSUPPORTED.to_owned()
        )]
    );
    let reason: Option<String> = conn
        .query_row(
            "SELECT held_reason FROM delivery_batches WHERE id='held-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason.as_deref(), Some(hold_reason::RENEWAL_UNSUPPORTED));
    let released: Option<String> = conn
        .query_row(
            "SELECT held_source FROM delivery_batches WHERE id='held-0'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(released, None);
}

#[test]
fn another_org_workspace_or_delivery_account_asks_the_issuer_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("acquisition"));
    let (conn, config) = queue(
        directory.path(),
        &[("claude", "free", hold_reason::NO_GRANT)],
    );
    let before = held_reasons(&conn);
    let other_delivery = Config {
        delivery_account: destination::account_id("org", Some("other")),
        ..config.clone()
    };
    for (config, auth) in [
        (&config, auth("other", "workspace")),
        (&config, auth("org", "other")),
        (&other_delivery, auth("org", "workspace")),
    ] {
        assert_eq!(pass(&conn, config, auth, &store), 0);
        assert_eq!(held_reasons(&conn), before);
    }
    assert!(!store.path().exists(), "nothing stored for another account");
}
