//! Durable reset: stop capture, erase Cloud first, then replace the local journal.
//! Original provider files are never modified. Cloud resets suppress rediscovery;
//! local development resets clear that suppression and never contact Cloud.
use super::{bridge, collector, history, read_config, save_json, Config};
use crate::history::delivery::{self, SessionIdentity};
use crate::history::destination::{ResetUnsupported, RESET_UNSUPPORTED_MESSAGE};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

pub const CONFIRMATION: &str = "DELETE ALL SESSIONS";
pub const LOCAL_CONFIRMATION: &str = "RESET LOCAL SESSIONS";

#[derive(Serialize, Deserialize)]
struct ResetPlan {
    request_id: String,
    config: Config,
    baseline: Vec<SessionIdentity>,
    #[serde(default)]
    local_only: bool,
}

fn root() -> Result<std::path::PathBuf> {
    Ok(super::home()?.join(".agentworkforce"))
}

/// Readers and reset share a process-independent lock, including the private caches.
pub fn reader_lock() -> Result<crate::history::history_access::Access> {
    reader_lock_at(&root()?)
}
fn reader_lock_at(root: &Path) -> Result<crate::history::history_access::Access> {
    match crate::history::history_access::try_reader(root) {
        Ok(reader) => Ok(reader),
        Err(error) if agent_relay_node::platform::fs::lock_held(&error) => Err(super::user_error(
            crate::history::history_access::RESET_BUSY,
        )),
        Err(error) => Err(error.into()),
    }
}
fn access_lock(exclusive: bool) -> Result<crate::history::history_access::Access> {
    let root = root()?;
    Ok(if exclusive {
        crate::history::history_access::writer(&root)?
    } else {
        crate::history::history_access::reader(&root)?
    })
}

pub fn baseline() -> Result<HashSet<SessionIdentity>> {
    let path = root()?.join("session-reset-baseline.json");
    if !path.exists() {
        return Ok(HashSet::new());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// Discovery may see preserved provider files again. Remove their shallow rows
/// immediately; the durable baseline is the only metadata we retain for them.
pub fn prune_catalog(db: &Path) -> Result<()> {
    let forgotten = baseline()?;
    if forgotten.is_empty() {
        return Ok(());
    }
    let mut conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = conn.transaction()?;
    for identity in forgotten {
        tx.execute(
            "DELETE FROM sessions WHERE source=? AND session_id=?",
            rusqlite::params![identity.source, identity.session_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn reset(directory: &Path, confirmation: &str) -> Result<()> {
    reset_mode(directory, confirmation, false)
}

pub fn reset_mode(directory: &Path, confirmation: &str, local_only: bool) -> Result<()> {
    ensure!(
        confirmation
            == if local_only {
                LOCAL_CONFIRMATION
            } else {
                CONFIRMATION
            },
        super::user_error(if local_only {
            "Type RESET LOCAL SESSIONS to confirm the local reset."
        } else {
            "Type DELETE ALL SESSIONS to confirm the reset."
        })
    );
    let _control = bridge::control_lock(directory)?;
    // Do not turn a pending cloud deletion into a local reset (or vice versa).
    let pending = directory.join("session-reset-pending.json");
    if pending.exists() {
        let plan: ResetPlan = serde_json::from_slice(&fs::read(&pending)?)?;
        ensure!(
            plan.local_only == local_only,
            super::user_error(
                "A different session reset is pending. Finish that reset before starting another."
            )
        );
    }
    let config = read_config(directory)?;
    if !local_only && !pending.exists() {
        // Ask before stopping anything: a service without cloud reset gets no plan.
        std::env::set_var("RELAYHISTORY_HOME", directory);
        crate::history::destination::require_session_reset(
            &config.history_url,
            &config.delivery_account,
        )
        .map_err(unsupported)?;
    }
    let was_running = collector::running(directory)?;
    collector::stop_for_change(directory)?;
    let result = reset_stopped(directory, config, local_only);
    finish_paused_reset(&pending, was_running, result, || {
        collector::start_background(directory)
    })?;
    bridge::emit(serde_json::json!({"reset":true, "local_only":local_only}));
    Ok(())
}

// The stopped operation owns its locks so they are released before capture resumes.
fn reset_stopped(directory: &Path, config: Config, local_only: bool) -> Result<()> {
    let _collector = super::lock(directory)?;
    let _readers = access_lock(true)?;
    let pending = directory.join("session-reset-pending.json");
    if !pending.exists() {
        let mut known = HashSet::new();
        if !local_only {
            bridge::recover(directory)?;
            // Discover all metadata for the permanent cloud/local deletion.
            history::discover(&directory.join("history.db"), None, || false)?;
            let store = history::open_read(&directory.join("history.db"))?;
            known = baseline()?;
            let mut query = ai_hist::CatalogQuery::default();
            query.scope = ai_hist::SessionScope::All;
            for row in store.sessions(query) {
                let row = row?;
                known.insert(SessionIdentity {
                    source: row.source.as_str().into(),
                    session_id: row.session_id,
                });
            }
        }
        let plan = ResetPlan {
            request_id: format!("desktop-reset-{}-{}", collector::now(), std::process::id()),
            config: if local_only {
                config
            } else {
                read_config(directory)?
            },
            baseline: known.into_iter().collect(),
            local_only,
        };
        save_json(&pending, &plan)?;
    }
    recover_locked(directory).map_err(unsupported)
}

/// The sentence a reset the History service does not serve ends with.
fn unsupported(error: anyhow::Error) -> anyhow::Error {
    if error.is::<ResetUnsupported>() {
        super::user_error(RESET_UNSUPPORTED_MESSAGE)
    } else {
        error
    }
}

fn finish_paused_reset(
    pending: &Path,
    was_running: bool,
    result: Result<()>,
    resume: impl FnOnce() -> Result<()>,
) -> Result<()> {
    // A saved plan must keep uploads fenced until recovery succeeds. A failure
    // before that plan exists has no recovery path: restore the prior capture state.
    if result.is_ok() || (was_running && !pending.exists()) {
        match (result, resume()) {
            (Err(error), Err(resume_error)) => Err(error.context(format!(
                "Capture also failed to restart after reset planning failed: {resume_error:#}"
            ))),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), resumed) => resumed,
        }
    } else {
        result
    }
}

/// Called while the collector lock is held, before any upload may resume.
pub fn recover(directory: &Path) -> Result<()> {
    if !directory.join("session-reset-pending.json").exists() {
        return Ok(());
    }
    let _readers = access_lock(true)?;
    match recover_locked(directory) {
        // The plan is gone, so uploads resume; the log says why.
        Err(error) if error.is::<ResetUnsupported>() => Ok(()),
        other => other,
    }
}

/// A History service that does not serve reset never will: drop the plan
/// rather than fence uploads behind it, and leave local history as it is.
fn recover_locked(directory: &Path) -> Result<()> {
    let result = replay_cloud_reset(directory);
    if result
        .as_ref()
        .is_err_and(|error| error.is::<ResetUnsupported>())
    {
        remove_file(&directory.join("session-reset-pending.json"))?;
    }
    result
}

fn replay_cloud_reset(directory: &Path) -> Result<()> {
    recover_with(directory, |plan| {
        std::env::set_var("RELAYHISTORY_HOME", directory);
        crate::history::destination::reset_sessions(
            &plan.config.history_url,
            &plan.config.delivery_account,
            &plan.request_id,
            CONFIRMATION,
        )
    })
}

fn recover_with(directory: &Path, cloud: impl FnOnce(&ResetPlan) -> Result<()>) -> Result<()> {
    let pending = directory.join("session-reset-pending.json");
    let mut plan: ResetPlan = serde_json::from_slice(&fs::read(&pending)?)?;
    if !plan.local_only {
        cloud(&plan)?;
    } else {
        // A dev reset discards pending sharing changes, never replays them to Cloud.
        remove_file(&directory.join("sharing-change.json"))?;
        plan.baseline.clear();
        let images = root()?.join("session-reader/images");
        if images.exists() {
            fs::remove_dir_all(images)?;
        }
    }
    // Keep only identities, never prompts/transcripts, to suppress old sessions.
    save_json(&root()?.join("session-reset-baseline.json"), &plan.baseline)?;
    for path in [
        delivery::upload_queue_path(&directory.join("history.db")),
        directory.join("history.db"),
        root()?.join("session-reader/history.db"),
        root()?.join("session-preview/catalog.db"),
    ] {
        remove_database(&path)?;
    }
    for path in [
        directory.join("selected.json"),
        directory.join("deleted-uploads.json"),
        directory.join("status.json"),
        directory.join("session-status.json"),
        root()?.join("session-preview/sessions.json"),
    ] {
        remove_file(&path)?;
    }
    let conn = delivery::open_db(&directory.join("history.db"))?;
    for identity in &plan.baseline {
        delivery::set_session_excluded(&conn, identity, true)?;
    }
    let job = delivery::create_session_job(
        &conn,
        &delivery::DeliveryJobConfig {
            destination_id: "relayhistory".into(),
            instance_id: "teams-probe".into(),
            account_id: plan.config.delivery_account.clone(),
            mapping_version: crate::history::destination::MAPPING_VERSION.into(),
            selection: collector::selection(false),
            limits: Default::default(),
        },
        collector::now(),
    )?;
    plan.config.job_id = job.job_id;
    plan.config.include_existing = false;
    plan.config.sharing_mode = Some(crate::contract::account::SharingMode::Selected);
    save_json(
        &directory.join("selected.json"),
        &Vec::<SessionIdentity>::new(),
    )?;
    save_json(&directory.join("config.json"), &plan.config)?;
    fs::remove_file(pending)?;
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn remove_database(path: &Path) -> Result<()> {
    // No store handles remain and both collector and reader locks are held.
    remove_file(path)?;
    for suffix in ["-wal", "-shm"] {
        remove_file(Path::new(&format!("{}{suffix}", path.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_reader_fails_fast_when_reset_intent_waits_for_its_daemon() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let daemon = crate::history::history_access::reader(&root).unwrap();
        let reset_root = root.clone();
        let reset = std::thread::spawn(move || {
            let _reset = crate::history::history_access::writer(&reset_root).unwrap();
        });
        let started = std::time::Instant::now();
        while !crate::history::history_access::reset_requested(&root).unwrap() {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (entered, ready) = std::sync::mpsc::channel();
        let (answer, result) = std::sync::mpsc::channel();
        let helper = std::thread::spawn(move || {
            entered.send(()).unwrap();
            answer
                .send(reader_lock_at(&root).err().map(|error| error.to_string()))
                .unwrap();
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let fast = result.recv_timeout(std::time::Duration::from_secs(2));
        // Release every owned participant before asserting even on the old
        // blocking implementation, so its negative control cannot leak work.
        drop(daemon);
        reset.join().unwrap();
        helper.join().unwrap();
        assert_eq!(
            fast.unwrap(),
            Some(crate::history::history_access::RESET_BUSY.into())
        );
    }

    #[test]
    fn a_failed_reset_plan_resumes_capture_but_preserves_the_error() {
        let directory = tempfile::tempdir().unwrap();
        let pending = directory.path().join("session-reset-pending.json");
        let resumed = std::cell::Cell::new(false);
        let result = finish_paused_reset(
            &pending,
            true,
            Err(anyhow::anyhow!("catalog read failed")),
            || {
                resumed.set(true);
                Ok(())
            },
        );
        assert!(resumed.get(), "An unplanned reset must not strand capture");
        assert_eq!(result.unwrap_err().to_string(), "catalog read failed");
    }

    #[test]
    fn a_failed_reset_plan_preserves_an_already_stopped_collector() {
        let directory = tempfile::tempdir().unwrap();
        let pending = directory.path().join("session-reset-pending.json");
        let result = finish_paused_reset(
            &pending,
            false,
            Err(anyhow::anyhow!("catalog read failed")),
            || panic!("A failed reset must not turn previously stopped uploads on"),
        );
        assert_eq!(result.unwrap_err().to_string(), "catalog read failed");
    }

    #[test]
    fn a_pending_reset_failure_keeps_capture_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let pending = directory.path().join("session-reset-pending.json");
        fs::write(&pending, "durable reset intent").unwrap();
        let result = finish_paused_reset(
            &pending,
            true,
            Err(anyhow::anyhow!("Cloud offline")),
            || panic!("Capture must stay paused while a reset needs recovery"),
        );
        assert_eq!(result.unwrap_err().to_string(), "Cloud offline");
        assert!(pending.exists());
    }

    #[test]
    fn a_completed_reset_resumes_capture() {
        let directory = tempfile::tempdir().unwrap();
        let pending = directory.path().join("session-reset-pending.json");
        let resumed = std::cell::Cell::new(false);
        finish_paused_reset(&pending, false, Ok(()), || {
            resumed.set(true);
            Ok(())
        })
        .unwrap();
        assert!(resumed.get());
    }

    #[test]
    fn local_reset_cli_requires_an_explicit_target_and_confirmation() {
        use clap::Parser;
        let cli = crate::history::probe::Cli::try_parse_from([
            "probe",
            "sessions",
            "reset",
            "--local",
            "--account",
            "user",
            "--workspace",
            "workspace",
            "--confirmation",
            LOCAL_CONFIRMATION,
        ]);
        assert!(cli.is_ok());
        assert!(crate::history::probe::Cli::try_parse_from([
            "probe", "sessions", "reset", "--local"
        ])
        .is_err());
    }

    #[test]
    fn reset_requires_exact_consent_before_touching_disk() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("not-created");
        for input in ["", CONFIRMATION, "reset local sessions"] {
            assert!(reset_mode(&directory, input, true).is_err());
            assert!(!directory.exists());
        }
        for input in ["", "delete all sessions", "DELETE ALL SESSIONS "] {
            assert!(reset(&directory, input).is_err());
            assert!(!directory.exists());
        }
    }

    #[test]
    fn local_reset_rediscovers_sessions_without_cloud_or_losing_sign_in() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::session_reset::tests::local_reset_rediscovers_sessions_without_cloud_or_losing_sign_in") else { return; };
        let directory = home.join("install");
        fs::create_dir_all(&directory).unwrap();
        let source = home.join(".claude/projects/example/old.jsonl");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let original =
            crate::history::probe::live::tests::transcript_line("old", "preserved conversation");
        fs::write(&source, &original).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "version":1,"site_url":"https://agentrelay.com","account_id":"user","org_id":"org",
            "workspace_id":"workspace","history_url":"https://history.agentrelay.com","delivery_account":crate::history::destination::account_id("org", Some("workspace")),
            "job_id":"old-job","include_existing":true,"sharing_mode":"all"
        })).unwrap();
        save_json(&directory.join("config.json"), &config).unwrap();
        fs::write(directory.join("credentials.json"), "sign-in must survive").unwrap();
        let old = SessionIdentity {
            source: "claude".into(),
            session_id: "old".into(),
        };
        save_json(
            &root().unwrap().join("session-reset-baseline.json"),
            &vec![old.clone()],
        )
        .unwrap();
        fs::write(
            directory.join("sharing-change.json"),
            "discard pending upload changes",
        )
        .unwrap();
        fs::write(directory.join("selected.json"), "old selection").unwrap();
        fs::write(directory.join("session-status.json"), "stale status").unwrap();
        let images = root().unwrap().join("session-reader/images");
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("cached.png"), "cached image").unwrap();
        let plan = ResetPlan {
            request_id: "local-reset".into(),
            config,
            baseline: vec![old.clone()],
            local_only: true,
        };
        let pending = directory.join("session-reset-pending.json");
        save_json(&pending, &plan).unwrap();
        // Simulate an interrupted reset by replaying the saved plan twice.
        for _ in 0..2 {
            save_json(&pending, &plan).unwrap();
            recover_with(&directory, |_| {
                panic!("local reset must never contact Cloud")
            })
            .unwrap();
            assert!(!pending.exists());
            assert!(!images.exists());
            assert!(!directory.join("session-status.json").exists());
            assert_eq!(
                fs::read_to_string(directory.join("selected.json"))
                    .unwrap()
                    .trim(),
                "[]"
            );
            assert!(baseline().unwrap().is_empty());
            assert!(!directory.join("sharing-change.json").exists());
            let fresh = read_config(&directory).unwrap();
            assert_eq!(fresh.account_id, "user");
            assert_eq!(fresh.workspace_id, "workspace");
            assert_eq!(
                bridge::mode(&fresh),
                crate::contract::account::SharingMode::Selected
            );
            assert!(!fresh.include_existing);
            assert_ne!(fresh.job_id, "old-job");
            assert_eq!(
                fs::read_to_string(directory.join("credentials.json")).unwrap(),
                "sign-in must survive"
            );
            let conn = delivery::open_db(&directory.join("history.db")).unwrap();
            assert!(delivery::is_session_job(&conn, &fresh.job_id).unwrap());
            drop(conn);
        }
        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        let db = directory.join("history.db");
        history::discover(&db, None, || false).unwrap();
        history::hydrate(&db, "claude", "old", || false).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM session_events WHERE session_id='old' AND text LIKE '%preserved conversation%'", [], |r| r.get(0)).unwrap();
        assert!(
            count > 0,
            "previously excluded conversation must be readable again"
        );
    }

    const UNSUPPORTED: &str = r#"{"error":{"code":"unsupported_by_backend","message":"this route is not served by the self-host session ingest","reason":"route_not_served_selfhost"},"correlationId":"6f1c2a9e-3b4d-4e5f-8a7b-9c0d1e2f3a4b"}"#;

    /// A History service that answers each request with the next reply, as
    /// the self-hosted ingest does (content type and correlation id). Joins
    /// to the request lines it was sent.
    fn serve_history(
        replies: Vec<(u16, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                requests.push(line);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                write!(reader.get_mut(), "HTTP/1.1 {status} Reply\r\ncontent-type: application/json\r\nx-correlation-id: 6f1c2a9e-3b4d-4e5f-8a7b-9c0d1e2f3a4b\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (site, server)
    }

    /// An install signed in to the History service at `site`.
    fn signed_in_install(home: &Path, site: &str) -> std::path::PathBuf {
        let directory = home.join("install");
        fs::create_dir_all(&directory).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "version":1,"site_url":site,"account_id":"user","org_id":"org",
            "workspace_id":"workspace","history_url":site,"delivery_account":crate::history::destination::account_id("org", Some("workspace")),
            "job_id":"old-job","include_existing":true,"sharing_mode":"all"
        })).unwrap();
        save_json(&directory.join("config.json"), &config).unwrap();
        std::env::set_var("RELAYHISTORY_HOME", &directory);
        crate::history::cloud::save_auth(&crate::history::cloud::StoredAuth {
            base_url: site.into(),
            access_token: "install-token".into(),
            access_token_expires_at: Some("2100-01-01T00:00:00Z".into()),
            org_id: Some("org".into()),
            workspace_id: Some("workspace".into()),
            ..Default::default()
        })
        .unwrap();
        directory
    }

    #[test]
    fn a_reset_the_history_service_does_not_serve_ends_and_uploads_resume() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::session_reset::tests::a_reset_the_history_service_does_not_serve_ends_and_uploads_resume") else { return; };
        let other_code = UNSUPPORTED.replace("unsupported_by_backend", "not_implemented");
        let (site, server) = serve_history(vec![
            // Hosted History sends no `sessionReset`: reset is allowed.
            (200, r#"{"sessionDeletion":true}"#.into()),
            (503, r#"{"error":{"code":"unavailable"}}"#.into()),
            (501, other_code),
            (501, UNSUPPORTED.into()),
            (
                200,
                r#"{"sessionDeletion":false,"sessionReset":true}"#.into(),
            ),
            (501, UNSUPPORTED.into()),
        ]);
        let directory = signed_in_install(&home, &site);
        let pending = directory.join("session-reset-pending.json");
        let retry = "Cloud reset could not be completed. Uploads remain stopped; retry the reset.";
        // Any other refusal keeps the plan, and uploads stay fenced.
        assert_eq!(
            reset(&directory, CONFIRMATION).unwrap_err().to_string(),
            retry
        );
        assert!(pending.exists());
        assert_eq!(recover(&directory).unwrap_err().to_string(), retry);
        assert!(pending.exists());
        // The self-hosted answer ends the reset: the plan is dropped and
        // the next collector pass runs.
        recover(&directory).unwrap();
        assert!(!pending.exists());
        assert!(fs::read(directory.join("config.json")).is_ok());
        assert_eq!(read_config(&directory).unwrap().job_id, "old-job");
        // Asked for again, the person is told why.
        assert_eq!(
            reset(&directory, CONFIRMATION).unwrap_err().to_string(),
            RESET_UNSUPPORTED_MESSAGE
        );
        assert!(!pending.exists());
        let requests = server.join().unwrap();
        let paths: Vec<_> = requests
            .iter()
            .map(|line| line.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /v1/delivery/limits",
                "POST /v1/delivery/sessions/reset",
                "POST /v1/delivery/sessions/reset",
                "POST /v1/delivery/sessions/reset",
                "GET /v1/delivery/limits",
                "POST /v1/delivery/sessions/reset",
            ]
        );
    }

    #[test]
    fn a_history_service_without_cloud_reset_gets_no_reset_plan() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::session_reset::tests::a_history_service_without_cloud_reset_gets_no_reset_plan") else { return; };
        let (site, server) = serve_history(vec![(
            200,
            r#"{"sessionDeletion":false,"sessionReset":false}"#.into(),
        )]);
        let directory = signed_in_install(&home, &site);
        assert_eq!(
            reset(&directory, CONFIRMATION).unwrap_err().to_string(),
            RESET_UNSUPPORTED_MESSAGE
        );
        assert!(!directory.join("session-reset-pending.json").exists());
        assert_eq!(server.join().unwrap().len(), 1, "no reset request is sent");
    }

    #[test]
    fn reset_recovery_erases_caches_preserves_sources_and_suppresses_rediscovery() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::session_reset::tests::reset_recovery_erases_caches_preserves_sources_and_suppresses_rediscovery") else { return; };
        let directory = home.join("install");
        fs::create_dir_all(&directory).unwrap();
        let source = home.join(".claude/projects/example/old.jsonl");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(
            &source,
            crate::history::probe::live::tests::transcript_line("old", "private-text"),
        )
        .unwrap();
        let db_path = directory.join("history.db");
        history::discover(&db_path, None, || false).unwrap();
        history::hydrate(&db_path, "claude", "old", || false).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "version":1,"site_url":"https://agentrelay.com","account_id":"user","org_id":"org",
            "workspace_id":"workspace","history_url":"https://history.agentrelay.com","delivery_account":crate::history::destination::account_id("org", Some("workspace")),
            "job_id":"old-job","include_existing":true,"sharing_mode":"all"
        })).unwrap();
        save_json(&directory.join("config.json"), &config).unwrap();
        let plan = ResetPlan {
            request_id: "durable-reset".into(),
            config,
            local_only: false,
            baseline: vec![SessionIdentity {
                source: "claude".into(),
                session_id: "old".into(),
            }],
        };
        let pending = directory.join("session-reset-pending.json");
        save_json(&pending, &plan).unwrap();
        // The acquisition installation and grants live beside history.db and
        // outlive the reset that rotates the origin.
        let acquisition = directory.join("acquisition/state.json");
        fs::create_dir_all(acquisition.parent().unwrap()).unwrap();
        fs::write(
            &acquisition,
            r#"{"version":1,"installations":[],"grants":[]}"#,
        )
        .unwrap();
        for path in [
            root().unwrap().join("session-reader/history.db"),
            root().unwrap().join("session-preview/catalog.db"),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "cached private text").unwrap();
        }
        assert!(recover_with(&directory, |_| anyhow::bail!("offline")).is_err());
        assert!(pending.exists());
        assert!(history::captured_sessions(&db_path).unwrap() > 0);
        recover_with(&directory, |p| {
            assert_eq!(p.request_id, "durable-reset");
            Ok(())
        })
        .unwrap();
        assert!(!pending.exists());
        assert!(source.exists());
        assert!(!root().unwrap().join("session-reader/history.db").exists());
        assert_eq!(
            fs::read_to_string(&acquisition).unwrap(),
            r#"{"version":1,"installations":[],"grants":[]}"#
        );
        assert!(!root().unwrap().join("session-preview/catalog.db").exists());
        assert_eq!(history::captured_sessions(&db_path).unwrap(), 0);
        let config = read_config(&directory).unwrap();
        assert_eq!(
            bridge::mode(&config),
            crate::contract::account::SharingMode::Selected
        );
        let conn = delivery::open_db(&db_path).unwrap();
        assert!(delivery::is_session_job(&conn, &config.job_id).unwrap());
        drop(conn);
        history::discover(&db_path, None, || false).unwrap();
        let store = history::open_read(&db_path).unwrap();
        assert!(history::catalog(&store, ai_hist::SessionScope::All, 100)
            .unwrap()
            .is_empty());
        drop(store);
        fs::write(
            source.parent().unwrap().join("new.jsonl"),
            crate::history::probe::live::tests::transcript_line("new", "fresh"),
        )
        .unwrap();
        history::discover(&db_path, None, || false).unwrap();
        let store = history::open_read(&db_path).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let old: i64 = conn
            .query_row(
                "SELECT count(*) FROM sessions WHERE session_id='old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
        let rows = history::catalog(&store, ai_hist::SessionScope::All, 1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "new");
    }
}
