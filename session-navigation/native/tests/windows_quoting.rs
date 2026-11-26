//! Windows command-line quoting (MSVC CRT / `CommandLineToArgvW` rules),
//! the `cmd.exe` and Windows Terminal hazards, and launch validation.
#![cfg(feature = "windows")]
mod windows_support;

use std::collections::BTreeMap;

use session_navigation::model::{Runtime, TerminalKind};
use session_navigation::plan::{Command, LaunchContext};
use session_navigation_native::system::FakeSystem;
use session_navigation_native::windows::launch::{self, LaunchError};
use session_navigation_native::windows::quote::{self, QuoteError};
use windows_support::*;

const HARD: &[&str] = &[
    "",
    "plain",
    "two words",
    "tab\there",
    "quote\"inside",
    "\"",
    "\"\"",
    "back\\slash",
    "trailing\\",
    "trailing space \\",
    "\\\\server\\share\\",
    "\\\"",
    "a\\\\\"b",
    "C:\\Program Files\\x\\",
    "ünïcödé ✓",
    "semi;colon & amp | pipe",
    "%PATH% !x! ^caret",
    "new\nline",
    "-flag=value with space",
];

#[test]
fn every_argument_round_trips() {
    let program = r"C:\Program Files\Tool\tool.exe".to_owned();
    for a in HARD {
        for b in HARD {
            let args = vec![(*a).to_owned(), (*b).to_owned()];
            let line = quote::command_line(&program, &args).expect("quotable");
            let mut want = vec![program.clone()];
            want.extend(args);
            assert_eq!(quote::split(&line), want, "line {line:?}");
        }
    }
}

#[test]
fn quoting_is_minimal_and_refuses_nul() {
    assert_eq!(quote::quote_arg("--resume").as_deref(), Ok("--resume"));
    assert_eq!(quote::quote_arg("").as_deref(), Ok("\"\""));
    assert_eq!(quote::quote_arg("a b\\").as_deref(), Ok("\"a b\\\\\""));
    assert_eq!(quote::quote_arg("a\"b").as_deref(), Ok("\"a\\\"b\""));
    assert_eq!(quote::quote_arg("nul\0"), Err(QuoteError::Unquotable));
    assert_eq!(quote::quote_program("C:\\a\"b"), Err(QuoteError::Program));
    let long = vec!["x".repeat(33_000)];
    assert_eq!(quote::join_args(&long), Err(QuoteError::TooLong));
}

#[test]
fn the_reference_splitter_matches_documented_examples() {
    // Microsoft's "Parsing C++ command-line arguments" table.
    let s = |l: &str| quote::split(&format!("prog {l}"))[1..].to_vec();
    assert_eq!(s(r#""a b c" d e"#), ["a b c", "d", "e"]);
    assert_eq!(s(r#""ab\"c" "\\" d"#), ["ab\"c", "\\", "d"]);
    assert_eq!(s(r#"a\\\b d"e f"g h"#), ["a\\\\\\b", "de fg", "h"]);
    assert_eq!(s(r#"a\\\"b c d"#), ["a\\\"b", "c", "d"]);
    assert_eq!(s(r#"a\\\\"b c" d e"#), ["a\\\\b c", "d", "e"]);
    assert_eq!(s(r#"a"b"" c d"#), ["ab\" c d"]);
    // The program word: no escapes, a quote ends it.
    assert_eq!(quote::split(r#""C:\a b\x.exe" y"#), [r"C:\a b\x.exe", "y"]);
    assert!(quote::split("   ").is_empty());
}

#[test]
fn cmd_and_terminal_hazards() {
    for safe in ["--resume", CLAUDE_ID, r"C:\Users\dev\x y", "a=b", "{x}"] {
        assert!(quote::cmd_safe(safe), "{safe}");
    }
    for bad in [
        "%PATH%", "!x!", "a&b", "a|b", "a^b", "(a)", "<a>", "a\"b", "a;b",
    ] {
        assert!(!quote::cmd_safe(bad), "{bad}");
    }
    for safe in ["new-tab", r"C:\a b\c", r"C:\", "x\\"] {
        assert!(quote::wt_safe(safe), "{safe}");
    }
    for bad in ["", "a;b", "a\"b", "%x%", r"C:\a b\", "a\nb"] {
        assert!(!quote::wt_safe(bad), "{bad:?}");
    }
    assert!(quote::is_batch(r"C:\npm\claude.CMD"));
    assert!(!quote::is_batch(r"C:\x\claude.exe"));
}

fn machine() -> FakeSystem {
    let fake = FakeSystem::new();
    fake.add_file(CLAUDE_EXE, b"MZ".to_vec(), 1);
    fake.add_file(
        r"C:\Users\dev\AppData\Roaming\npm\claude.cmd",
        b"@echo".to_vec(),
        1,
    );
    fake.add_dir(PROJECT);
    fake.add_dir(r"\\wsl.localhost\Ubuntu\home\dev\code\project");
    fake
}

fn command(program: &str, args: &[&str], cwd: &str) -> Command {
    Command {
        program: program.to_owned(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: cwd.to_owned(),
        env: BTreeMap::new(),
    }
}

fn native(profile: Option<&str>) -> LaunchContext {
    LaunchContext {
        runtime: Runtime::Native,
        profile: profile.map(str::to_owned),
    }
}

#[test]
fn console_and_terminal_launch_specs() {
    let fake = machine();
    let cfg = cfg();
    let cmd = command(CLAUDE_EXE, &["--resume", CLAUDE_ID], PROJECT);
    let console =
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &cmd, &native(None)).expect("console");
    assert_eq!(console.file, CLAUDE_EXE);
    assert_eq!(console.line, format!("--resume {CLAUDE_ID}"));
    assert_eq!(console.cwd, PROJECT);
    let wt = launch::spec(
        &fake,
        &cfg,
        TerminalKind::WindowsTerminal,
        &cmd,
        &native(Some(PROFILE_GUID)),
    )
    .expect("wt");
    assert_eq!(wt.file, cfg.wt);
    assert_eq!(
        quote::split(&format!("wt {}", wt.line))[1..],
        [
            "-w",
            "0",
            "new-tab",
            "--profile",
            PROFILE_GUID,
            "--startingDirectory",
            PROJECT,
            "--",
            CLAUDE_EXE,
            "--resume",
            CLAUDE_ID
        ]
    );
    // A profile is only ever a GUID, and only for Windows Terminal.
    for bad in ["Ubuntu; calc", "{not-a-guid}", ""] {
        let r = launch::spec(
            &fake,
            &cfg,
            TerminalKind::WindowsTerminal,
            &cmd,
            &native(Some(bad)),
        );
        assert_eq!(r, Err(LaunchError::Profile), "{bad}");
    }
    let r = launch::spec(
        &fake,
        &cfg,
        TerminalKind::ConsoleHost,
        &cmd,
        &native(Some(PROFILE_GUID)),
    );
    assert_eq!(r, Err(LaunchError::Profile));
}

#[test]
fn launch_refusals() {
    let fake = machine();
    let cfg = cfg();
    let spec = |cmd: &Command, t: TerminalKind| launch::spec(&fake, &cfg, t, cmd, &native(None));
    let console = TerminalKind::ConsoleHost;
    assert_eq!(
        spec(&command("claude.exe", &[], PROJECT), console),
        Err(LaunchError::Program)
    );
    assert_eq!(
        spec(
            &command(r"C:\Windows\System32\calc.exe", &[], PROJECT),
            console
        ),
        Err(LaunchError::UnknownProgram)
    );
    assert_eq!(
        spec(&command(r"C:\nope\claude.exe", &[], PROJECT), console),
        Err(LaunchError::Program)
    );
    assert_eq!(
        spec(&command(CLAUDE_EXE, &[], r"C:\missing"), console),
        Err(LaunchError::Cwd)
    );
    assert_eq!(
        spec(&command(CLAUDE_EXE, &["a\nb"], PROJECT), console),
        Err(LaunchError::Quote(QuoteError::Unquotable))
    );
    let shim = r"C:\Users\dev\AppData\Roaming\npm\claude.cmd";
    assert!(spec(&command(shim, &["--resume", CLAUDE_ID], PROJECT), console).is_ok());
    assert_eq!(
        spec(&command(shim, &["%USERPROFILE%"], PROJECT), console),
        Err(LaunchError::Quote(QuoteError::CmdUnsafe))
    );
    assert_eq!(
        spec(
            &command(CLAUDE_EXE, &["a;b"], PROJECT),
            TerminalKind::WindowsTerminal
        ),
        Err(LaunchError::Quote(QuoteError::WtUnsafe))
    );
    let mut env = command(CLAUDE_EXE, &[], PROJECT);
    env.env.insert("PATH".into(), "x".into());
    assert_eq!(spec(&env, console), Err(LaunchError::Environment));
    assert_eq!(
        spec(&command(CLAUDE_EXE, &[], PROJECT), TerminalKind::Ghostty),
        Err(LaunchError::Terminal)
    );
    let wsl = LaunchContext {
        runtime: Runtime::Wsl {
            distro: DISTRO.into(),
        },
        profile: None,
    };
    let r = launch::spec(
        &fake,
        &cfg,
        console,
        &command(CLAUDE_EXE, &[], PROJECT),
        &wsl,
    );
    assert_eq!(
        r,
        Err(LaunchError::Runtime),
        "a native config never runs WSL"
    );
}

#[test]
fn provider_homes_reach_a_terminal_tab_through_the_wrapper() {
    let fake = machine();
    let cfg = cfg();
    let mut cmd = command(CLAUDE_EXE, &["--resume", CLAUDE_ID], PROJECT);
    cmd.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        r"C:\Users\dev\.claude-work".into(),
    );
    let spec = launch::spec(
        &fake,
        &cfg,
        TerminalKind::WindowsTerminal,
        &cmd,
        &native(None),
    )
    .expect("wt");
    let words = quote::split(&format!("wt {}", spec.line));
    let tail: Vec<&str> = words.iter().skip(7).map(String::as_str).collect();
    assert_eq!(tail.first().copied(), Some(cfg.powershell().as_str()));
    assert!(tail.contains(&"-EncodedCommand"));
    // Nothing but Base64 follows: no path or value is in the script text.
    let encoded = tail.last().expect("encoded");
    assert!(encoded
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)));
    assert!(spec.env.is_empty());
    // On a console the launcher's own environment carries it.
    let console =
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &cmd, &native(None)).expect("console");
    assert_eq!(console.env, cmd.env);
}

#[test]
fn wsl_launches_run_inside_the_distribution() {
    let fake = machine();
    let cfg = wsl_cfg();
    let ctx = |profile: Option<&str>| LaunchContext {
        runtime: Runtime::Wsl {
            distro: DISTRO.into(),
        },
        profile: profile.map(str::to_owned),
    };
    let mut cmd = command(
        "/home/dev/.local/bin/claude",
        &["--resume", CLAUDE_ID],
        "/home/dev/code/project",
    );
    cmd.env
        .insert("CLAUDE_CONFIG_DIR".into(), "/home/dev/.claude-2".into());
    let console =
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &cmd, &ctx(None)).expect("console");
    assert_eq!(console.file, cfg.wsl_exe());
    assert_eq!(
        quote::split(&format!("wsl {}", console.line))[1..],
        [
            "-d",
            DISTRO,
            "--cd",
            "/home/dev/code/project",
            "--exec",
            "/usr/bin/env",
            "CLAUDE_CONFIG_DIR=/home/dev/.claude-2",
            "/home/dev/.local/bin/claude",
            "--resume",
            CLAUDE_ID
        ]
    );
    assert_eq!(console.cwd, PROFILE);
    let wt = launch::spec(
        &fake,
        &cfg,
        TerminalKind::WindowsTerminal,
        &cmd,
        &ctx(Some(PROFILE_GUID)),
    )
    .expect("wt");
    let words = quote::split(&format!("wt {}", wt.line));
    assert!(!words.iter().any(|w| w == "--startingDirectory"));
    assert_eq!(
        words.get(7).map(String::as_str),
        Some(cfg.wsl_exe().as_str())
    );
    // Another distribution, a Windows path or a missing directory: refused.
    let other = LaunchContext {
        runtime: Runtime::Wsl {
            distro: "Debian".into(),
        },
        profile: None,
    };
    assert_eq!(
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &cmd, &other),
        Err(LaunchError::Runtime)
    );
    let win = command(CLAUDE_EXE, &[], PROJECT);
    assert_eq!(
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &win, &ctx(None)),
        Err(LaunchError::Program)
    );
    let gone = command("/home/dev/.local/bin/claude", &[], "/home/dev/gone");
    assert_eq!(
        launch::spec(&fake, &cfg, TerminalKind::ConsoleHost, &gone, &ctx(None)),
        Err(LaunchError::Cwd)
    );
}
