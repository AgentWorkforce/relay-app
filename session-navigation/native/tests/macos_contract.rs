//! The macOS adapter against the current core contract: platform, surfaces,
//! the new executable classes, steps it cannot perform, and Grok through
//! `MacObserver`.
#![cfg(feature = "macos")]
mod common;

use std::collections::BTreeMap;

use common::*;
use session_navigation::evidence::{Exe, Fact, ProviderEvidence};
use session_navigation::hosts::{Ability, PaneRef, Setting, TerminalSupport, Unsupported};
use session_navigation::model::{
    App, BootId, Platform, ProcessIdentity, Provider, Reason, Runtime, SessionRef, TerminalKind,
};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::{Execute, Observe};
use session_navigation_native::macos::observe::surfaces;
use session_navigation_native::macos::parse::ps;
use session_navigation_native::system::FakeSystem;
use session_navigation_native::{MacExecutor, MacObserver};

#[test]
fn classifies_the_new_executables() {
    let cases = [
        ("grok", "grok", Exe::GrokCli),
        (
            "grok-0.1.23-dar",
            "/Users/example/.grok/bin/grok",
            Exe::GrokCli,
        ),
        ("tmux", "tmux", Exe::Tmux),
        (
            "iTerm2",
            "/Applications/iTerm.app/Contents/MacOS/iTerm2",
            Exe::ITerm2,
        ),
        (
            "kitty",
            "/Applications/kitty.app/Contents/MacOS/kitty",
            Exe::Kitty,
        ),
        (
            "wezterm-gui",
            "/Applications/WezTerm.app/Contents/MacOS/wezterm-gui",
            Exe::WezTerm,
        ),
        (
            "alacritty",
            "/Applications/Alacritty.app/Contents/MacOS/alacritty",
            Exe::Alacritty,
        ),
        // App names without their bundle path are not the app.
        ("iTerm2", "iTerm2", Exe::Other),
        ("kitty", "/usr/local/bin/kitty", Exe::Other),
        ("grokker", "grokker", Exe::Other),
    ];
    for (ucomm, comm, exe) in cases {
        assert_eq!(ps::classify(ucomm, comm), exe, "{ucomm} {comm}");
    }
    assert!(ps::is_agent_cli(Exe::GrokCli));
    assert!(!ps::is_agent_cli(Exe::Tmux));
}

#[test]
fn snapshot_shape_on_macos() {
    let snap = MacObserver::new(&Machine::new().build(), cfg()).observe(&codex());
    assert_eq!(snap.platform, Platform::MacOs);
    assert_eq!(snap.runtime, Fact::Known(Runtime::Native));
    assert_eq!(snap.terminal_profile, Fact::Known(None));
    assert_eq!(snap.original_host, Fact::Unknown);
    assert_eq!(
        (snap.front.pane, snap.front.window),
        (Fact::Known(None), Fact::Known(None))
    );
    let table = snap.processes.known().cloned().unwrap_or_default();
    assert!(!table.is_empty() && table.iter().all(|p| p.window.is_none()));
    assert_eq!(snap.surfaces, surfaces());
    let apps = snap.apps.known().cloned().unwrap_or_default();
    assert!(apps
        .iter()
        .all(|a| a.app.handler(Platform::MacOs).is_some()));
    assert!(apps.iter().any(|a| a.app == App::ITerm2 && !a.installed));
}

#[test]
fn surfaces_without_probes_script_three_terminals() {
    // Ghostty, Terminal and iTerm2 are scripted by tty; Kitty and WezTerm
    // focus only once their API answers; Alacritty has no pane API.
    let surfaces = surfaces();
    for terminal in [
        TerminalKind::Ghostty,
        TerminalKind::AppleTerminal,
        TerminalKind::ITerm2,
    ] {
        assert_eq!(
            surfaces.support(terminal),
            Fact::Known(TerminalSupport::scripted(terminal))
        );
    }
    let no = Ability::Unsupported(Unsupported::NotImplemented);
    for (terminal, focus, tabs) in [
        (
            TerminalKind::Kitty,
            Ability::NeedsSetting(Setting::KittyRemoteControl),
            true,
        ),
        (
            TerminalKind::WezTerm,
            Ability::NeedsSetting(Setting::WezTermMuxSocket),
            true,
        ),
        (
            TerminalKind::Alacritty,
            Ability::Unsupported(Unsupported::NoPaneApi),
            false,
        ),
    ] {
        assert_eq!(
            surfaces.support(terminal),
            Fact::Known(TerminalSupport {
                terminal,
                focus_pane: focus,
                raise_window: no,
                activate_app: Ability::Available,
                launch: Ability::Available,
                tabs_share_window: tabs,
            })
        );
    }
    assert_eq!(surfaces.panes, Fact::Known(vec![]));
    assert_eq!(surfaces.tmux, Fact::Known(vec![]));
    assert_eq!(surfaces.windows, Fact::Known(vec![]));
}

fn exec(fake: &FakeSystem, step: &Step) -> Dispatch {
    MacExecutor::new(fake, cfg()).execute(step)
}

fn identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 998,
        start: BOOT + 1998,
        boot: BootId(BOOT),
    }
}

fn command() -> Command {
    Command {
        program: "/opt/homebrew/bin/codex".into(),
        args: vec!["resume".into(), CODEX_ID.into()],
        cwd: PROJECT.into(),
        env: BTreeMap::new(),
    }
}

#[test]
fn steps_it_cannot_perform_run_nothing() {
    let tty = PaneRef::Tty {
        tty: "/dev/ttys000".into(),
    };
    let wsl = LaunchContext {
        runtime: Runtime::Wsl {
            distro: "Ubuntu".into(),
        },
        profile: None,
    };
    let profile = LaunchContext {
        runtime: Runtime::Native,
        profile: Some("{guid}".into()),
    };
    let cases = [
        (
            Step::FocusPane {
                terminal: TerminalKind::Ghostty,
                pane: PaneRef::KittyWindow {
                    socket: "unix:/tmp/kitty".into(),
                    id: 1,
                },
                process: identity(),
            },
            Reason::HostUnsupported,
        ),
        (
            Step::FocusPane {
                terminal: TerminalKind::Alacritty,
                pane: tty.clone(),
                process: identity(),
            },
            Reason::HostUnsupported,
        ),
        (
            Step::FocusPane {
                terminal: TerminalKind::Kitty,
                pane: PaneRef::WezTermPane {
                    socket: "/tmp/sock".into(),
                    id: 1,
                },
                process: identity(),
            },
            Reason::HostUnsupported,
        ),
        (
            Step::Launch {
                terminal: TerminalKind::GnomeTerminal,
                command: command(),
                context: LaunchContext::native(),
            },
            Reason::HostUnsupported,
        ),
        (
            Step::Launch {
                terminal: TerminalKind::Ghostty,
                command: command(),
                context: wsl,
            },
            Reason::PlatformUnsupported,
        ),
        (
            Step::Launch {
                terminal: TerminalKind::Ghostty,
                command: command(),
                context: profile,
            },
            Reason::PlatformUnsupported,
        ),
        (
            Step::ActivateApp {
                app: App::WindowsTerminal,
            },
            Reason::PlatformUnsupported,
        ),
    ];
    for (step, reason) in cases {
        let fake = Machine::new().build();
        assert_eq!(exec(&fake, &step), Dispatch::Failed(reason), "{step:?}");
        assert!(fake.runs().is_empty(), "{step:?}");
    }
}

#[test]
fn grok_through_the_mac_observer() {
    let id = "0b5a3c1e-1111-4222-8333-444455556666";
    let mut m = Machine::new();
    m.dir(PROJECT);
    m.file(
        &format!("{HOME}/.grok/sessions/%2FUsers%2Fexample%2Fcode%2Fproject/{id}/summary.json"),
        serde_json::json!({ "info": { "id": id, "cwd": PROJECT } })
            .to_string()
            .into_bytes(),
    );
    let snap = MacObserver::new(&m.build(), cfg()).observe(&SessionRef::new(Provider::Grok, id));
    assert!(matches!(snap.provider, ProviderEvidence::Grok(_)));
    assert_eq!(snap.cwd, Fact::Known(PROJECT.to_owned()));
    assert_eq!(snap.cwd_exists, Fact::Known(true));
    assert_eq!(snap.environment, Fact::Known(BTreeMap::new()));
}

#[test]
fn grok_under_grok_home_is_recorded_with_it() {
    let id = "0b5a3c1e-1111-4222-8333-444455556666";
    let alt = "/Users/example/grok-alt";
    let mut m = Machine::new();
    m.dir(PROJECT);
    m.file(
        &format!("{alt}/sessions/%2FUsers%2Fexample%2Fcode%2Fproject/{id}/summary.json"),
        serde_json::json!({ "info": { "id": id, "cwd": PROJECT } })
            .to_string()
            .into_bytes(),
    );
    let mut config = cfg();
    config.common.paths.grok_home =
        session_navigation_native::common::grok::home_from_env(Some("~/grok-alt"), HOME);
    let snap = MacObserver::new(&m.build(), config).observe(&SessionRef::new(Provider::Grok, id));
    assert_eq!(snap.cwd, Fact::Known(PROJECT.to_owned()));
    assert_eq!(
        snap.environment,
        Fact::Known(BTreeMap::from([("GROK_HOME".to_owned(), alt.to_owned())]))
    );
}
