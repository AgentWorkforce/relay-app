//! Where a live process can be returned to, from evidence only: its tty and
//! the kernel table on Unix, process identity and window ownership on
//! Windows, and what the adapter reports each terminal can do. Nothing is
//! assumed from a terminal's name.
use super::interpret::{chain, find_host};
use super::Context;
use crate::evidence::{Fact, ProcessInfo};
use crate::hosts::{Ability, PaneRef, TerminalSupport, Unsupported, WindowId};
use crate::model::{App, DisplayServer, Host, Platform, Precision, ProcessIdentity, Reason};
use crate::model::{Runtime, TerminalKind};

/// How to bring a live pane back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Return {
    /// Focus a pane or raise a window; the plan's precision is at most `cap`.
    Focus {
        terminal: TerminalKind,
        pane: PaneRef,
        cap: Precision,
        note: Option<Reason>,
    },
    /// Only the terminal application can come forward.
    Activate { app: App, note: Reason },
    /// The pane lives in a tmux session no client shows: attach one.
    Attach {
        socket: String,
        server: ProcessIdentity,
        session: String,
        window: String,
        pane: String,
    },
}

/// Where a live process can be returned to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Pane(Return),
    Desktop(App),
    Headless,
    /// In a terminal, but not reachable; the reason says why.
    Unlocatable(Reason),
}

pub fn place(cx: &Context<'_>, process: &ProcessInfo) -> Place {
    // A WSL or foreign process is never in this machine's windows.
    if !matches!(cx.snap.runtime, Fact::Known(Runtime::Native)) {
        return match find_host(cx.table, process, true) {
            Fact::Known((Host::Headless, _)) => Place::Headless,
            _ => Place::Unlocatable(Reason::LiveNotLocatable),
        };
    }
    let unix = cx.snap.platform.is_unix();
    match (find_host(cx.table, process, unix), &process.tty) {
        (Fact::Known((Host::Terminal(terminal), Some(host))), _) => {
            match locate_terminal(cx, process, terminal, host) {
                Ok(ret) => Place::Pane(ret),
                Err(reason) => Place::Unlocatable(reason),
            }
        }
        (Fact::Known((Host::Tmux(_), Some(server))), Some(tty)) => {
            match locate_tmux(cx, server, tty) {
                Ok(ret) => Place::Pane(ret),
                Err(reason) => Place::Unlocatable(reason),
            }
        }
        (Fact::Known((Host::Desktop(app), _)), None) => Place::Desktop(app),
        (Fact::Known((Host::Headless, _)), None) => Place::Headless,
        _ => Place::Unlocatable(Reason::LiveNotLocatable),
    }
}

fn blocked_by(ability: Ability) -> Option<Reason> {
    match ability {
        Ability::Available => None,
        Ability::NeedsSetting(_) => Some(Reason::HostSettingDisabled),
        Ability::NeedsPermission(_) => Some(Reason::PermissionRequired),
        Ability::Unsupported(Unsupported::Wayland) => Some(Reason::WaylandNoRaise),
        Ability::Unsupported(Unsupported::NotOnThisPlatform) => Some(Reason::PlatformUnsupported),
        Ability::Unsupported(_) => Some(Reason::HostUnsupported),
    }
}

fn tty_scripted(terminal: TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::Ghostty | TerminalKind::AppleTerminal | TerminalKind::ITerm2
    )
}

/// The pane-API pane holding `process`: by tty, or by the pane's own
/// process being `process` or one of its ancestors below the terminal.
fn api_pane(
    cx: &Context<'_>,
    process: &ProcessInfo,
    terminal: TerminalKind,
    host: &ProcessInfo,
) -> Option<PaneRef> {
    let Fact::Known(panes) = &cx.snap.surfaces.panes else {
        return None;
    };
    let lineage = chain(cx.table, process, Some(host.identity.pid));
    let found: Vec<&PaneRef> = panes
        .iter()
        .filter(|p| p.terminal == terminal)
        .filter(|p| {
            let by_tty = p.tty.is_some() && p.tty == process.tty;
            let by_pid = p
                .pid
                .is_some_and(|pid| lineage.iter().any(|a| a.identity.pid == pid));
            by_tty || by_pid
        })
        .map(|p| &p.pane)
        .collect();
    match found.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

/// The one window holding `process`: the window it declares, if its own
/// terminal owns it; else the only window that terminal owns (for a classic
/// console, owned by the console host or by its first client).
fn window(
    cx: &Context<'_>,
    process: &ProcessInfo,
    host: &ProcessInfo,
) -> Result<(WindowId, ProcessIdentity), Reason> {
    let Fact::Known(windows) = &cx.snap.surfaces.windows else {
        return Err(Reason::EvidenceIncomplete);
    };
    let console_client = (host.exe == crate::evidence::Exe::ConsoleHost)
        .then(|| cx.table.iter().find(|p| p.identity.pid == host.ppid))
        .flatten();
    let owned = |owner: &ProcessIdentity| {
        *owner == host.identity || console_client.is_some_and(|c| c.identity == *owner)
    };
    let lineage = chain(cx.table, process, Some(host.identity.pid));
    let declared = lineage.iter().find_map(|p| p.window);
    if let Some(id) = declared {
        if let Some(w) = windows.iter().find(|w| w.id == id && owned(&w.owner)) {
            return Ok((w.id, w.owner));
        }
    }
    let candidates: Vec<_> = windows.iter().filter(|w| owned(&w.owner)).collect();
    match candidates.as_slice() {
        [one] => Ok((one.id, one.owner)),
        [] => Err(Reason::LiveNotLocatable),
        _ => Err(Reason::WindowAmbiguous),
    }
}

fn window_ref(platform: Platform, id: WindowId, owner: ProcessIdentity) -> Option<PaneRef> {
    match platform {
        Platform::Windows => Some(PaneRef::WindowsWindow { hwnd: id, owner }),
        Platform::Linux {
            session: Fact::Known(DisplayServer::X11),
        } => Some(PaneRef::X11Window { id, owner }),
        _ => None,
    }
}

fn locate_terminal(
    cx: &Context<'_>,
    process: &ProcessInfo,
    terminal: TerminalKind,
    host: &ProcessInfo,
) -> Result<Return, Reason> {
    let support: TerminalSupport = match cx.snap.surfaces.support(terminal) {
        Fact::Known(s) => s,
        Fact::Unknown => return Err(Reason::EvidenceIncomplete),
    };
    let mut why: Vec<Reason> = Vec::new();
    match support.focus_pane {
        Ability::Available => {
            let pane = if tty_scripted(terminal) {
                process.tty.clone().map(|tty| PaneRef::Tty { tty })
            } else {
                api_pane(cx, process, terminal, host)
            };
            if let Some(pane) = pane {
                return Ok(Return::Focus {
                    terminal,
                    pane,
                    cap: Precision::ExactConversation,
                    note: None,
                });
            }
        }
        other => why.extend(blocked_by(other)),
    }
    match support.raise_window {
        Ability::Available => match window(cx, process, host) {
            Ok((id, owner)) => {
                if let Some(pane) = window_ref(cx.snap.platform, id, owner) {
                    let (cap, note) = if support.tabs_share_window {
                        (Precision::AppOnly, Some(Reason::PaneSelectionUnavailable))
                    } else {
                        (Precision::ExactConversation, None)
                    };
                    return Ok(Return::Focus {
                        terminal,
                        pane,
                        cap,
                        note,
                    });
                }
            }
            Err(reason) => why.push(reason),
        },
        other => why.extend(blocked_by(other)),
    }
    if support.activate_app.available() {
        let note = why
            .iter()
            .copied()
            .find(|r| *r != Reason::LiveNotLocatable)
            .unwrap_or(Reason::PaneSelectionUnavailable);
        return Ok(Return::Activate {
            app: terminal.app(),
            note,
        });
    }
    why.extend(blocked_by(support.activate_app));
    Err(strongest(&why))
}

/// The most specific reason: what the person can act on comes first.
fn strongest(why: &[Reason]) -> Reason {
    const ORDER: [Reason; 7] = [
        Reason::EvidenceIncomplete,
        Reason::WindowAmbiguous,
        Reason::WaylandNoRaise,
        Reason::HostSettingDisabled,
        Reason::PermissionRequired,
        Reason::PlatformUnsupported,
        Reason::HostUnsupported,
    ];
    ORDER
        .into_iter()
        .find(|r| why.contains(r))
        .unwrap_or(Reason::LiveNotLocatable)
}

fn locate_tmux(cx: &Context<'_>, server: &ProcessInfo, tty: &str) -> Result<Return, Reason> {
    let Fact::Known(servers) = &cx.snap.surfaces.tmux else {
        return Err(Reason::EvidenceIncomplete);
    };
    let found = servers.iter().find(|s| s.server == server.identity);
    let Some(srv) = found else {
        return Err(Reason::LiveNotLocatable);
    };
    let Some(pane) = srv.panes.iter().find(|p| p.tty == tty) else {
        return Err(Reason::LiveNotLocatable);
    };
    let mut clients: Vec<_> = srv
        .clients
        .iter()
        .filter(|c| c.session == pane.session)
        .collect();
    clients.sort_by(|a, b| a.tty.cmp(&b.tty));
    if clients.is_empty() {
        return Ok(Return::Attach {
            socket: srv.socket.clone(),
            server: srv.server,
            session: pane.session.clone(),
            window: pane.window.clone(),
            pane: pane.pane.clone(),
        });
    }
    let mut fallback = Err(Reason::LiveNotLocatable);
    for client in clients {
        let proc = cx.table.iter().find(|p| {
            p.identity.pid == client.pid
                && p.exe == crate::evidence::Exe::Tmux
                && p.tty.as_deref() == Some(client.tty.as_str())
        });
        let Some(proc) = proc else {
            continue;
        };
        let Fact::Known((Host::Terminal(terminal), Some(host))) = find_host(cx.table, proc, true)
        else {
            continue;
        };
        match locate_terminal(cx, proc, terminal, host) {
            Ok(Return::Focus {
                terminal,
                pane: outer,
                cap,
                note,
            }) => {
                return Ok(Return::Focus {
                    terminal,
                    pane: PaneRef::TmuxPane {
                        socket: srv.socket.clone(),
                        server: srv.server,
                        pane: pane.pane.clone(),
                        client_tty: client.tty.clone(),
                        outer: Box::new(outer),
                    },
                    cap,
                    note,
                })
            }
            Ok(other) if fallback.is_err() => fallback = Ok(other),
            Err(reason) if fallback.is_err() => fallback = Err(reason),
            _ => {}
        }
    }
    fallback
}
