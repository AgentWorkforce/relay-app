//! The Windows adapter's parsers against PowerShell 5.1 and `wsl.exe`
//! output shapes (fixtures in `tests/fixtures/windows/`).
#![cfg(feature = "windows")]
mod windows_support;

use session_navigation::evidence::{Exe, Fact, Front};
use session_navigation::hosts::WindowId;
use session_navigation::model::{App, BootId, Tool};
use session_navigation_native::windows::classify::{classify_posix, classify_windows};
use session_navigation_native::windows::powershell::{base64, decode_text, encode_script};
use session_navigation_native::windows::report::{self, ms_date, Report};
use session_navigation_native::windows::wsl;
use windows_support::*;

fn parse(name: &str) -> Report {
    let text = decode_text(&fixture(name)).expect("decodable");
    serde_json::from_str(text.trim()).expect("a report")
}

#[test]
fn dates_in_every_powershell_form() {
    assert_eq!(ms_date("/Date(1794291866500)/"), Some(1_794_291_866));
    assert_eq!(ms_date("/Date(1794300100700+0100)/"), Some(1_794_300_100));
    assert_eq!(ms_date("/Date(-5)/"), None);
    assert_eq!(ms_date("Date(1)"), None);
    assert_eq!(ms_date("/Date()/"), None);
}

#[test]
fn the_full_report_becomes_a_table_without_ttys() {
    let report = parse("observe-report.json");
    assert_eq!(report.boot(), Some(BootId(4242)));
    assert_eq!(report.boot_time(), Some(1_794_291_866));
    let table = report.table().expect("table");
    // The idle pseudo-process (no creation time) is skipped; the rest stay,
    // including the unreadable antivirus process (unreadable ≠ exited).
    assert_eq!(table.len(), 10);
    let rows = report::by_pid(&table);
    assert!(table.iter().all(|p| p.tty.is_none() && p.window.is_none()));
    assert!(table.iter().all(|p| p.cwd == Fact::Unknown));
    let exe = |pid: u32| rows.get(&pid).map(|p| p.exe);
    assert_eq!(exe(3000), Some(Exe::Shell));
    assert_eq!(exe(3001), Some(Exe::ConsoleHost));
    assert_eq!(exe(3002), Some(Exe::ClaudeCli));
    assert_eq!(exe(4000), Some(Exe::WindowsTerminal));
    // Windows Terminal's pseudo-console host is not a classic console.
    assert_eq!(exe(4001), Some(Exe::Other));
    assert_eq!(exe(4100), Some(Exe::ClaudeCli), "node running cli.js");
    assert_eq!(exe(5000), Some(Exe::Other));
    let conhost = rows.get(&3001).expect("conhost");
    assert_eq!(
        conhost.identity.start, 1_794_300_000,
        "the {{value}} date form"
    );
    assert_eq!(
        rows.get(&4002).map(|p| p.identity.start),
        Some(1_794_300_100)
    );
    let claude = rows.get(&3002).expect("claude");
    assert_eq!(
        claude.argv,
        vec![
            r"C:\Users\dev\.local\bin\claude.exe".to_owned(),
            "--resume".into(),
            CLAUDE_ID.into()
        ]
    );
    assert!(rows.get(&5000).is_some_and(|p| p.argv.is_empty()));
}

#[test]
fn windows_and_front_need_exact_owner_identities() {
    let report = parse("observe-report.json");
    let table = report.table().expect("table");
    let raw = report.windows.clone().expect("windows").into_vec();
    let windows = report::windows(&table, raw);
    assert_eq!(windows.len(), 3);
    let (app, window) = report::front(Some(&table), report.foreground.as_ref());
    assert_eq!(app, Fact::Known(Some(App::WindowsTerminal)));
    assert_eq!(window, Fact::Known(Some(WindowId(262_404))));
    // The console window is owned by its first client (cmd.exe), which
    // reads as the console host.
    let cmd = table.iter().find(|p| p.identity.pid == 3000).expect("cmd");
    assert_eq!(
        report::app_of(&table, &cmd.identity),
        Some(App::ConsoleHost)
    );
    // A window whose owner pid now belongs to a newer process is nobody's.
    let mut stale = report.windows.clone().expect("windows").into_vec();
    for w in &mut stale {
        w.created += 7;
    }
    assert!(report::windows(&table, stale).is_empty());
    let mut fg = report.foreground.expect("foreground");
    fg.created = -1;
    assert_eq!(report::front(Some(&table), Some(&fg)).0, Fact::Unknown);
}

#[test]
fn single_elements_bom_and_failed_sections() {
    let report = parse("observe-single-bom.json");
    // No boot counter: LastBootUpTime stands in.
    assert_eq!(report.boot(), Some(BootId(1_794_291_866)));
    let table = report.table().expect("one-row table");
    assert_eq!(table.len(), 1);
    let windows = report::windows(&table, report.windows.clone().expect("one").into_vec());
    assert_eq!(windows.len(), 1);
    assert_eq!(
        report::front(Some(&table), report.foreground.as_ref()),
        (Fact::Known(None), Fact::Known(None)),
        "no foreground window (a locked screen)"
    );
    assert_eq!(report.terminal.map(|t| t.installed), Some(false));

    let failed = parse("observe-sections-failed.json");
    assert_eq!(failed.table(), None);
    assert!(failed.windows.is_none());
    let front: Front = Front {
        app: report::front(None, failed.foreground.as_ref()).0,
        pane_tty: Fact::Known(None),
        pane: Fact::Known(None),
        window: Fact::Unknown,
        view: Fact::Known(None),
    };
    assert_eq!(front.app, Fact::Unknown);
}

#[test]
fn a_duplicated_pid_fails_the_whole_table() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&fixture("observe-report.json")).expect("json");
    let first = value["processes"][2].clone();
    value["processes"]
        .as_array_mut()
        .expect("array")
        .push(first);
    let report: Report = serde_json::from_value(value).expect("report");
    assert_eq!(report.table(), None);
}

#[test]
fn utf16_output_is_decoded() {
    let names = wsl::parse_list(&fixture("wsl-list-q.utf16le")).expect("list");
    assert_eq!(names, vec!["Ubuntu-22.04", "docker-desktop"]);
    let mut bom = vec![0xFF, 0xFE];
    bom.extend("{\"a\":1}".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(decode_text(&bom).as_deref(), Some("{\"a\":1}"));
    assert_eq!(decode_text(b"plain").as_deref(), Some("plain"));
    assert_eq!(decode_text(&[0xFF, 0xFE, 0x41]), None);
}

#[test]
fn encoded_command_is_base64_utf16le() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    // `powershell -EncodedCommand ZABpAHIA` runs `dir`.
    assert_eq!(encode_script("dir"), "ZABpAHIA");
}

#[test]
fn the_wsl_table_from_proc() {
    let table = wsl::parse_table(&fixture("wsl-table.txt")).expect("table");
    assert_eq!(table.uid, 1000);
    assert_eq!(table.boot_time, 1_794_400_000);
    assert_eq!(table.boot, BootId(0x3f1b_9c2e_8d4a_4b6f));
    let row = |pid: u32| table.processes.iter().find(|p| p.identity.pid == pid);
    let claude = row(311).expect("claude");
    assert_eq!(
        claude.exe,
        Exe::ClaudeCli,
        "native installer's versions/ path"
    );
    assert_eq!(claude.identity.start, 1_794_400_000 + 125);
    assert_eq!(claude.tty.as_deref(), Some("/dev/pts/0"));
    assert_eq!(claude.cwd, Fact::Known("/home/dev/code/project".to_owned()));
    assert_eq!(claude.argv.get(1).map(String::as_str), Some("--resume"));
    assert_eq!(row(312).map(|p| p.exe), Some(Exe::ClaudeCli), "node cli.js");
    assert_eq!(
        row(312).and_then(|p| p.tty.clone()).as_deref(),
        Some("/dev/pts/1")
    );
    assert_eq!(row(310).map(|p| p.exe), Some(Exe::Shell));
    assert_eq!(row(310).map(|p| p.cwd.clone()), Some(Fact::Unknown));
    // An unreadable exe link falls back to the task name.
    assert_eq!(row(2).map(|p| p.exe), Some(Exe::Other));
    // A `)` inside the task name does not shift the fields.
    assert_eq!(row(313).map(|p| p.ppid), Some(1));
    assert_eq!(row(313).and_then(|p| p.tty.clone()), None);
}

#[test]
fn a_malformed_wsl_line_fails_the_table() {
    let mut text = String::from_utf8(fixture("wsl-table.txt")).expect("utf8");
    text.push_str("proc 999 1000 1000 zz - - -\n");
    assert_eq!(wsl::parse_table(text.as_bytes()), None);
    assert_eq!(wsl::parse_table(b"boot x\n"), None);
}

#[test]
fn wsl_tools_are_posix_paths() {
    let tools = wsl::parse_tools(&fixture("wsl-tools.txt")).expect("tools");
    let tool = |t: Tool| tools.iter().find(|i| i.tool == t).expect("tool");
    assert_eq!(
        tool(Tool::ClaudeCli).path.as_deref(),
        Some("/home/dev/.local/bin/claude")
    );
    assert_eq!(
        tool(Tool::ClaudeCli).version,
        Fact::Known(Some("2.1.321".into()))
    );
    assert_eq!(tool(Tool::CodexCli).version, Fact::Known(None));
    assert_eq!(tool(Tool::OpenCodeCli).version, Fact::Unknown);
    assert_eq!(
        tool(Tool::GrokCli).version,
        Fact::Known(Some("0.1.23".into()))
    );
    assert_eq!(tool(Tool::Tmux).version, Fact::Known(Some("3.4".into())));
    assert!(wsl::parse_tools(b"tool claude 2f\n").is_none());
}

#[test]
fn tty_numbers() {
    assert_eq!(wsl::tty_of(0), None);
    assert_eq!(wsl::tty_of(136 << 8 | 3).as_deref(), Some("/dev/pts/3"));
    assert_eq!(wsl::tty_of(4 << 8 | 1).as_deref(), Some("/dev/tty1"));
}

#[test]
fn classification() {
    let none: Vec<String> = Vec::new();
    let c = |p: &str| classify_windows(p, &none);
    assert_eq!(
        c(r"C:\Users\dev\AppData\Local\AnthropicClaude\app-1.0.0\claude.exe"),
        Exe::ClaudeDesktopApp
    );
    assert_eq!(
        c(r"C:\Users\dev\AppData\Roaming\Claude\claude-code\2.1.321\claude.exe"),
        Exe::ClaudeCli
    );
    assert_eq!(
        c(
            r"C:\Users\dev\AppData\Roaming\npm\node_modules\@openai\codex\vendor\x86_64-pc-windows-msvc\codex\codex.exe"
        ),
        Exe::CodexCli
    );
    assert_eq!(
        c(r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0.0.0_x64__abc\app\codex.exe"),
        Exe::CodexDesktopApp
    );
    assert_eq!(
        c(r"C:\Users\dev\AppData\Local\OpenCode\OpenCode.exe"),
        Exe::OpenCodeDesktopApp
    );
    assert_eq!(
        c(r"C:\Users\dev\.opencode\bin\opencode.exe"),
        Exe::OpenCodeCli
    );
    assert_eq!(
        c(r"C:\Users\dev\.grok\bin\grok-0.1.23-windows-x86_64.exe"),
        Exe::GrokCli
    );
    assert_eq!(
        c("conhost.exe"),
        Exe::ConsoleHost,
        "image name when the path is unreadable"
    );
    assert_eq!(c(r"C:\Program Files\WezTerm\wezterm-gui.exe"), Exe::WezTerm);
    assert_eq!(
        c(r"C:\Program Files\Alacritty\alacritty.exe"),
        Exe::Alacritty
    );
    assert_eq!(c(r"C:\Program Files\nodejs\node.exe"), Exe::Other);
    let argv = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        classify_windows(
            r"C:\Windows\System32\conhost.exe",
            &argv(&["conhost.exe", "--headless"])
        ),
        Exe::Other
    );
    assert_eq!(classify_posix("/usr/bin/tmux", &none), Exe::Tmux);
    assert_eq!(classify_posix("/usr/bin/zsh", &none), Exe::Shell);
    assert_eq!(
        classify_posix("/usr/bin/node", &argv(&["node", "/x/codex.js"])),
        Exe::Other
    );
}
