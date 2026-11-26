//! Kitty panes on macOS through `kitty @ --to unix:<socket> ls`.
//!
//! UNVERIFIED (Kitty not installed): sockets are found as
//! `<dir>/<name>-<kitty pid>` (Kitty appends its pid to a `listen_on` path
//! set in kitty.conf), the CLI is `<kitty.app>/Contents/MacOS/kitty`, and a
//! refused / disabled remote control is recognised from its error text.
use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::{Ability, Setting};
use session_navigation::model::App;

use super::super::config::MacConfig;
use super::super::machine::app_executable;
use super::super::parse::AppsReport;
use super::ApiView;
use crate::common::kitty as api;
use crate::system::{Run, System};

/// The `kitty` CLI inside the installed app.
pub fn program(report: Option<&AppsReport>) -> Option<String> {
    app_executable(report?, App::Kitty, "kitty")
}

/// One `kitty @` run on `address`.
pub fn run(program: &str, args: Vec<String>, cfg: &MacConfig) -> Run {
    Run::with_args(program, args)
        .timeout(cfg.common.timeouts.probe)
        .max_output(api::MAX_LS)
}

pub fn observe<S: System>(
    sys: &S,
    cfg: &MacConfig,
    table: Option<&[ProcessInfo]>,
    report: Option<&AppsReport>,
) -> ApiView {
    let off = Ability::NeedsSetting(Setting::KittyRemoteControl);
    let Some(table) = table else {
        return ApiView::unknown(off);
    };
    if !super::running(table, cfg.uid, Exe::Kitty) {
        return ApiView::none(off);
    }
    let Some(sockets) = super::sockets(
        sys,
        &cfg.hosts.kitty_socket_dirs,
        table,
        cfg.uid,
        Exe::Kitty,
        api::socket_owner_pid,
    ) else {
        return ApiView::unknown(off);
    };
    if sockets.is_empty() {
        // Kitty runs without a socket we can find: remote control is off
        // (or `listen_on` points elsewhere).
        return ApiView::none(off);
    }
    let Some(program) = program(report) else {
        return ApiView::unknown(off);
    };
    let mut panes = Vec::new();
    let mut focused = Vec::new();
    let mut answered = false;
    for (path, _) in sockets {
        let address = api::address_for_path(&path);
        if !api::is_valid_address(&address) {
            continue;
        }
        let Ok(output) = sys.run(&run(&program, api::ls_args(&address), cfg)) else {
            return ApiView::unknown(off);
        };
        if !output.success() {
            match api::classify_error(&output.stderr) {
                // Stale socket or remote control refused: no panes here.
                api::RemoteError::Gone | api::RemoteError::Refused => continue,
                api::RemoteError::Other => return ApiView::unknown(off),
            }
        }
        let Some(listed) = api::parse_ls(&output.stdout) else {
            return ApiView::unknown(off);
        };
        answered = true;
        focused.extend(api::focused(&address, &listed));
        panes.extend(api::pane_infos(&address, &listed));
    }
    let front = match focused.as_slice() {
        [one] => Fact::Known(Some(one.clone())),
        _ => Fact::Unknown,
    };
    ApiView {
        panes: Fact::Known(panes),
        ability: if answered { Ability::Available } else { off },
        front: if answered { front } else { Fact::Unknown },
        // Kitty's `ls` reports no tty.
        front_tty: Fact::Unknown,
    }
}
