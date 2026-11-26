//! The Linux adapter's pure parsers against the kernel's and the tools'
//! documented output shapes (`tests/fixtures/linux/`).
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux_common;

use linux_common::*;
use session_navigation::evidence::{Exe, Fact};
use session_navigation::model::DisplayServer;
use session_navigation_native::linux::classify::{classify_comm, classify_exe};
use session_navigation_native::linux::procfs::*;
use session_navigation_native::linux::x11::{owned, parse_number, parse_wmctrl};
use session_navigation_native::linux::LinuxConfig;

#[test]
fn stat_of_a_shell() {
    let stat = parse_stat(&fixture("pid-stat-bash.txt")).unwrap();
    assert_eq!(stat.pid, 2481);
    assert_eq!(stat.comm, "bash");
    assert_eq!(stat.ppid, 2470);
    assert_eq!(stat.tty_nr, 34817);
    assert_eq!(stat.starttime, 51234);
    assert_eq!(tty_path(stat.tty_nr).as_deref(), Some("/dev/pts/1"));
}

#[test]
fn stat_comm_with_spaces_and_parentheses_is_cut_at_the_last_paren() {
    let stat = parse_stat(&fixture("pid-stat-tricky.txt")).unwrap();
    assert_eq!(stat.pid, 7310);
    assert_eq!(stat.comm, "we (ird) x)");
    assert_eq!(stat.ppid, 7300);
    assert_eq!(stat.starttime, 98765);
}

#[test]
fn kernel_thread_has_no_tty() {
    let stat = parse_stat(&fixture("pid-stat-kthread.txt")).unwrap();
    assert_eq!(
        (stat.pid, stat.ppid, stat.comm.as_str()),
        (2, 0, "kthreadd")
    );
    assert_eq!(tty_path(stat.tty_nr), None);
}

#[test]
fn malformed_stat_fails() {
    for bad in [
        &b""[..],
        b"2481 bash S 1",
        b"2481 (bash) S x 1 1 0",
        b"(bash) S 1 1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 5",
        b"2481 (bash) S 1 1 1 0",
        b"2481 (bash\xff) S",
    ] {
        assert_eq!(parse_stat(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn tty_numbers() {
    assert_eq!(tty_path(0), None);
    assert_eq!(tty_path(pts(0)).as_deref(), Some("/dev/pts/0"));
    assert_eq!(tty_path(pts(255)).as_deref(), Some("/dev/pts/255"));
    // pts 256 is major 137, minor 0.
    assert_eq!(tty_path(137 << 8).as_deref(), Some("/dev/pts/256"));
    // Minor bits above 255 live in bits 20..31.
    assert_eq!(
        tty_path((136 << 8) | (1 << 20)).as_deref(),
        Some("/dev/pts/256")
    );
    assert_eq!(tty_path((4 << 8) | 2).as_deref(), Some("/dev/tty2"));
    // A serial line (ttyS0, major 4 minor 64) is not a terminal pane.
    assert_eq!(tty_path((4 << 8) | 64), None);
}

#[test]
fn status_uids_real_then_effective() {
    assert_eq!(
        parse_status_uids(&fixture("pid-status.txt")),
        Some((1000, 1000))
    );
    assert_eq!(
        parse_status_uids(&fixture("pid-status-setuid.txt")),
        Some((1000, 0))
    );
    assert_eq!(parse_status_uids(b"Name:\tx\n"), None);
}

#[test]
fn boot_time_and_id() {
    assert_eq!(parse_btime(&fixture("proc-stat.txt")), Some(BTIME));
    let id = parse_boot_id(&fixture("boot_id.txt")).unwrap();
    // Case and trailing newline do not change it; another id does.
    assert_eq!(
        parse_boot_id(b"6F1A2B3C-4D5E-4F60-8A7B-9C0D1E2F3A4B"),
        Some(id)
    );
    assert_ne!(
        parse_boot_id(b"6f1a2b3c-4d5e-4f60-8a7b-9c0d1e2f3a4c\n"),
        Some(id)
    );
    assert_eq!(parse_boot_id(b"not-a-uuid"), None);
    assert_eq!(start_seconds(BTIME, 51234, 100), Some(BTIME + 512));
    assert_eq!(start_seconds(BTIME, 51234, 0), None);
}

#[test]
fn cmdline_is_nul_separated() {
    assert_eq!(
        parse_cmdline(b"/usr/bin/grok\0--resume\0a b\0"),
        vec!["/usr/bin/grok", "--resume", "a b"]
    );
    assert!(parse_cmdline(b"").is_empty());
    assert_eq!(parse_cmdline(b"x"), vec!["x"]);
}

#[test]
fn exe_links() {
    assert_eq!(
        link_target("/usr/bin/kitty"),
        Some(("/usr/bin/kitty".into(), false))
    );
    assert_eq!(
        link_target("/home/example/.grok/bin/grok-0.1.22-linux-x86_64 (deleted)"),
        Some((
            "/home/example/.grok/bin/grok-0.1.22-linux-x86_64".into(),
            true
        ))
    );
    assert_eq!(link_target("anon_inode:[eventfd]"), None);
    assert_eq!(
        link_target("/memfd:x (deleted)"),
        Some(("/memfd:x".into(), true))
    );
}

#[test]
fn environ_keeps_only_identity_keys() {
    let environ = b"HOME=/home/example\0WINDOWID=60817415\0AWS_SECRET_ACCESS_KEY=s3cr3t\0\
KITTY_LISTEN_ON=unix:/run/user/1000/kitty-2470\0TMUX=/tmp/tmux-1000/default,3200,0\0\
WEZTERM_UNIX_SOCKET=/run/user/1000/wezterm/gui-sock-4100\0BROKEN\0WINDOWID2=1\0";
    let found = parse_environ(environ);
    assert_eq!(found.window, Some(60_817_415));
    assert_eq!(found.kitty_listen_on.as_deref(), Some(KITTY_ADDRESS));
    assert_eq!(found.wezterm_socket.as_deref(), Some(WEZ_SOCKET));
    assert_eq!(found.tmux.as_deref(), Some("/tmp/tmux-1000/default,3200,0"));
    let debug = format!("{found:?}");
    assert!(!debug.contains("s3cr3t") && !debug.contains("/home/example\""));
    assert_eq!(
        tmux_socket("/tmp/tmux-1000/default,3200,0"),
        Some(TMUX_SOCKET)
    );
    assert_eq!(tmux_socket("relative,1,0"), None);
    assert_eq!(parse_environ(b"WINDOWID=0x3a\0").window, None);
    assert_eq!(parse_environ(b"WINDOWID=0\0").window, None);
}

#[test]
fn classification_uses_the_kernel_executable() {
    for (path, exe) in [
        (
            "/home/example/.local/share/claude/versions/2.1.321",
            Exe::ClaudeCli,
        ),
        ("/home/example/.local/bin/claude", Exe::ClaudeCli),
        (
            "/usr/lib/node_modules/@openai/codex/bin/codex-x86_64-unknown-linux-musl",
            Exe::CodexCli,
        ),
        ("/home/example/.opencode/bin/opencode", Exe::OpenCodeCli),
        (GROK_REAL, Exe::GrokCli),
        ("/usr/bin/tmux", Exe::Tmux),
        ("/usr/bin/kitty", Exe::Kitty),
        ("/usr/bin/wezterm-gui", Exe::WezTerm),
        ("/usr/libexec/gnome-terminal-server", Exe::GnomeTerminal),
        ("/usr/bin/konsole", Exe::Konsole),
        ("/usr/bin/xterm", Exe::Xterm),
        ("/usr/bin/alacritty", Exe::Alacritty),
        ("/usr/bin/ghostty", Exe::Ghostty),
        ("/usr/bin/bash", Exe::Shell),
        ("/usr/bin/login", Exe::Login),
        // An npm Claude is `node`: not an agent CLI (as on macOS).
        ("/usr/bin/node", Exe::Other),
        ("/usr/bin/gnome-terminal", Exe::Other),
    ] {
        assert_eq!(classify_exe(path), exe, "{path}");
    }
    // `comm` alone (an unreadable exe) never makes an agent or a terminal.
    assert_eq!(classify_comm("claude"), Exe::Other);
    assert_eq!(classify_comm("kitty"), Exe::Other);
    assert_eq!(classify_comm("tmux: server"), Exe::Other);
    assert_eq!(classify_comm("login"), Exe::Login);
    assert_eq!(classify_comm("bash"), Exe::Shell);
}

#[test]
fn display_server_detection() {
    let with = |vars: &[(&str, &str)]| {
        LinuxConfig::for_user(HOME, UID)
            .with_session_env(vars.iter().map(|(k, v)| (*k, *v)))
            .display_server()
    };
    assert_eq!(
        with(&[("XDG_SESSION_TYPE", "x11")]),
        Fact::Known(DisplayServer::X11)
    );
    assert_eq!(
        with(&[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0")]),
        Fact::Known(DisplayServer::Wayland)
    );
    // XWayland sets DISPLAY too: WAYLAND_DISPLAY wins.
    assert_eq!(
        with(&[
            ("XDG_SESSION_TYPE", "tty"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("DISPLAY", ":0")
        ]),
        Fact::Known(DisplayServer::Wayland)
    );
    assert_eq!(with(&[("DISPLAY", ":1")]), Fact::Known(DisplayServer::X11));
    assert_eq!(with(&[("DISPLAY", "")]), Fact::Unknown);
    assert_eq!(with(&[]), Fact::Unknown);
    // Only the session keys are kept, and never a value with a newline.
    let cfg = LinuxConfig::for_user(HOME, UID).with_session_env([
        ("AWS_SECRET_ACCESS_KEY", "x"),
        ("DISPLAY", ":0\nX"),
        ("HOME", HOME),
    ]);
    assert_eq!(cfg.session_env, vec![("HOME".to_owned(), HOME.to_owned())]);
}

#[test]
fn xdg_dirs() {
    let cfg = LinuxConfig::for_user(HOME, UID)
        .with_xdg_data(Some("/data/home"), Some("/opt/share:relative:/usr/share"))
        .with_xdg_state(Some("/state/"));
    assert_eq!(
        cfg.data_dirs,
        vec!["/data/home", "/opt/share", "/usr/share"]
    );
    assert_eq!(
        cfg.common.paths.reservation_dir,
        "/state/session-navigation/reservations"
    );
    let plain = LinuxConfig::for_user(HOME, UID).with_xdg_state(None);
    assert_eq!(
        plain.common.paths.reservation_dir,
        "/home/example/.local/state/session-navigation/reservations"
    );
}

#[test]
fn wmctrl_rows_and_owners() {
    let rows = parse_wmctrl(&fixture("wmctrl-lp.txt")).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!((rows[1].id, rows[1].pid), (0x03a0_0007, 2470));
    assert_eq!(rows[0].pid, 1630);
    assert_eq!(rows[3].pid, 0);
    assert_eq!(rows[5].machine, "N/A");
    assert_eq!(parse_wmctrl(b"0x01 x 1 host t\n"), None);
    assert_eq!(parse_wmctrl(b"12 0 1 host t\n"), None);
    let mut m = Machine::new(Some("x11"));
    m.push(Proc::new(2470, 1600, KITTY, &["kitty"]));
    m.push(Proc::new(
        3500,
        1600,
        "/usr/lib/firefox/firefox",
        &["firefox"],
    ));
    m.push(Proc::new(9999, 1600, "/usr/bin/sleep", &["sleep"]));
    let table: Vec<_> = m
        .procs
        .iter()
        .map(|p| session_navigation::evidence::ProcessInfo {
            identity: p.identity(),
            ppid: p.ppid,
            euid: p.euid,
            ruid: p.ruid,
            exe: Exe::Other,
            tty: None,
            argv: vec![],
            cwd: Fact::Unknown,
            window: None,
        })
        .collect();
    let windows = owned(&rows, &table, Some("devbox"));
    let ids: Vec<u64> = windows.iter().map(|w| w.id.0).collect();
    // No pid, a pid not in the table (1630, 3100), another host's pid: left out.
    assert_eq!(ids, vec![0x03a0_0007, 0x0460_0002]);
    assert_eq!(windows[0].owner, identity(2470, 2470));
    assert_eq!(
        parse_number(&fixture("xdotool-getactivewindow.txt")),
        Some(60_817_415)
    );
    assert_eq!(parse_number(b"0x3a\n"), None);
}
