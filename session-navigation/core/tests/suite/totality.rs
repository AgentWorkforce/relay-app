//! `decide` is total: random snapshots (including nonsense the simulator
//! would never produce, such as parent cycles and contradictory facts) always
//! yield a well-formed plan or a specific reason, and never a launch on
//! incomplete evidence.
use session_navigation::decide::decide;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::plan::{ActionKind, OpenRequest, Plan, Preferences, Step, Target};
use session_navigation::sim::rng::Rng;

pub(super) fn fact<T>(unknown: bool, value: T) -> Fact<T> {
    if unknown {
        Fact::Unknown
    } else {
        Fact::Known(value)
    }
}

const EXES: [Exe; 11] = [
    Exe::ClaudeCli,
    Exe::ClaudeDesktopApp,
    Exe::CodexCli,
    Exe::CodexDesktopApp,
    Exe::OpenCodeCli,
    Exe::OpenCodeDesktopApp,
    Exe::Ghostty,
    Exe::AppleTerminal,
    Exe::Login,
    Exe::Shell,
    Exe::Other,
];

fn id(rng: &mut Rng, provider: Provider) -> String {
    match rng.below(4) {
        0 => "garbage id".to_owned(),
        _ => match provider {
            Provider::Claude => {
                "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeee".to_owned() + &rng.below(10).to_string()
            }
            Provider::Codex => format!("01900000-0000-7000-8000-00000000000{}", rng.below(10)),
            Provider::OpenCode => format!("ses_0123456789abAAAAAAAAAAAAA{}", rng.below(10)),
            Provider::Grok => {
                "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeee".to_owned() + &rng.below(10).to_string()
            }
        },
    }
}

fn argv(rng: &mut Rng, session: &str) -> Vec<String> {
    let words = [
        "claude",
        "codex",
        "opencode",
        "resume",
        "--resume",
        "-s",
        "exec",
        "app-server",
        "run",
        "serve",
        session,
    ];
    (0..rng.below(5))
        .map(|_| words[rng.below(words.len() as u64) as usize].to_owned())
        .collect()
}

fn table(rng: &mut Rng, session: &str) -> Vec<ProcessInfo> {
    let n = rng.below(12);
    (0..n)
        .map(|i| ProcessInfo {
            identity: ProcessIdentity {
                pid: 1 + i as u32,
                start: rng.below(3),
                boot: BootId(rng.below(2)),
            },
            ppid: rng.below(n + 2) as u32,
            euid: if rng.chance(20) { 0 } else { 501 },
            ruid: if rng.chance(10) { 0 } else { 501 },
            exe: EXES[rng.below(11) as usize],
            tty: rng
                .chance(50)
                .then(|| format!("/dev/ttys00{}", rng.below(3))),
            argv: argv(rng, session),
            cwd: fact(rng.chance(15), "/Users/example/code/alpha".to_owned()),
            window: None,
        })
        .collect()
}

pub(super) fn provider_evidence(
    rng: &mut Rng,
    provider: Provider,
    session: &str,
) -> ProviderEvidence {
    match provider {
        Provider::Claude => ProviderEvidence::Claude(ClaudeEvidence {
            registry: fact(
                rng.chance(15),
                (0..rng.below(4))
                    .map(|i| RegistryClaim {
                        pid: 1 + i as u32,
                        proc_start: rng.below(3),
                        written_this_boot: fact(rng.chance(15), rng.chance(70)),
                        session_id: if rng.chance(70) {
                            session.to_owned()
                        } else {
                            "other".to_owned()
                        },
                        host_session: rng.chance(40).then(|| "local_x".to_owned()),
                        spare: rng.chance(20),
                        entrypoint: [Entrypoint::Cli, Entrypoint::Desktop, Entrypoint::Sdk]
                            [rng.below(3) as usize],
                    })
                    .collect(),
            ),
            desktop: fact(
                rng.chance(15),
                (0..rng.below(3))
                    .map(|_| DesktopClaim {
                        local_id: if rng.chance(80) {
                            "local_x".to_owned()
                        } else {
                            "bad id".to_owned()
                        },
                        archived: rng.chance(40),
                        kind: [ClaimKind::Live, ClaimKind::Stem, ClaimKind::Prior]
                            [rng.below(3) as usize],
                        cwd: None,
                        account: if rng.chance(80) { "a/o" } else { "b/o" }.to_owned(),
                        current: rng.chance(50).then(|| "other".to_owned()),
                        last_focused: fact(rng.chance(15), rng.chance(50).then(|| rng.below(5))),
                    })
                    .collect(),
            ),
            transcript: fact(
                rng.chance(15),
                rng.chance(70).then(|| Transcript {
                    cwd: Some("/Users/example/code/alpha".to_owned()),
                    entrypoint: [Entrypoint::Cli, Entrypoint::Desktop, Entrypoint::Other]
                        [rng.below(3) as usize],
                }),
            ),
            account: fact(rng.chance(15), "a/o".to_owned()),
        }),
        Provider::Codex => ProviderEvidence::Codex(CodexEvidence {
            thread: fact(
                rng.chance(15),
                rng.chance(80).then(|| ThreadInfo {
                    archived: rng.chance(30),
                    origin: [CodexOrigin::Desktop, CodexOrigin::Tui, CodexOrigin::Exec]
                        [rng.below(3) as usize],
                    subagent: rng.chance(10),
                }),
            ),
            lock_holders: fact(
                rng.chance(15),
                (0..rng.below(3))
                    .map(|_| 1 + rng.below(12) as u32)
                    .collect(),
            ),
            window_owner: fact(rng.chance(15), rng.chance(30)),
        }),
        Provider::OpenCode => ProviderEvidence::OpenCode(OpenCodeEvidence {
            server: fact(
                rng.chance(15),
                if rng.chance(50) {
                    ServerState::Down
                } else {
                    ServerState::Up {
                        pid: 3,
                        version: "2.2.6".into(),
                    }
                },
            ),
            session: fact(
                rng.chance(15),
                rng.chance(80).then(|| OpenCodeSession {
                    archived: rng.chance(20),
                    child: rng.chance(20),
                    directory: "/Users/example/code/alpha".into(),
                }),
            ),
            busy: fact(rng.chance(15), rng.chance(20)),
            tui_tab_hint: rng.chance(50),
            desktop_tab_hint: rng.chance(50),
            desktop_tabs: fact(rng.chance(15), vec![session.to_owned()]),
            reserved: fact(rng.chance(15), vec![]),
        }),
        Provider::Grok => ProviderEvidence::Grok(GrokEvidence {
            active: fact(
                rng.chance(15),
                (0..rng.below(3))
                    .map(|_| GrokActive {
                        session_id: if rng.chance(50) {
                            session.to_owned()
                        } else {
                            "cccccccc-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_owned()
                        },
                        pid: 1 + rng.below(12) as u32,
                        cwd: rng
                            .chance(70)
                            .then(|| "/Users/example/code/alpha".to_owned()),
                        opened_at: rng.chance(80).then(|| rng.below(4)),
                    })
                    .collect(),
            ),
            session: fact(
                rng.chance(15),
                rng.chance(80).then(|| GrokSession {
                    cwd: "/Users/example/code/alpha".to_owned(),
                    subagent: rng.chance(10),
                }),
            ),
        }),
    }
}

pub(super) fn snapshot(rng: &mut Rng) -> Snapshot {
    let provider = [Provider::Claude, Provider::Codex, Provider::OpenCode][rng.below(3) as usize];
    let session = SessionRef::new(provider, id(rng, provider));
    let versions = ["5.1231.2", "26.1103.60933", "2.2.6", "1.4.0", "2.15", "9.9"];
    Snapshot {
        observed_at: 0,
        boot: fact(rng.chance(15), BootId(rng.below(2))),
        processes: fact(rng.chance(15), table(rng, &session.id)),
        front: Front {
            app: Fact::Known(rng.chance(50).then(|| App::ALL[rng.below(5) as usize])),
            pane_tty: fact(rng.chance(15), None),
            view: fact(rng.chance(15), None),
            pane: session_navigation::evidence::Fact::Known(None),
            window: session_navigation::evidence::Fact::Known(None),
        },
        apps: Fact::Known(
            App::ALL
                .into_iter()
                .map(|app| AppInfo {
                    app,
                    installed: rng.chance(85),
                    version: Some(versions[rng.below(6) as usize].to_owned()),
                    running: rng.chance(50),
                })
                .collect(),
        ),
        tools: Tool::ALL
            .into_iter()
            .map(|tool| ToolInfo {
                tool,
                version: fact(
                    rng.chance(15),
                    Some(["2.1.321", "0.167.0", "2.2.6", "9"][rng.below(4) as usize].to_owned()),
                ),
                path: rng
                    .chance(80)
                    .then(|| format!("/usr/local/bin/{}", tool.program())),
            })
            .collect(),
        schemes: Fact::Known(
            ["claude", "codex"]
                .into_iter()
                .map(|s| SchemeHandler {
                    scheme: s.into(),
                    bundle: Some(
                        if s == "claude" {
                            App::ClaudeDesktop
                        } else {
                            App::CodexDesktop
                        }
                        .bundle_id()
                        .into(),
                    ),
                })
                .collect(),
        ),
        platform: session_navigation::model::Platform::MacOs,
        runtime: session_navigation::evidence::Fact::Known(
            session_navigation::model::Runtime::Native,
        ),
        surfaces: session_navigation::hosts::Surfaces::mac_scripted(),
        original_host: session_navigation::evidence::Fact::Unknown,
        terminal_profile: session_navigation::evidence::Fact::Known(None),
        default_terminal: fact(
            rng.chance(15),
            [
                None,
                Some(TerminalKind::Ghostty),
                Some(TerminalKind::AppleTerminal),
            ][rng.below(3) as usize],
        ),
        last_seen_host: fact(rng.chance(15), Host::Terminal(TerminalKind::Ghostty)),
        reservation: fact(
            rng.chance(15),
            rng.chance(20).then(|| Reservation {
                session: session.clone(),
                made_at: 0,
                tty: None,
                seen: true,
                boot: None,
                via: [Pending::Launch, Pending::Link][rng.below(2) as usize],
            }),
        ),
        cwd: fact(rng.chance(15), "/Users/example/code/alpha".to_owned()),
        cwd_exists: fact(rng.chance(15), rng.chance(80)),
        environment: fact(rng.chance(15), Default::default()),
        capabilities: if rng.chance(30) {
            vec![Capability::ClaudeRegistryFollowsSession {
                cli_version: "2.1.321".into(),
            }]
        } else {
            vec![]
        },
        provider: provider_evidence(rng, provider, &session.id),
        session,
    }
}

pub(super) fn request(rng: &mut Rng) -> OpenRequest {
    let mut request = OpenRequest {
        prefs: Preferences {
            default_terminal: rng.chance(60).then_some(TerminalKind::Ghostty),
            allow_import: rng.chance(30),
            prefer_terminal: false,
        },
        take_over: rng.chance(30),
        use_default_terminal: rng.chance(20),
        confirm_launch_gone: rng.chance(10),
        selected: rng
            .chance(10)
            .then_some(Target::Desktop(App::ClaudeDesktop)),
        confirm_not_open_elsewhere: rng.chance(20),
    };
    // From a stream of its own (seeded by a peek at the next draw), so no
    // other draw moves.
    request.prefs.prefer_terminal = Rng::new(rng.clone().next_u64() ^ 0x7E2A_1AA1).chance(30);
    request
}

#[test]
fn decide_is_total_and_well_formed_on_random_snapshots() {
    let mut rng = Rng::new(2026);
    let mut acted = 0;
    for _ in 0..200_000 {
        let snap = snapshot(&mut rng);
        let req = request(&mut rng);
        let d = decide(&snap, &req);
        match &d.plan {
            Plan::Act(a) => {
                acted += 1;
                assert!(!a.steps.is_empty());
                if a.kind == ActionKind::BringForward {
                    assert_eq!(a.precision, Precision::AppOnly);
                }
                if a.starts_process() {
                    assert!(snap.processes.is_known() && snap.boot.is_known(), "{d:?}");
                    assert!(
                        matches!(snap.reservation, Fact::Known(None)) || req.confirm_launch_gone,
                        "{d:?}"
                    );
                }
                for step in &a.steps {
                    match step {
                        Step::Launch { command, .. } => {
                            assert_eq!(snap.cwd_exists, Fact::Known(true), "{d:?}");
                            assert_eq!(command.args.len(), 2);
                            assert_eq!(command.args[1], snap.session.id);
                        }
                        Step::Stop { .. } => assert!(req.take_over),
                        Step::OpenUrl { url, .. } => {
                            assert!(!url.contains("prompt") && !url.contains('#'))
                        }
                        _ => {}
                    }
                }
            }
            Plan::Choose(list) => {
                assert!(list.len() > 1 && list.iter().all(|a| !a.starts_process()))
            }
            Plan::Confirm(c) => {
                assert!(!req.confirm_not_open_elsewhere, "{d:?}");
                assert!(!c.then.steps.is_empty() && !c.elsewhere.is_empty(), "{d:?}");
            }
            Plan::Unavailable(_) => {}
        }
    }
    assert!(
        acted > 1000,
        "the generator must reach real plans too ({acted})"
    );
}
