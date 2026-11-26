//! Parsers for every command output: real (sanitized) samples, malformed,
//! truncated and hostile inputs, and a seeded mutation fuzz loop.
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::{Entrypoint, Exe, Fact};
use session_navigation::model::BootId;
use session_navigation_native::common::opencode::is_loopback_url;
use session_navigation_native::common::parse;
use session_navigation_native::macos::parse::{self as mparse, ps};

fn table() -> Vec<session_navigation::evidence::ProcessInfo> {
    ps::join_table(
        PS_IDENTITY.as_bytes(),
        PS_COMM.as_bytes(),
        PS_ARGS.as_bytes(),
        BootId(BOOT),
    )
    .unwrap_or_default()
}

#[test]
fn ps_fixture_joins_and_classifies() {
    let t = table();
    assert_eq!(t.len(), 13);
    let exe = |pid: u32| t.iter().find(|p| p.identity.pid == pid).map(|p| p.exe);
    assert_eq!(exe(811), Some(Exe::Ghostty));
    assert_eq!(exe(815), Some(Exe::OpenCodeDesktopApp));
    assert_eq!(exe(819), Some(Exe::CodexDesktopApp));
    assert_eq!(exe(995), Some(Exe::Login));
    assert_eq!(exe(998), Some(Exe::Shell));
    assert_eq!(exe(1111), Some(Exe::OpenCodeCli));
    assert_eq!(exe(1447), Some(Exe::CodexCli));
    assert_eq!(exe(1569), Some(Exe::Other));
    assert_eq!(exe(25208), Some(Exe::ClaudeDesktopApp));
    assert_eq!(exe(25353), Some(Exe::Other));
    assert_eq!(exe(25354), Some(Exe::ClaudeCli));
    let login = t
        .iter()
        .find(|p| p.identity.pid == 995)
        .cloned()
        .unwrap_or_else(|| t[0].clone());
    assert_eq!((login.euid, login.ruid), (0, 501));
    assert_eq!(login.tty.as_deref(), Some("/dev/ttys000"));
    assert_eq!(login.cwd, Fact::Unknown);
    let claude = t
        .iter()
        .find(|p| p.identity.pid == 25354)
        .cloned()
        .unwrap_or_else(|| t[0].clone());
    // argv[0] keeps its spaces (from comm); the rest is whitespace-split.
    assert!(claude.argv[0].contains("Application Support"));
    assert_eq!(claude.argv[1], "--output-format");
    assert_eq!(claude.tty, None);
    assert_eq!(
        claude.identity.start,
        session_navigation::parse::lstart::parse("Wed Nov 11 03:45:11 2026").unwrap_or(0)
    );
}

#[test]
fn ps_rows_that_changed_between_calls_are_dropped() {
    let comm = PS_COMM.replace("25354 Wed Nov 11 03:45:11", "25354 Wed Nov 11 03:45:12");
    let t = ps::join_table(
        PS_IDENTITY.as_bytes(),
        comm.as_bytes(),
        PS_ARGS.as_bytes(),
        BootId(BOOT),
    )
    .unwrap_or_default();
    assert_eq!(t.len(), 12);
    assert!(!t.iter().any(|p| p.identity.pid == 25354));
    let args: String = PS_ARGS
        .lines()
        .filter(|l| !l.starts_with("  998"))
        .map(|l| format!("{l}\n"))
        .collect();
    let t = ps::join_table(
        PS_IDENTITY.as_bytes(),
        PS_COMM.as_bytes(),
        args.as_bytes(),
        BootId(BOOT),
    )
    .unwrap_or_default();
    assert!(!t.iter().any(|p| p.identity.pid == 998));
}

#[test]
fn ps_any_bad_row_fails_the_whole_table() {
    let bad = [
        PS_IDENTITY.replace("ttys000 ", "TTY!000 "),
        PS_IDENTITY.replace("  995   811", "  995  -811"),
        PS_IDENTITY.replace("Tue Nov 10 07:11:14", "Mon Nov 10 07:11:14"),
        format!("{PS_IDENTITY}garbage\n"),
        PS_IDENTITY.replace("    login", ""),
        format!(
            "{PS_IDENTITY}  811     1   501   501 ??       Tue Nov 10 07:11:06 2026     ghostty\n"
        ),
        String::new(),
    ];
    for identity in bad {
        assert!(
            ps::join_table(
                identity.as_bytes(),
                PS_COMM.as_bytes(),
                PS_ARGS.as_bytes(),
                BootId(BOOT)
            )
            .is_none(),
            "{identity}"
        );
    }
    assert!(ps::join_table(
        PS_IDENTITY.as_bytes(),
        b"\xff\xfe",
        PS_ARGS.as_bytes(),
        BootId(BOOT)
    )
    .is_none());
}

#[test]
fn classification_needs_kernel_name_and_bundle_path() {
    // argv[0] alone never makes an app: a renamed process stays Other.
    assert_eq!(
        ps::classify("sleep", "/Applications/Ghostty.app/Contents/MacOS/ghostty"),
        Exe::Other
    );
    assert_eq!(ps::classify("ghostty", "/tmp/ghostty"), Exe::Other);
    assert_eq!(
        ps::classify(
            "Terminal",
            "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"
        ),
        Exe::AppleTerminal
    );
    assert_eq!(
        ps::classify("Codex", "/Applications/Codex.app/Contents/MacOS/Codex"),
        Exe::CodexDesktopApp
    );
    // A CLI is known by its kernel name, whatever argv[0] says (`exec -l`).
    assert_eq!(
        ps::classify("codex", "-/opt/homebrew/bin/codex"),
        Exe::CodexCli
    );
    assert_eq!(ps::classify("codex", "codex"), Exe::CodexCli);
    assert_eq!(ps::classify("opencode", "opencode"), Exe::OpenCodeCli);
    assert_eq!(ps::classify("claude", "claude"), Exe::ClaudeCli);
    assert_eq!(
        ps::classify("codex-code-mode-", "/x/codex-code-mode-host"),
        Exe::Other
    );
    assert_eq!(ps::classify("fish", "-fish"), Exe::Shell);
}

#[test]
fn argv_and_tty_shapes() {
    assert_eq!(ps::argv("-/bin/zsh", "-/bin/zsh"), vec!["-/bin/zsh"]);
    assert_eq!(
        ps::argv("/a b/c", "/a b/c resume x"),
        vec!["/a b/c", "resume", "x"]
    );
    assert_eq!(ps::argv("codex", "codexy resume"), vec!["codexy", "resume"]);
    assert_eq!(ps::tty("??"), Some(None));
    assert_eq!(ps::tty("ttys012"), Some(Some("/dev/ttys012".into())));
    assert_eq!(ps::tty("../etc"), None);
}

#[test]
fn sysctl_boot_time() {
    assert_eq!(mparse::boot_time(SYSCTL.as_bytes()), Some(BOOT));
    for bad in [
        "",
        "{ sec = , usec = 1 }",
        "{ sec = 12a, usec = 1 }",
        "sec = 1, usec = 2",
        "{ sec = 99999999999999999999, usec = 0 }",
        "{ sec = 5 }",
    ] {
        assert_eq!(mparse::boot_time(bad.as_bytes()), None, "{bad}");
    }
}

#[test]
fn tool_versions() {
    assert_eq!(
        parse::tool_version(b"codex-cli 0.167.0\n").as_deref(),
        Some("0.167.0")
    );
    assert_eq!(
        parse::tool_version(b"2.1.321 (Claude Code)\n").as_deref(),
        Some("2.1.321")
    );
    assert_eq!(parse::tool_version(b"2.2.6\n").as_deref(), Some("2.2.6"));
    assert_eq!(
        parse::tool_version(b"v1.2.3-beta").as_deref(),
        Some("1.2.3-beta")
    );
    for bad in [
        &b""[..],
        b"\n\n",
        b"no version here",
        b"1",
        b"\xff\xff 1.2",
        b"1.2;rm",
    ] {
        assert_eq!(parse::tool_version(bad), None);
    }
    assert_eq!(parse::tool_version(&vec![b'1'; 5000]), None);
}

#[test]
fn osascript_errors() {
    let refused =
        b"14:52: execution error: Not authorized to send Apple events to Ghostty. (-1743)\n";
    assert_eq!(mparse::osascript_error(refused), Some(-1743));
    assert_eq!(
        mparse::osascript_error(b"execution error: pane unavailable (7001)"),
        Some(7001)
    );
    for bad in [
        &b""[..],
        b"(-)",
        b"error (-17x3)",
        b"error (-1743) trailing",
        b"(1234567)",
    ] {
        assert_eq!(mparse::osascript_error(bad), None);
    }
}

#[test]
fn lsof_outputs() {
    let cwds = parse::lsof_cwds(LSOF_CWD.as_bytes()).unwrap_or_default();
    assert_eq!(
        cwds.get(&25354).map(String::as_str),
        Some("/Users/example/code/project")
    );
    assert_eq!(cwds.len(), 3);
    assert_eq!(
        parse::lsof_pids(b"p1447\np79653\n"),
        Some(vec![1447, 79653])
    );
    assert_eq!(parse::lsof_pids(b""), Some(vec![]));
    for bad in [
        &b"x1\n"[..],
        b"n/orphan\n",
        b"p\n",
        b"p12a\n",
        b"p99999999999\n",
    ] {
        assert_eq!(parse::lsof_names(bad), None);
    }
    // A relative or doubled name is not a cwd.
    let odd = parse::lsof_cwds(b"p1\nnrelative\np2\nn/a\nn/b\n").unwrap_or_default();
    assert!(odd.is_empty());
}

#[test]
fn apps_report_real_sample() {
    let bundles = session_navigation_native::macos::machine::app_bundles();
    let report = mparse::apps_report(APPS_JXA.as_bytes(), &bundles);
    let report = report.unwrap_or_else(|| panic!("fixture parses"));
    assert_eq!(
        report.schemes.get("codex").cloned().flatten().as_deref(),
        Some("com.openai.codex")
    );
    assert_eq!(
        report
            .apps
            .get("com.openai.codex")
            .cloned()
            .flatten()
            .map(|a| a.path)
            .as_deref(),
        Some("/Applications/ChatGPT.app")
    );
    assert_eq!(
        report.front.as_deref(),
        Some("com.anthropic.claudefordesktop")
    );
    // An answer for other bundles than asked is refused.
    assert!(mparse::apps_report(APPS_JXA.as_bytes(), &bundles[..2]).is_none());
    assert!(mparse::apps_report(b"{\"schemes\":{}}", &bundles).is_none());
    assert!(mparse::apps_report(b"", &bundles).is_none());
}

#[test]
fn sqlite_and_curl_outputs() {
    assert_eq!(parse::sqlite_rows(b"").map(|r| r.len()), Some(0));
    assert_eq!(
        parse::sqlite_rows(b"[{\"a\":1},\n{\"a\":2}]\n").map(|r| r.len()),
        Some(2)
    );
    assert!(parse::sqlite_rows(b"[1]").is_none());
    assert!(parse::sqlite_rows(b"Error: no such table").is_none());
    assert_eq!(
        parse::curl_response(b"{\"x\":1}\n200"),
        Some((200, &b"{\"x\":1}"[..]))
    );
    assert_eq!(parse::curl_response(b"\n404"), Some((404, &b""[..])));
    for bad in [
        &b"200"[..],
        b"body\n20",
        b"body\n2000",
        b"body\n999",
        b"body\nabc",
    ] {
        assert_eq!(parse::curl_response(bad), None);
    }
}

#[test]
fn service_password_and_tabs() {
    let state = core_fixture("opencode/service-state.json");
    let secret = parse::service_password(&state);
    assert_eq!(
        secret.as_ref().map(|s| s.expose_secret().to_vec()),
        Some(SECRET.as_bytes().to_vec())
    );
    assert_eq!(format!("{secret:?}"), "Some(<redacted>)");
    assert!(parse::service_password(b"{\"password\":\"a\\nb\"}").is_none());
    assert!(parse::service_password(b"{\"password\":7}").is_none());
    let tabs: serde_json::Value =
        serde_json::from_slice(&core_fixture("opencode/desktop-window-tabs-state.json"))
            .unwrap_or_default();
    let value = tabs["tabs"].to_string();
    assert_eq!(
        parse::desktop_tabs(&value),
        Some(vec![OPENCODE_ID.to_owned()])
    );
    let encoded = serde_json::Value::String(value).to_string();
    assert_eq!(
        parse::desktop_tabs(&encoded),
        Some(vec![OPENCODE_ID.to_owned()])
    );
    assert_eq!(
        parse::desktop_tabs("[{\"type\":\"session\",\"sessionId\":\"../x\"}]"),
        None
    );
    assert_eq!(
        parse::desktop_tabs("[{\"type\":\"settings\"}]"),
        Some(vec![])
    );
}

#[test]
fn registry_names_and_transcripts() {
    assert_eq!(parse::registry_file_pid("43210.json"), Some(43210));
    for bad in [
        "43210.abc.key",
        ".json",
        "x.json",
        "99999999999.json",
        "1.json.tmp",
    ] {
        assert_eq!(parse::registry_file_pid(bad), None, "{bad}");
    }
    let line = core_fixture("claude/transcript-first-message-line.json");
    let queue = format!("{{\"type\":\"queue-operation\",\"sessionId\":\"{CLAUDE_ID}\"}}");
    let mut prefix = format!("{queue}\nnot json\n").into_bytes();
    prefix.extend_from_slice(&line);
    let head = parse::transcript_head(&prefix, CLAUDE_ID);
    assert_eq!(
        head.as_ref().and_then(|h| h.entrypoint),
        Some(Entrypoint::Desktop)
    );
    // Only a queue record: still the session, entrypoint unknown.
    let only_queue = parse::transcript_head(queue.as_bytes(), CLAUDE_ID);
    assert_eq!(only_queue.map(|h| h.entrypoint), Some(None));
    // Another session's record does not count.
    assert!(parse::transcript_head(&line, "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee").is_none());
}

#[test]
fn loopback_urls_only() {
    assert!(is_loopback_url("http://127.0.0.1:49374/api/info"));
    assert!(is_loopback_url("http://localhost:49374/api/info"));
    assert!(is_loopback_url("http://[::1]:49374/api/info"));
    for bad in [
        "https://127.0.0.1:49374/api",
        "http://127.0.0.1.evil.com:80/",
        "http://evil.com/",
        "http://127.0.0.1@evil.com:1/",
        "http://127.0.0.1:/",
        "http://127.0.0.1:1 /x",
        "file:///etc/passwd",
        "http://127.0.0.1:1/\"x",
        "http://127.0.0.1:123456/",
    ] {
        assert!(!is_loopback_url(bad), "{bad}");
    }
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

fn mutate(rng: &mut Rng, sample: &[u8]) -> Vec<u8> {
    let mut bytes = sample.to_vec();
    for _ in 0..=rng.below(8) {
        let at = rng.below(bytes.len() + 1);
        match rng.below(5) {
            0 if at < bytes.len() => bytes[at] = u8::try_from(rng.below(256)).unwrap_or(0),
            1 if at < bytes.len() => {
                bytes.remove(at);
            }
            2 => {
                let pool = b"\n \t\0\xff{}[]\"'\\-:/(";
                bytes.insert(at, pool[rng.below(pool.len())]);
            }
            3 => bytes.truncate(at),
            _ => {
                let from = rng.below(bytes.len() + 1);
                let chunk: Vec<u8> = bytes[from.min(bytes.len())..]
                    .iter()
                    .take(rng.below(64))
                    .copied()
                    .collect();
                bytes.splice(at.min(bytes.len())..at.min(bytes.len()), chunk);
            }
        }
    }
    bytes
}

#[test]
fn fuzz_every_parser_over_mutated_real_samples() {
    let bundles = session_navigation_native::macos::machine::app_bundles();
    let samples: Vec<Vec<u8>> = vec![
        PS_IDENTITY.into(),
        PS_COMM.into(),
        PS_ARGS.into(),
        SYSCTL.into(),
        APPS_JXA.into(),
        LSOF_CWD.into(),
        b"codex-cli 0.167.0\n".to_vec(),
        b"x execution error: no (-1743)\n".to_vec(),
        b"{\"data\":{}}\n200".to_vec(),
        core_fixture("opencode/service-state.json"),
        core_fixture("claude/transcript-first-message-line.json"),
        b"[{\"value\":\"[]\"}]".to_vec(),
    ];
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for round in 0..4000 {
        let sample = &samples[round % samples.len()];
        let m = mutate(&mut rng, sample);
        let text = String::from_utf8_lossy(&m);
        let _ = ps::join_table(&m, PS_COMM.as_bytes(), PS_ARGS.as_bytes(), BootId(BOOT));
        let _ = ps::join_table(PS_IDENTITY.as_bytes(), &m, &m, BootId(BOOT));
        let _ = ps::parse_identity(&m);
        let _ = ps::argv(&text, &text);
        let _ = mparse::boot_time(&m);
        let _ = parse::tool_version(&m);
        let _ = mparse::osascript_error(&m);
        let _ = parse::lsof_cwds(&m);
        let _ = mparse::apps_report(&m, &bundles);
        let _ = parse::sqlite_rows(&m);
        let _ = parse::curl_response(&m);
        let _ = parse::service_password(&m);
        let _ = parse::desktop_tabs(&text);
        let _ = parse::registry_file_pid(&text);
        let _ = parse::transcript_head(&m, CLAUDE_ID);
        let _ = session_navigation_native::common::reservations::session_of(&text);
        let _ = is_loopback_url(&text);
    }
}

#[test]
fn ps_prints_the_nobody_uid_as_minus_two() {
    // Seen natively: `ps` prints uid 4294967294 as -2.
    let line = b"  770     1    -2     0 ??       Tue Nov 10 07:11:06 2026     IPConfigurationH\n";
    let rows = session_navigation_native::macos::parse::ps::parse_identity(line);
    let row = rows.as_ref().and_then(|r| r.first());
    assert_eq!(row.map(|r| (r.euid, r.ruid)), Some((4_294_967_294, 0)));
    let zero = b"  770     1    -0     0 ??       Tue Nov 10 07:11:06 2026     x\n";
    assert!(session_navigation_native::macos::parse::ps::parse_identity(zero).is_none());
}
