//! `decide` and the post-condition checks stay total over the terminal-host
//! space: random platforms, runtimes, terminal abilities, pane listings,
//! tmux servers, OS windows, Windows-shaped process tables and Grok
//! evidence. Every plan is well formed for its platform and runtime.
use session_navigation::decide::decide;
use session_navigation::engine::verify::check;
use session_navigation::evidence::*;
use session_navigation::hosts::*;
use session_navigation::model::*;
use session_navigation::plan::{ActionKind, Plan, Step};
use session_navigation::sim::rng::Rng;

use super::totality::{fact, provider_evidence, request, snapshot};

const NEW_EXES: [Exe; 13] = [
    Exe::GrokCli,
    Exe::ITerm2,
    Exe::Kitty,
    Exe::WezTerm,
    Exe::Alacritty,
    Exe::GnomeTerminal,
    Exe::Konsole,
    Exe::Xterm,
    Exe::WindowsTerminal,
    Exe::ConsoleHost,
    Exe::Tmux,
    Exe::Shell,
    Exe::ClaudeCli,
];

fn pick<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    items[rng.below(items.len() as u64) as usize]
}

fn ability(rng: &mut Rng) -> Ability {
    pick(
        rng,
        &[
            Ability::Available,
            Ability::Available,
            Ability::NeedsSetting(Setting::KittyRemoteControl),
            Ability::NeedsPermission(Permission::MacAutomation),
            Ability::Unsupported(Unsupported::NoPaneApi),
            Ability::Unsupported(Unsupported::Wayland),
            Ability::Unsupported(Unsupported::NotImplemented),
        ],
    )
}

fn ident(rng: &mut Rng) -> ProcessIdentity {
    ProcessIdentity {
        pid: 1 + rng.below(12) as u32,
        start: rng.below(3),
        boot: BootId(rng.below(2)),
    }
}

fn pane_ref(rng: &mut Rng, depth: u32) -> PaneRef {
    match rng.below(if depth > 1 { 5 } else { 6 }) {
        0 => PaneRef::Tty {
            tty: format!("/dev/ttys00{}", rng.below(3)),
        },
        1 => PaneRef::KittyWindow {
            socket: "unix:/tmp/kitty-1".into(),
            id: rng.below(3),
        },
        2 => PaneRef::WezTermPane {
            socket: "/run/wez".into(),
            id: rng.below(3),
        },
        3 => PaneRef::X11Window {
            id: WindowId(rng.below(3)),
            owner: ident(rng),
        },
        4 => PaneRef::WindowsWindow {
            hwnd: WindowId(rng.below(3)),
            owner: ident(rng),
        },
        _ => PaneRef::TmuxPane {
            socket: "/tmp/tmux-501/default".into(),
            server: ident(rng),
            pane: format!("%{}", rng.below(3)),
            client_tty: format!("/dev/ttys00{}", rng.below(3)),
            outer: Box::new(pane_ref(rng, depth + 1)),
        },
    }
}

fn surfaces(rng: &mut Rng) -> Surfaces {
    let chosen: Vec<TerminalKind> = TerminalKind::ALL
        .into_iter()
        .filter(|_| rng.chance(70))
        .collect();
    let terminals = chosen
        .into_iter()
        .map(|terminal| TerminalSupport {
            terminal,
            focus_pane: ability(rng),
            raise_window: ability(rng),
            activate_app: ability(rng),
            launch: ability(rng),
            tabs_share_window: rng.chance(50),
        })
        .collect();
    let panes = (0..rng.below(4))
        .map(|_| PaneInfo {
            terminal: pick(rng, &[TerminalKind::Kitty, TerminalKind::WezTerm]),
            pane: pane_ref(rng, 2),
            tty: rng
                .chance(50)
                .then(|| format!("/dev/ttys00{}", rng.below(3))),
            pid: rng.chance(70).then(|| 1 + rng.below(12) as u32),
        })
        .collect();
    let tmux = (0..rng.below(2))
        .map(|_| TmuxServer {
            socket: "/tmp/tmux-501/default".into(),
            server: ident(rng),
            panes: (0..rng.below(3))
                .map(|i| TmuxPaneInfo {
                    pane: format!("%{i}"),
                    tty: format!("/dev/ttys00{}", rng.below(3)),
                    session: format!("${}", rng.below(2)),
                    window: format!("@{}", rng.below(2)),
                })
                .collect(),
            clients: (0..rng.below(3))
                .map(|_| TmuxClientInfo {
                    tty: format!("/dev/ttys00{}", rng.below(3)),
                    pid: 1 + rng.below(12) as u32,
                    session: format!("${}", rng.below(2)),
                    active_pane: format!("%{}", rng.below(3)),
                })
                .collect(),
        })
        .collect();
    let windows = (0..rng.below(4))
        .map(|_| WindowInfo {
            id: WindowId(rng.below(3)),
            owner: ident(rng),
        })
        .collect();
    Surfaces {
        terminals: fact(rng.chance(10), terminals),
        panes: fact(rng.chance(10), panes),
        tmux: fact(rng.chance(10), tmux),
        windows: fact(rng.chance(10), windows),
    }
}

fn host_snapshot(rng: &mut Rng) -> Snapshot {
    let mut s = snapshot(rng);
    s.platform = pick(
        rng,
        &[
            Platform::MacOs,
            Platform::Linux {
                session: Fact::Known(DisplayServer::X11),
            },
            Platform::Linux {
                session: Fact::Known(DisplayServer::Wayland),
            },
            Platform::Linux {
                session: Fact::Unknown,
            },
            Platform::Windows,
        ],
    );
    s.runtime = match rng.below(6) {
        0 => Fact::Unknown,
        1 => Fact::Known(Runtime::Wsl {
            distro: "Ubuntu".into(),
        }),
        2 => Fact::Known(Runtime::Foreign {
            label: "ssh".into(),
        }),
        _ => Fact::Known(Runtime::Native),
    };
    s.surfaces = surfaces(rng);
    s.terminal_profile = fact(rng.chance(15), rng.chance(50).then(|| "{p}".to_owned()));
    s.original_host = fact(
        rng.chance(30),
        Host::Terminal(pick(rng, &TerminalKind::ALL)),
    );
    s.front.pane = fact(rng.chance(20), rng.chance(50).then(|| pane_ref(rng, 2)));
    s.front.window = fact(
        rng.chance(20),
        rng.chance(50).then(|| WindowId(rng.below(3))),
    );
    s.front.app = fact(rng.chance(20), rng.chance(70).then(|| pick(rng, &App::ALL)));
    if rng.chance(20) {
        s.apps = Fact::Unknown;
    }
    if rng.chance(20) {
        s.schemes = Fact::Unknown;
    }
    if let Fact::Known(table) = &mut s.processes {
        for p in table.iter_mut() {
            if rng.chance(40) {
                p.exe = pick(rng, &NEW_EXES);
            }
            if s.platform == Platform::Windows && rng.chance(80) {
                p.tty = None;
            }
            p.window = rng.chance(30).then(|| WindowId(rng.below(3)));
        }
    }
    if rng.chance(30) {
        s.session = SessionRef::new(Provider::Grok, "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeee1");
        s.provider = provider_evidence(rng, Provider::Grok, &s.session.id);
        s.tools.push(ToolInfo {
            tool: Tool::GrokCli,
            version: Fact::Known(Some("1.0.46".into())),
            path: Some("/usr/local/bin/grok".into()),
        });
    }
    if rng.chance(15) {
        tmux_world(rng, &mut s);
    }
    s
}

/// A coherent Grok pane in tmux, attached or not, so tmux plans occur.
fn tmux_world(rng: &mut Rng, s: &mut Snapshot) {
    let boot = BootId(1);
    s.boot = Fact::Known(boot);
    let p = |pid, ppid, exe, tty: Option<&str>, argv: &[&str]| ProcessInfo {
        identity: ProcessIdentity {
            pid,
            start: 10,
            boot,
        },
        ppid,
        euid: 501,
        ruid: 501,
        exe,
        tty: tty.map(str::to_owned),
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        cwd: Fact::Known("/Users/example/code/alpha".into()),
        window: None,
    };
    let id = "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeee2";
    let attached = rng.chance(50);
    let mut table = vec![
        p(1, 0, Exe::Other, None, &["launchd"]),
        p(50, 1, Exe::Tmux, None, &["tmux"]),
        p(
            51,
            50,
            Exe::GrokCli,
            Some("/dev/ttys009"),
            &["grok", "--resume", id],
        ),
        p(60, 1, Exe::Ghostty, None, &["ghostty"]),
        p(61, 60, Exe::Shell, Some("/dev/ttys001"), &["zsh"]),
    ];
    if attached {
        table.push(p(
            62,
            61,
            Exe::Tmux,
            Some("/dev/ttys001"),
            &["tmux", "attach"],
        ));
    }
    s.session = SessionRef::new(Provider::Grok, id);
    s.provider = ProviderEvidence::Grok(GrokEvidence {
        active: Fact::Known(vec![]),
        session: Fact::Known(Some(GrokSession {
            cwd: "/Users/example/code/alpha".into(),
            subagent: false,
        })),
    });
    s.processes = Fact::Known(table);
    s.runtime = Fact::Known(Runtime::Native);
    s.platform = Platform::MacOs;
    s.reservation = Fact::Known(None);
    s.tools.push(ToolInfo {
        tool: Tool::Tmux,
        version: Fact::Known(Some("3.5a".into())),
        path: Some("/opt/homebrew/bin/tmux".into()),
    });
    s.surfaces.terminals = Fact::Known(vec![TerminalSupport::scripted(TerminalKind::Ghostty)]);
    s.surfaces.tmux = Fact::Known(vec![TmuxServer {
        socket: "/tmp/tmux-501/default".into(),
        server: ProcessIdentity {
            pid: 50,
            start: 10,
            boot,
        },
        panes: vec![TmuxPaneInfo {
            pane: "%3".into(),
            tty: "/dev/ttys009".into(),
            session: "$1".into(),
            window: "@1".into(),
        }],
        clients: if attached {
            vec![TmuxClientInfo {
                tty: "/dev/ttys001".into(),
                pid: 62,
                session: "$1".into(),
                active_pane: "%3".into(),
            }]
        } else {
            vec![]
        },
    }]);
}

fn check_ref(platform: Platform, pane: &PaneRef) {
    match pane {
        PaneRef::X11Window { .. } => assert!(
            platform
                == Platform::Linux {
                    session: Fact::Known(DisplayServer::X11)
                },
            "{pane:?} on {platform:?}"
        ),
        PaneRef::WindowsWindow { .. } => assert_eq!(platform, Platform::Windows, "{pane:?}"),
        PaneRef::TmuxPane { outer, .. } => check_ref(platform, outer),
        _ => {}
    }
}

#[test]
fn decide_is_total_and_well_formed_over_terminal_hosts() {
    let mut rng = Rng::new(2027);
    let mut acted = 0;
    let mut kinds = std::collections::BTreeSet::new();
    for _ in 0..120_000 {
        let snap = host_snapshot(&mut rng);
        let req = request(&mut rng);
        let d = decide(&snap, &req);
        let actions = match &d.plan {
            Plan::Act(a) => vec![a],
            Plan::Choose(list) => {
                assert!(list.len() > 1 && list.iter().all(|a| !a.starts_process()));
                list.iter().collect()
            }
            Plan::Confirm(c) => vec![&c.then],
            Plan::Unavailable(_) => vec![],
        };
        for a in actions {
            acted += 1;
            kinds.insert(format!("{:?}", a.kind));
            assert!(!a.steps.is_empty());
            if a.kind == ActionKind::BringForward {
                assert_eq!(a.precision, Precision::AppOnly);
            }
            for step in &a.steps {
                match step {
                    Step::FocusPane { pane, .. } => check_ref(snap.platform, pane),
                    Step::Launch { context, .. } => {
                        assert!(snap.runtime.is_known(), "{d:?}");
                        match &context.runtime {
                            Runtime::Native => {
                                assert_eq!(snap.runtime, Fact::Known(Runtime::Native))
                            }
                            Runtime::Wsl { .. } => assert_eq!(snap.platform, Platform::Windows),
                            Runtime::Foreign { .. } => panic!("launch into {context:?}"),
                        }
                        assert!(snap.apps.is_known(), "{d:?}");
                    }
                    Step::OpenUrl { .. } => assert!(snap.schemes.is_known(), "{d:?}"),
                    _ => {}
                }
            }
            // The post-condition check is total on any snapshot.
            let _ = check(&a.post, &snap, &req);
            let other = host_snapshot(&mut rng);
            let _ = check(&a.post, &other, &req);
        }
        if snap.session.provider == Provider::Grok && snap.platform == Platform::Windows {
            assert!(
                matches!(d.plan, Plan::Unavailable(_)),
                "Grok is not offered on Windows: {d:?}"
            );
        }
    }
    eprintln!("plans: {acted} {kinds:?}");
    assert!(acted > 3_000, "too few plans: {acted}");
    for kind in ["FocusPane", "BringForward", "Resume", "Attach"] {
        assert!(kinds.contains(kind), "no {kind} plan: {kinds:?}");
    }
}
