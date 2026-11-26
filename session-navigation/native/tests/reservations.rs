//! Reservation files: round trip, corruption, and hostile ids.
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::{Fact, Pending, Reservation};
use session_navigation::model::{BootId, Provider, SessionRef};
use session_navigation::ports::{Observe, Reservations};
use session_navigation_native::common::reservations::{file_name, session_of, FileReservations};
use session_navigation_native::system::{Call, FakeSystem};
use session_navigation_native::MacObserver;

fn reservation(session: SessionRef) -> Reservation {
    Reservation {
        session,
        made_at: NOW,
        tty: Some("/dev/ttys004".into()),
        seen: true,
        boot: Some(BootId(BOOT)),
        via: Pending::Launch,
    }
}

#[test]
fn reserve_observe_release_round_trip() {
    let fake = Machine::new().build();
    let dir = cfg().common.paths.reservation_dir;
    let mut store = FileReservations::new(&fake, &dir);
    assert_eq!(store.reserve(reservation(codex())), Ok(()));
    let calls = fake.calls();
    assert!(calls.contains(&Call::CreateDir(dir.clone())));
    let name = file_name(&codex()).unwrap_or_default();
    assert!(calls.contains(&Call::WriteAtomic(format!("{dir}/{name}"))));
    let snap = MacObserver::new(&fake, cfg()).observe(&codex());
    assert_eq!(snap.reservation, Fact::Known(Some(reservation(codex()))));
    assert_eq!(store.release(&codex()), Ok(()));
    assert_eq!(store.release(&codex()), Ok(()), "releasing twice is fine");
    assert_eq!(store.read(&codex()), Fact::Known(None));
}

#[test]
fn corrupt_or_foreign_files_are_unknown() {
    let fake = FakeSystem::new();
    let dir = cfg().common.paths.reservation_dir;
    let name = file_name(&codex()).unwrap_or_default();
    let store = FileReservations::new(&fake, &dir);
    for bytes in [
        b"{".to_vec(),
        b"".to_vec(),
        serde_json::to_vec(&reservation(claude())).unwrap_or_default(),
    ] {
        fake.add_file(&format!("{dir}/{name}"), bytes, 1);
        assert_eq!(store.read(&codex()), Fact::Unknown);
    }
    fake.make_unreadable(&format!("{dir}/{name}"));
    assert_eq!(store.read(&codex()), Fact::Unknown);
}

#[test]
fn store_failures_are_errors() {
    let fake = FakeSystem::new();
    fake.make_read_only();
    let mut store = FileReservations::new(&fake, &cfg().common.paths.reservation_dir);
    assert!(store.reserve(reservation(codex())).is_err());
    let fake = FakeSystem::new();
    fake.add_file(
        &cfg().common.paths.reservation_dir,
        b"not a dir".to_vec(),
        1,
    );
    let mut store = FileReservations::new(&fake, &cfg().common.paths.reservation_dir);
    assert!(store.reserve(reservation(codex())).is_err());
}

#[test]
fn hostile_ids_cannot_escape_the_directory() {
    let fake = FakeSystem::new();
    let dir = cfg().common.paths.reservation_dir;
    let mut store = FileReservations::new(&fake, &dir);
    for id in [
        "../../../../etc/passwd",
        "/abs",
        "a/b",
        ".",
        "..",
        "x\0y",
        "ses_\n",
        "日本",
        "%2e%2e",
    ] {
        let session = SessionRef::new(Provider::OpenCode, id);
        let name = file_name(&session).unwrap_or_default();
        assert!(name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'));
        assert!(!name.contains('/') && !name.starts_with('.'));
        assert_eq!(session_of(&name), Some(session.clone()));
        assert_eq!(store.reserve(reservation(session)), Ok(()));
    }
    for call in fake.calls() {
        if let Call::WriteAtomic(path) = call {
            let rest = path.strip_prefix(&format!("{dir}/")).unwrap_or_default();
            assert!(!rest.is_empty() && !rest.contains('/'), "{path}");
        }
    }
    assert_eq!(fake.files_under(&dir).len(), 9);
    // Empty and oversized ids get no file at all.
    assert_eq!(file_name(&SessionRef::new(Provider::Codex, "")), None);
    assert!(store
        .reserve(reservation(SessionRef::new(
            Provider::Codex,
            "a".repeat(101)
        )))
        .is_err());
    for bad in [
        "opencode-zz.json",
        "opencode-6.json",
        "nope-61.json",
        "opencode-61",
        "codex-.json",
    ] {
        assert_eq!(session_of(bad), None, "{bad}");
    }
}
