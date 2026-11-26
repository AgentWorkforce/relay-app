//! A fake Mac running Kitty, WezTerm and tmux (plus the default Ghostty
//! chain), answering with the documentation-derived fixtures under
//! `fixtures/macos/` (see PROVENANCE.txt there: none of these tools was
//! installed, nothing here was captured).
#![allow(dead_code)]

use session_navigation::model::{App, BootId, ProcessIdentity};
use session_navigation_native::common::{kitty, tmux, wezterm};
use session_navigation_native::macos::hosts::{
    kitty as kitty_host, tmux as tmux_host, wezterm as wezterm_host,
};
use session_navigation_native::system::{Output, Run};

use super::common::*;

pub const KITTY_LS: &str = include_str!("../fixtures/macos/kitty-ls.json");
pub const KITTY_GONE: &str = include_str!("../fixtures/macos/kitty-error-gone.txt");
pub const KITTY_DISABLED: &str = include_str!("../fixtures/macos/kitty-error-disabled.txt");
pub const WEZ_LIST: &str = include_str!("../fixtures/macos/wezterm-list.json");
pub const WEZ_CLIENTS: &str = include_str!("../fixtures/macos/wezterm-list-clients.json");
pub const WEZ_GONE: &str = include_str!("../fixtures/macos/wezterm-error-gone.txt");
pub const TMUX_PID: &str = include_str!("../fixtures/macos/tmux-server-pid.txt");
pub const TMUX_PANES: &str = include_str!("../fixtures/macos/tmux-list-panes.txt");
pub const TMUX_CLIENTS: &str = include_str!("../fixtures/macos/tmux-list-clients.txt");
pub const TMUX_GONE: &str = include_str!("../fixtures/macos/tmux-error-gone.txt");

pub const KITTY: &str = "/Applications/kitty.app/Contents/MacOS/kitty";
pub const KITTY_SOCK: &str = "/private/tmp/kitty-2000";
pub const KITTY_ADDR: &str = "unix:/private/tmp/kitty-2000";
pub const WEZ: &str = "/Applications/WezTerm.app/Contents/MacOS/wezterm";
pub const WEZ_SOCK: &str = "/Users/example/.local/share/wezterm/gui-sock-3000";
pub const TMUX: &str = "/opt/homebrew/bin/tmux";
pub const TMUX_SOCK: &str = "/private/tmp/tmux-501/default";
pub const GROK_ID: &str = "0b5a3c1e-1111-4222-8333-444455556666";

pub fn ident(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        start: BOOT + 1000 + u64::from(pid),
        boot: BootId(BOOT),
    }
}

/// The registry query with iTerm2, Kitty, WezTerm and Alacritty installed.
pub fn apps_with_terminals(front: Option<App>, running: &[App]) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(&apps_json(front, running)).unwrap_or_default();
    for (bundle, path, version) in [
        ("com.googlecode.iterm2", "/Applications/iTerm.app", "3.6.6"),
        ("net.kovidgoyal.kitty", "/Applications/kitty.app", "0.44.0"),
        (
            "com.github.wez.wezterm",
            "/Applications/WezTerm.app",
            "20240203-110809-5046fc22",
        ),
        ("org.alacritty", "/Applications/Alacritty.app", "0.16.1"),
    ] {
        value["apps"][bundle] = serde_json::json!({ "path": path, "version": version });
    }
    value.to_string()
}

pub fn kitty_procs() -> Vec<Proc> {
    vec![
        Proc::new(2000, 1, "??", "kitty", KITTY, KITTY),
        Proc::new(2001, 2000, "ttys004", "zsh", "-zsh", "-zsh"),
        Proc::new(
            2002,
            2001,
            "ttys004",
            "codex",
            "/opt/homebrew/bin/codex",
            &format!("/opt/homebrew/bin/codex resume {CODEX_ID}"),
        ),
        Proc::new(2010, 2000, "ttys005", "zsh", "-zsh", "-zsh"),
    ]
}

pub fn wezterm_procs() -> Vec<Proc> {
    let gui = "/Applications/WezTerm.app/Contents/MacOS/wezterm-gui";
    vec![
        Proc::new(3000, 1, "??", "wezterm-gui", gui, gui),
        Proc::new(3001, 3000, "ttys006", "zsh", "-zsh", "-zsh"),
        Proc::new(3009, 3000, "ttys009", "zsh", "-zsh", "-zsh"),
    ]
}

/// A tmux server (4000) whose pane %0 (ttys007) runs Grok (4003), shown by
/// a client (4001) in the Ghostty pane on ttys000.
pub fn tmux_procs() -> Vec<Proc> {
    vec![
        Proc::new(4000, 1, "??", "tmux", "tmux", "tmux new-session -s work"),
        Proc::new(4001, 998, "ttys000", "tmux", "tmux", "tmux attach -t work"),
        Proc::new(4002, 4000, "ttys007", "zsh", "-zsh", "-zsh"),
        Proc::new(
            4003,
            4002,
            "ttys007",
            "grok",
            "/Users/example/.grok/bin/grok",
            "/Users/example/.grok/bin/grok",
        ),
    ]
}

pub fn kitty_run(args: Vec<String>) -> Run {
    kitty_host::run(KITTY, args, &cfg())
}

pub fn wez_run(args: Vec<String>) -> Run {
    wezterm_host::run(WEZ, args, WEZ_SOCK, &cfg())
}

pub fn tmux_run(args: Vec<String>) -> Run {
    tmux_host::run(TMUX, args, &cfg())
}

/// Everything running and answering.
pub fn host_machine() -> Machine {
    let mut m = Machine::new();
    m.procs.extend(kitty_procs());
    m.procs.extend(wezterm_procs());
    m.procs.extend(tmux_procs());
    m.apps_json = apps_with_terminals(None, &[App::Ghostty, App::Kitty, App::WezTerm]);
    m.file(KITTY_SOCK, Vec::new());
    m.file(WEZ_SOCK, Vec::new());
    m.file(TMUX_SOCK, Vec::new());
    m.tools.push((TMUX.into(), "tmux 3.5a\n".into()));
    m.run(
        kitty_run(kitty::ls_args(KITTY_ADDR)),
        Ok(Output::ok(KITTY_LS)),
    );
    m.run(wez_run(wezterm::list_args()), Ok(Output::ok(WEZ_LIST)));
    m.run(
        wez_run(wezterm::clients_args()),
        Ok(Output::ok(WEZ_CLIENTS)),
    );
    m.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::ok(TMUX_PID)),
    );
    m.run(
        tmux_run(tmux::list_panes_args(TMUX_SOCK)),
        Ok(Output::ok(TMUX_PANES)),
    );
    m.run(
        tmux_run(tmux::list_clients_args(TMUX_SOCK)),
        Ok(Output::ok(TMUX_CLIENTS)),
    );
    m
}
