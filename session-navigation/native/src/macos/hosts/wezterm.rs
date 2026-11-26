//! WezTerm panes on macOS through `wezterm cli --no-auto-start list` on each
//! GUI's socket.
//!
//! UNVERIFIED (WezTerm not installed): the GUI socket is
//! `~/.local/share/wezterm/gui-sock-<wezterm-gui pid>`, the CLI is
//! `<WezTerm.app>/Contents/MacOS/wezterm`, and the GUI lists itself in
//! `list-clients` with its own pid and focused pane.
use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::{Ability, PaneRef, Setting};
use session_navigation::model::App;

use super::super::config::MacConfig;
use super::super::machine::app_executable;
use super::super::parse::AppsReport;
use super::ApiView;
use crate::common::wezterm as api;
use crate::system::{Output, Run, RunError, System};

/// The `wezterm` CLI inside the installed app.
pub fn program(report: Option<&AppsReport>) -> Option<String> {
    app_executable(report?, App::WezTerm, "wezterm")
}

/// One `wezterm cli` run against `socket`.
pub fn run(program: &str, args: Vec<String>, socket: &str, cfg: &MacConfig) -> Run {
    let mut run = Run::with_args(program, args)
        .timeout(cfg.common.timeouts.probe)
        .max_output(api::MAX_LIST);
    run.env = api::socket_env(socket);
    run
}

enum Answer {
    Listed(Output),
    Gone,
    Unknown,
}

fn ask<S: System>(sys: &S, run: &Run) -> Answer {
    match sys.run(run) {
        Ok(output) if output.success() => Answer::Listed(output),
        Ok(output) => match api::classify_error(&output.stderr) {
            api::CliError::Gone => Answer::Gone,
            _ => Answer::Unknown,
        },
        Err(RunError::NotFound | RunError::TimedOut | RunError::TooLarge | RunError::Io) => {
            Answer::Unknown
        }
    }
}

pub fn observe<S: System>(
    sys: &S,
    cfg: &MacConfig,
    table: Option<&[ProcessInfo]>,
    report: Option<&AppsReport>,
) -> ApiView {
    let off = Ability::NeedsSetting(Setting::WezTermMuxSocket);
    let Some(table) = table else {
        return ApiView::unknown(off);
    };
    if !super::running(table, cfg.uid, Exe::WezTerm) {
        return ApiView::none(off);
    }
    let Some(sockets) = super::sockets(
        sys,
        &cfg.hosts.wezterm_socket_dirs,
        table,
        cfg.uid,
        Exe::WezTerm,
        api::socket_owner_pid,
    ) else {
        return ApiView::unknown(off);
    };
    if sockets.is_empty() {
        return ApiView::none(off);
    }
    let Some(program) = program(report) else {
        return ApiView::unknown(off);
    };
    let mut panes = Vec::new();
    let mut focused: Vec<(PaneRef, Option<String>)> = Vec::new();
    let mut answered = false;
    for (socket, gui_pid) in sockets {
        if !api::is_valid_socket(&socket) {
            continue;
        }
        let listed = match ask(sys, &run(&program, api::list_args(), &socket, cfg)) {
            Answer::Listed(output) => output,
            Answer::Gone => continue,
            Answer::Unknown => return ApiView::unknown(off),
        };
        let Some(listed) = api::parse_list(&listed.stdout) else {
            return ApiView::unknown(off);
        };
        answered = true;
        // The focused pane is a hint for `front`; failing to read it
        // leaves the panes standing.
        if let Answer::Listed(out) = ask(sys, &run(&program, api::clients_args(), &socket, cfg)) {
            if let Some(clients) = api::parse_clients(&out.stdout) {
                if let Some(pane) = api::focused(&socket, gui_pid, &clients, &listed) {
                    let tty = match &pane {
                        PaneRef::WezTermPane { id, .. } => listed
                            .iter()
                            .find(|p| p.id == *id)
                            .and_then(|p| p.tty.clone()),
                        _ => None,
                    };
                    focused.push((pane, tty));
                }
            }
        }
        panes.extend(api::pane_infos(&socket, &listed));
    }
    let (front, front_tty) = match focused.as_slice() {
        [(pane, tty)] => (
            Fact::Known(Some(pane.clone())),
            tty.clone().map_or(Fact::Unknown, |t| Fact::Known(Some(t))),
        ),
        _ => (Fact::Unknown, Fact::Unknown),
    };
    ApiView {
        panes: Fact::Known(panes),
        ability: if answered { Ability::Available } else { off },
        front,
        front_tty,
    }
}
