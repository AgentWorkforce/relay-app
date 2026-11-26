//! The session-locations store as the engine's history: each recorded
//! shape maps to a host, and what is unknown stays unknown.
use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use session_navigation::evidence::Fact;
use session_navigation::model::{App, Host, TerminalKind};
use session_navigation::ports::Observe;
use session_navigation_native::{FakeSystem, HostHistory, MacObserver};

use super::support::*;
use crate::session_locations::SessionLocation;
use crate::session_open::history::{self, StoreHistory, STORE_FILE};

fn at(bundle: &str, application: &str) -> SessionLocation {
    SessionLocation {
        application: application.into(),
        bundle_id: bundle.into(),
        pid: 30,
        process_start: 31,
        host_pid: Some(10),
        tty: Some("/dev/ttys004".into()),
        runtime: Some("macos".into()),
        ..SessionLocation::default()
    }
}

#[test]
fn each_recorded_host_maps_to_the_core_host() {
    let terminals = [
        ("com.mitchellh.ghostty", TerminalKind::Ghostty),
        ("com.apple.Terminal", TerminalKind::AppleTerminal),
        ("com.googlecode.iterm2", TerminalKind::ITerm2),
        ("net.kovidgoyal.kitty", TerminalKind::Kitty),
        ("com.github.wez.wezterm", TerminalKind::WezTerm),
        ("org.alacritty", TerminalKind::Alacritty),
        ("org.gnome.Terminal", TerminalKind::GnomeTerminal),
        ("org.kde.konsole", TerminalKind::Konsole),
        ("xterm", TerminalKind::Xterm),
        ("Microsoft.WindowsTerminal", TerminalKind::WindowsTerminal),
    ];
    for (bundle, terminal) in terminals {
        assert_eq!(
            history::host(&at(bundle, "x")),
            Fact::Known(Host::Terminal(terminal)),
            "{bundle}"
        );
    }
    let desktop = |bundle| history::host(&at(bundle, "x"));
    assert_eq!(
        desktop("com.anthropic.claudefordesktop"),
        Fact::Known(Host::Desktop(App::ClaudeDesktop))
    );
    assert_eq!(
        desktop("com.openai.codex"),
        Fact::Known(Host::Desktop(App::CodexDesktop))
    );
    // tmux's server is the recorded ancestor; the terminal around it is not.
    assert_eq!(desktop("tmux"), Fact::Known(Host::Tmux(None)));
}

#[test]
fn hosts_nothing_models_stay_unknown() {
    for bundle in [
        "dev.warp.Warp-Stable",
        "com.microsoft.VSCode",
        "dev.zed.Zed",
    ] {
        assert_eq!(history::host(&at(bundle, "x")), Fact::Unknown, "{bundle}");
    }
    // A process with a tty but no recognised terminal is somewhere unknown.
    let mut background = at("", "Background process");
    background.host_pid = None;
    assert_eq!(history::host(&background), Fact::Unknown);
    // No tty, pane or window on Unix: headless.
    background.tty = None;
    assert_eq!(history::host(&background), Fact::Known(Host::Headless));
    // Windows never records a tty, so the same shape proves nothing there.
    background.runtime = Some("windows".into());
    assert_eq!(history::host(&background), Fact::Unknown);
    // A record from before runtimes were captured proves nothing either.
    background.runtime = None;
    assert_eq!(history::host(&background), Fact::Unknown);
    background.runtime = Some("linux".into());
    background.window_id = Some("0x1".into());
    assert_eq!(history::host(&background), Fact::Unknown);
}

#[test]
fn windows_terminal_profile_is_known_only_when_read() {
    let mut wt = at("Microsoft.WindowsTerminal", "Windows Terminal");
    wt.runtime = Some("windows".into());
    assert_eq!(history::profile(Some(&wt)), Fact::Unknown);
    wt.provider_environment_known = Some(true);
    assert_eq!(history::profile(Some(&wt)), Fact::Known(None));
    wt.profile_id = Some("{61c54bbd-c2c6-5271-96e7-009a87ff44bf}".into());
    assert_eq!(
        history::profile(Some(&wt)),
        Fact::Known(Some("{61c54bbd-c2c6-5271-96e7-009a87ff44bf}".into()))
    );
    assert_eq!(
        history::profile(Some(&at("com.mitchellh.ghostty", "Ghostty"))),
        Fact::Known(None)
    );
    assert_eq!(history::profile(None), Fact::Unknown);
}

#[test]
fn the_recorded_environment_is_known_only_when_read() {
    let mut l = at("com.mitchellh.ghostty", "Ghostty");
    assert_eq!(history::environment(Some(&l)), Fact::Unknown);
    l.provider_environment_known = Some(false);
    assert_eq!(history::environment(Some(&l)), Fact::Unknown);
    l.provider_environment_known = Some(true);
    l.provider_environment = Some(BTreeMap::from([
        ("CODEX_HOME".into(), "/x".into()),
        ("NOT_A_STORE".into(), "/y".into()),
    ]));
    assert_eq!(
        history::environment(Some(&l)),
        Fact::Known(BTreeMap::from([("CODEX_HOME".into(), "/x".into())]))
    );
}

fn store(dir: &std::path::Path, rows: &[(&str, &str, &SessionLocation, &SessionLocation)]) {
    let db = Connection::open(dir.join(STORE_FILE)).unwrap();
    db.execute_batch(
        "CREATE TABLE origins(source TEXT NOT NULL, session_id TEXT NOT NULL, original TEXT NOT NULL, latest TEXT NOT NULL, PRIMARY KEY(source,session_id)); PRAGMA user_version=1;",
    )
    .unwrap();
    for (source, id, original, latest) in rows {
        db.execute(
            "INSERT INTO origins VALUES (?1,?2,?3,?4)",
            params![
                source,
                id,
                serde_json::to_string(original).unwrap(),
                serde_json::to_string(latest).unwrap()
            ],
        )
        .unwrap();
    }
}

#[test]
fn the_store_feeds_original_and_latest_hosts_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let original = at("com.apple.Terminal", "Terminal");
    let latest = at("com.mitchellh.ghostty", "Ghostty");
    store(dir.path(), &[("claude", CLAUDE_ID, &original, &latest)]);
    let before = std::fs::read(dir.path().join(STORE_FILE)).unwrap();
    let history = StoreHistory::load(dir.path(), &claude(), &[CLAUDE_ID]);
    assert_eq!(
        history.last_seen(&claude()),
        Fact::Known(Host::Terminal(TerminalKind::Ghostty))
    );
    assert_eq!(
        history.original(&claude()),
        Fact::Known(Host::Terminal(TerminalKind::AppleTerminal))
    );
    assert_eq!(std::fs::read(dir.path().join(STORE_FILE)).unwrap(), before);
    // Another session asked of this history is unknown.
    let other = session_navigation::model::SessionRef::new(
        session_navigation::model::Provider::Codex,
        CODEX_ID,
    );
    assert_eq!(history.last_seen(&other), Fact::Unknown);
    // The observer carries it into the snapshot.
    let fake = FakeSystem::new();
    let mut observer = MacObserver::with_history(&fake, mac_cfg("/tmp/app"), history);
    let snapshot = observer.observe(&claude());
    assert_eq!(
        snapshot.last_seen_host,
        Fact::Known(Host::Terminal(TerminalKind::Ghostty))
    );
    assert_eq!(
        snapshot.original_host,
        Fact::Known(Host::Terminal(TerminalKind::AppleTerminal))
    );
}

#[test]
fn a_missing_old_or_unknown_store_is_unknown_history() {
    let dir = tempfile::tempdir().unwrap();
    let missing = StoreHistory::load(dir.path(), &claude(), &[CLAUDE_ID]);
    assert_eq!(missing.last_seen(&claude()), Fact::Unknown);
    assert!(!dir.path().join(STORE_FILE).exists(), "never created here");
    let db = Connection::open(dir.path().join(STORE_FILE)).unwrap();
    db.execute_batch("CREATE TABLE origins(x TEXT);").unwrap();
    drop(db);
    let old = StoreHistory::load(dir.path(), &claude(), &[CLAUDE_ID]);
    assert_eq!(old.original(&claude()), Fact::Unknown);
}

#[test]
fn the_store_is_found_under_the_id_as_given_or_in_lower_case() {
    let dir = tempfile::tempdir().unwrap();
    let latest = at("com.mitchellh.ghostty", "Ghostty");
    store(dir.path(), &[("claude", CLAUDE_ID, &latest, &latest)]);
    let upper = CLAUDE_ID.to_ascii_uppercase();
    let history = StoreHistory::load(dir.path(), &claude(), &[&upper, CLAUDE_ID]);
    assert!(history.latest.is_some());
}
