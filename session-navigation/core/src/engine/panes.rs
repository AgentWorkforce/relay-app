//! Post-condition checks for terminal panes, windows and tmux. Only what the
//! fresh snapshot shows counts: the front app, the focused pane, tty or
//! window, the kernel table and the tmux listing.
use super::verify::wrong;
use crate::decide::decide;
use crate::evidence::{Fact, ProcessInfo, Snapshot};
use crate::hosts::{PaneRef, TmuxServer};
use crate::model::{ProcessIdentity, TerminalKind};
use crate::plan::{ActionKind, OpenRequest, Outcome, Step};

fn tmux_server<'a>(
    after: &'a Snapshot,
    socket: &str,
    server: &ProcessIdentity,
) -> Fact<Option<&'a TmuxServer>> {
    match &after.surfaces.tmux {
        Fact::Known(list) => Fact::Known(
            list.iter()
                .find(|s| s.socket == socket && s.server == *server),
        ),
        Fact::Unknown => Fact::Unknown,
    }
}

/// Whether the front shows `pane`, whose agent is on `tty` (Unix).
fn front_shows(after: &Snapshot, pane: &PaneRef, tty: Option<&str>) -> Fact<bool> {
    match pane {
        PaneRef::Tty { tty: want } => match &after.front.pane_tty {
            Fact::Known(front) => Fact::Known(front.as_deref() == Some(want) && tty == Some(want)),
            Fact::Unknown => Fact::Unknown,
        },
        PaneRef::KittyWindow { .. } | PaneRef::WezTermPane { .. } => match &after.front.pane {
            Fact::Known(front) => Fact::Known(front.as_ref() == Some(pane)),
            Fact::Unknown => Fact::Unknown,
        },
        PaneRef::X11Window { id, owner } | PaneRef::WindowsWindow { hwnd: id, owner } => {
            match (&after.front.window, &after.surfaces.windows) {
                (Fact::Known(front), Fact::Known(windows)) => Fact::Known(
                    *front == Some(*id) && windows.iter().any(|w| w.id == *id && w.owner == *owner),
                ),
                _ => Fact::Unknown,
            }
        }
        PaneRef::TmuxPane {
            socket,
            server,
            pane,
            client_tty,
            outer,
        } => {
            let outer_tty = match outer.as_ref() {
                PaneRef::Tty { tty } => Some(tty.as_str()),
                _ => Some(client_tty.as_str()),
            };
            let srv = match tmux_server(after, socket, server) {
                Fact::Known(srv) => srv,
                Fact::Unknown => return Fact::Unknown,
            };
            let shown = srv.is_some_and(|srv| {
                let holds = srv
                    .panes
                    .iter()
                    .any(|p| p.pane == *pane && Some(p.tty.as_str()) == tty);
                let client = srv
                    .clients
                    .iter()
                    .any(|c| c.tty == *client_tty && c.active_pane == *pane);
                holds && client
            });
            match front_shows(after, outer, outer_tty) {
                Fact::Known(front) => Fact::Known(front && shown),
                Fact::Unknown => Fact::Unknown,
            }
        }
    }
}

fn find<'a>(table: &'a [ProcessInfo], process: &ProcessIdentity) -> Option<&'a ProcessInfo> {
    table.iter().find(|p| p.identity == *process)
}

pub fn pane_focused(
    after: &Snapshot,
    terminal: TerminalKind,
    pane: &PaneRef,
    process: &ProcessIdentity,
) -> Outcome {
    match &after.front.app {
        Fact::Known(front) if *front == Some(terminal.app()) => {}
        Fact::Known(_) => return wrong(),
        Fact::Unknown => return Outcome::DispatchedUnverified,
    }
    let Fact::Known(table) = &after.processes else {
        return Outcome::DispatchedUnverified;
    };
    let tty = find(table, process).and_then(|p| p.tty.as_deref());
    let present = find(table, process).is_some();
    match front_shows(after, pane, tty) {
        Fact::Unknown => Outcome::DispatchedUnverified,
        Fact::Known(true) if present => Outcome::Verified,
        Fact::Known(_) => wrong(),
    }
}

/// A resume is verified when a fresh decision would focus the session in the
/// expected terminal and that pane is frontmost.
pub fn running_in(after: &Snapshot, terminal: TerminalKind, request: &OpenRequest) -> Outcome {
    let plain = OpenRequest {
        prefs: request.prefs,
        ..OpenRequest::default()
    };
    let decision = decide(after, &plain);
    let Some(action) = decision
        .action()
        .filter(|a| a.kind == ActionKind::FocusPane)
    else {
        return Outcome::DispatchedUnverified;
    };
    let Some(Step::FocusPane {
        terminal: t,
        pane,
        process,
    }) = action.steps.first()
    else {
        return Outcome::DispatchedUnverified;
    };
    let tty = after
        .processes
        .known()
        .and_then(|table| find(table, process))
        .and_then(|p| p.tty.as_deref());
    let front_ok = after.front.app.is(&Some(terminal.app()))
        && front_shows(after, pane, tty) == Fact::Known(true);
    if *t == terminal && front_ok {
        Outcome::Verified
    } else {
        Outcome::DispatchedUnverified
    }
}

/// An attach is verified when a client of the same server shows the pane,
/// the pane still holds the process, and that client's pane is frontmost.
pub fn tmux_attached(
    after: &Snapshot,
    terminal: TerminalKind,
    socket: &str,
    server: &ProcessIdentity,
    pane: &str,
    process: &ProcessIdentity,
) -> Outcome {
    if !after.front.app.is(&Some(terminal.app())) {
        return Outcome::DispatchedUnverified;
    }
    let (Fact::Known(table), Fact::Known(Some(front_tty)), Fact::Known(Some(srv))) = (
        &after.processes,
        &after.front.pane_tty,
        tmux_server(after, socket, server),
    ) else {
        return Outcome::DispatchedUnverified;
    };
    let tty = find(table, process).and_then(|p| p.tty.as_deref());
    let holds = srv
        .panes
        .iter()
        .any(|p| p.pane == pane && Some(p.tty.as_str()) == tty);
    let shown = srv
        .clients
        .iter()
        .any(|c| c.active_pane == pane && c.tty == *front_tty);
    if holds && shown {
        Outcome::Verified
    } else {
        Outcome::DispatchedUnverified
    }
}
