//! Native Copilot journals must keep growing through the live watcher.
use super::tests::{append, eventually, isolated_home, job_config, options};
use super::*;
use std::fs;

#[test]
fn copilot_appends_after_a_large_catalog_head_are_captured_by_native_events() {
    let Some(home) = isolated_home("history::probe::live::copilot_append_tests::copilot_appends_after_a_large_catalog_head_are_captured_by_native_events") else { return; };
    let directory = home.join("probe");
    let db = directory.join("history.db");
    delivery::create_job(&delivery::open_db(&db).unwrap(), &job_config(), 0).unwrap();
    let sid = "c72176be-2b94-4c6f-84aa-7b2f93f7da00";
    let journal = home
        .join(".copilot/session-state")
        .join(sid)
        .join("events.jsonl");
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    for value in [
        json!({"type":"session.start","id":"native-start","parentId":null,"timestamp":"2026-11-16T12:00:00Z","data":{"sessionId":sid,"version":1,"producer":"copilot-agent","copilotVersion":"1.4.2","context":{"cwd":"/fixture/copilot"}}}),
        json!({"type":"system.message","id":"large-system","parentId":"native-start","timestamp":"2026-11-16T12:00:01Z","data":{"content":"x".repeat(80 * 1024)}}),
        message("first-message", "initial question", "2026-11-16T12:00:02Z"),
    ] {
        append(&journal, &format!("{value}\n"));
    }

    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    shared.update(|state| state.metrics.mode = Some("watch"));
    let (notices, received) = mpsc::channel();
    let opts = LiveOptions {
        duty_factor: 0,
        cold_duty_factor: 0,
        fallback_poll: Duration::from_secs(60),
        backstop: Duration::from_secs(60),
        watched_backstop: Duration::from_secs(60),
        ..options()
    };
    let broad::Started {
        store,
        roots,
        watch,
    } = broad::start(history::open(&db).unwrap(), notices, &stop, &shared, &opts).unwrap();
    let capture = Capture {
        stop: stop.clone(),
        thread: Some(std::thread::spawn({
            let directory = directory.clone();
            let shared = shared.clone();
            move || {
                broad::follow(
                    broad::Context {
                        directory: &directory,
                        store,
                        watch,
                        received: &received,
                        last: Swept {
                            ended: Instant::now(),
                            took: Duration::ZERO,
                        },
                        cancelled: &stop,
                        shared: &shared,
                        progress: None,
                        options: &opts,
                    },
                    roots,
                )
            }
        })),
    };
    eventually("initial Copilot evidence", Duration::from_secs(20), || {
        has(&db, "first-message")
    });
    let before = shared.report().unwrap()["ticks_by_trigger"]["fs-event"]
        .as_u64()
        .unwrap_or(0);
    append(
        &journal,
        &format!(
            "{}\n",
            message(
                "late-message",
                "native append after the catalog head",
                "2026-11-16T12:00:03Z"
            )
        ),
    );
    eventually("native Copilot append", Duration::from_secs(10), || {
        has(&db, "late-message")
            && shared.report().unwrap()["ticks_by_trigger"]["fs-event"]
                .as_u64()
                .unwrap_or(0)
                > before
    });
    assert!(
        shared.report().unwrap()["ticks_by_trigger"]["fs-event"]
            .as_u64()
            .unwrap_or(0)
            > before
    );
    drop(capture);
}

fn message(id: &str, text: &str, at: &str) -> serde_json::Value {
    json!({"type":"user.message","id":id,"parentId":null,"timestamp":at,"data":{"content":text}})
}

fn has(db: &Path, message: &str) -> bool {
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_events WHERE source='copilot' AND message_id=?)",
            [message],
            |row| row.get(0),
        )
        .unwrap()
}
