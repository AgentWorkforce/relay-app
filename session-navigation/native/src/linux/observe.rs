//! [`Observe`] for Linux, assembled from [`System`] calls and pure parsers.
//! Every failure, timeout, oversize or parse error becomes `Fact::Unknown`
//! (never a default, never a panic).
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use session_navigation::evidence::{Fact, Front, ProcessInfo, Snapshot, ToolInfo};
use session_navigation::hosts::{PaneRef, Surfaces, WindowId, WindowInfo};
use session_navigation::model::{
    App, DisplayServer, Platform, Provider, Runtime, SessionRef, TerminalKind,
};
use session_navigation::ports::Observe;

use super::apps::{self, app_of};
use super::config::LinuxConfig;
use super::panes::{self, Apis};
use super::support::{self, Resolved};
use super::table::{self, BootClock, EnvCache, Table};
use super::{tmux, x11};
use crate::common::par::{self, Reuse};
use crate::common::tools::ToolResolver;
use crate::common::{self, reservations, HostHistory, NoHistory};
use crate::system::System;

pub struct LinuxObserver<S, H = NoHistory> {
    sys: S,
    cfg: LinuxConfig,
    history: H,
    tools: ToolResolver,
    clock: BootClock,
    /// Tool paths and versions, reused for a short window.
    tool_reuse: Reuse<Vec<ToolInfo>>,
    /// Helper programs, the default terminal and the installed apps.
    slow: Reuse<Slow>,
    /// Environment identity keys by process instance, and when they were
    /// last read (dropped once older than the reuse window).
    env: (Option<Instant>, EnvCache),
    window: Duration,
}

/// What cannot change within one click.
#[derive(Clone, Debug)]
struct Slow {
    resolved: Resolved,
    default_terminal: Fact<Option<TerminalKind>>,
    installed: Option<Vec<(App, bool)>>,
}

impl<S: System> LinuxObserver<S, NoHistory> {
    pub fn new(sys: S, cfg: LinuxConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory, BootClock::new())
    }
}

/// The X11 part of an observation: windows and the active window.
struct X11View {
    windows: Fact<Vec<WindowInfo>>,
    active: Fact<Option<WindowId>>,
}

impl<S: System, H: HostHistory> LinuxObserver<S, H> {
    /// `clock` is shared with the executor ([`super::LinuxPorts`] does it)
    /// so both compute the same start times.
    pub fn with_history(sys: S, cfg: LinuxConfig, history: H, clock: BootClock) -> Self {
        Self {
            sys,
            cfg,
            history,
            tools: ToolResolver::new(),
            clock,
            tool_reuse: Reuse::default(),
            slow: Reuse::default(),
            env: (None, EnvCache::new()),
            window: par::REUSE,
        }
    }

    /// How long slow-changing facts (helper and tool paths, tool versions,
    /// the default terminal, installed apps, environment identity keys of a
    /// process already read) are reused between looks; 2 s unless set,
    /// zero for never.
    pub fn set_reuse(&mut self, window: Duration) {
        self.tool_reuse.set_window(window);
        self.slow.set_window(window);
        self.env = (None, EnvCache::new());
        self.window = window;
    }

    pub fn config(&self) -> &LinuxConfig {
        &self.cfg
    }

    fn front(
        &self,
        display: Fact<DisplayServer>,
        view: &X11View,
        apis: &Apis,
        table: Option<&Table>,
    ) -> Front {
        let processes = table.map(|t| t.processes.as_slice());
        if display != Fact::Known(DisplayServer::X11) {
            // No generic front window or app on Wayland (or without a
            // display); Kitty still says which of its OS windows has focus.
            return Front {
                app: Fact::Unknown,
                pane_tty: Fact::Unknown,
                pane: apis
                    .kitty_focused()
                    .map_or(Fact::Unknown, |p| Fact::Known(Some(p))),
                window: Fact::Unknown,
                view: Fact::Known(None),
            };
        }
        let owner = match (&view.active, &view.windows) {
            (Fact::Known(Some(id)), Fact::Known(windows)) => {
                windows.iter().find(|w| w.id == *id).map(|w| w.owner)
            }
            _ => None,
        };
        let owner_process =
            owner.and_then(|o| processes?.iter().find(|p| p.identity == o).cloned());
        let app = owner_process.as_ref().and_then(|p| app_of(p.exe));
        let app_fact = if owner_process.is_some() {
            Fact::Known(app)
        } else {
            Fact::Unknown
        };
        let pane = match app.and_then(App::terminal) {
            Some(TerminalKind::Kitty) => apis
                .kitty_focused()
                .map_or(Fact::Unknown, |p| Fact::Known(Some(p))),
            Some(TerminalKind::WezTerm) => owner
                .and_then(|o| apis.wezterm_focused(o.pid))
                .map_or(Fact::Unknown, |p| Fact::Known(Some(p))),
            _ if app_fact.is_known() => Fact::Known(None),
            _ => Fact::Unknown,
        };
        let pane_tty = match (&pane, app.and_then(App::terminal)) {
            (Fact::Known(Some(p)), _) => pane_tty(p, apis, processes),
            (_, Some(_)) => Fact::Unknown,
            (_, None) if app_fact.is_known() => Fact::Known(None),
            _ => Fact::Unknown,
        };
        Front {
            app: app_fact,
            pane_tty,
            pane,
            window: view.active,
            // No provider desktop app exists on Linux.
            view: Fact::Known(None),
        }
    }
}

/// The X11 windows and the active window.
fn x11_view<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    resolved: &Resolved,
    table: Option<&Table>,
) -> X11View {
    let unknown = X11View {
        windows: Fact::Unknown,
        active: Fact::Unknown,
    };
    let Some(table) = table else {
        return unknown;
    };
    let windows = x11::windows(sys, cfg, &resolved.x11, &table.processes);
    let active = x11::active(sys, cfg, &resolved.x11);
    X11View {
        windows: windows.map_or(Fact::Unknown, Fact::Known),
        active: active.map_or(Fact::Unknown, |id| Fact::Known(Some(id))),
    }
}

/// The tty of a focused API pane: WezTerm reports it; for Kitty it is the
/// tty of the window's own process.
fn pane_tty(pane: &PaneRef, apis: &Apis, table: Option<&[ProcessInfo]>) -> Fact<Option<String>> {
    let tty = match pane {
        PaneRef::WezTermPane { socket, id } => apis
            .wezterm
            .iter()
            .filter(|w| w.socket == *socket)
            .flat_map(|w| &w.panes)
            .find(|p| p.id == *id)
            .and_then(|p| p.tty.clone()),
        PaneRef::KittyWindow { socket, id } => apis
            .kitty
            .iter()
            .filter(|k| k.address == *socket)
            .flat_map(|k| &k.panes)
            .find(|p| p.id == *id)
            .and_then(|p| p.pid)
            .and_then(|pid| table?.iter().find(|p| p.identity.pid == pid))
            .and_then(|p| p.tty.clone()),
        _ => None,
    };
    tty.map_or(Fact::Unknown, |t| Fact::Known(Some(t)))
}

impl<S: System + Sync, H: HostHistory> Observe for LinuxObserver<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        let observed_at = self.sys.now();
        let display = self.cfg.display_server();
        let (sys, cfg, clock) = (&self.sys, &self.cfg, &self.clock);
        let common = &cfg.common;
        if self.env.0.is_none_or(|at| at.elapsed() >= self.window) {
            self.env = (None, EnvCache::new());
        }
        let (resolver, tool_reuse, slow, env) = (
            &mut self.tools,
            &mut self.tool_reuse,
            &mut self.slow,
            &mut self.env,
        );
        // Independent reads at once: the process table, the tools, the
        // slow-changing facts and (all but Claude's, which needs the
        // table) the provider's stores.
        let early = session.provider != Provider::Claude;
        let (((boot, table), tools, slow), early_found) = par::join(
            sys,
            || {
                par::join3(
                    sys,
                    || {
                        let boot = clock.read(sys);
                        let table =
                            boot.and_then(|b| table::read_table_with(sys, cfg, b, &mut env.1));
                        if table.is_some() {
                            env.0.get_or_insert_with(Instant::now);
                        }
                        (boot, table)
                    },
                    || tool_reuse.get_or_keep(|| resolver.all_at_once(sys, common), |_| true),
                    || {
                        slow.get_or_keep(
                            || Slow {
                                resolved: Resolved::find(sys, cfg),
                                default_terminal: super::default_terminal::read(sys, cfg),
                                installed: apps::installed(sys, cfg),
                            },
                            |_| true,
                        )
                    },
                )
            },
            || early.then(|| common::observe_provider(sys, common, session, None, &Fact::Unknown)),
        );
        let processes: Fact<Vec<ProcessInfo>> = table
            .as_ref()
            .map_or(Fact::Unknown, |t| Fact::Known(t.processes.clone()));
        let resolved = &slow.resolved;
        let tmux_program = tools
            .iter()
            .find(|t| t.tool == session_navigation::model::Tool::Tmux)
            .and_then(|t| t.path.clone());
        // Then what needs the table, at once.
        let (found, apis, (tmux, x11)) = par::join3(
            sys,
            || match early_found {
                Some(found) => found,
                None => common::observe_provider(
                    sys,
                    common,
                    session,
                    boot.map(|b| b.btime),
                    &processes,
                ),
            },
            || match &table {
                Some(t) => panes::discover(
                    sys,
                    cfg,
                    t,
                    resolved.launcher(TerminalKind::Kitty),
                    resolved.launcher(TerminalKind::WezTerm),
                ),
                None => Apis {
                    incomplete: true,
                    ..Apis::default()
                },
            },
            || {
                par::join(
                    sys,
                    || match &table {
                        Some(t) => tmux::servers(sys, cfg, tmux_program.as_deref(), t),
                        None => Fact::Unknown,
                    },
                    || match display {
                        Fact::Known(DisplayServer::X11) => {
                            x11_view(sys, cfg, resolved, table.as_ref())
                        }
                        // Wayland has no window list or front window to read.
                        Fact::Known(DisplayServer::Wayland) => X11View {
                            windows: Fact::Known(Vec::new()),
                            active: Fact::Unknown,
                        },
                        Fact::Unknown => X11View {
                            windows: Fact::Unknown,
                            active: Fact::Unknown,
                        },
                    },
                )
            },
        );
        let front = self.front(display, &x11, &apis, table.as_ref());
        let surfaces = Surfaces {
            terminals: Fact::Known(support::terminals(display, resolved, &apis)),
            panes: if apis.incomplete {
                Fact::Unknown
            } else {
                Fact::Known(apis.pane_infos())
            },
            tmux,
            windows: x11.windows,
        };
        Snapshot {
            session: session.clone(),
            observed_at,
            platform: Platform::Linux { session: display },
            runtime: Fact::Known(Runtime::Native),
            boot: boot.map_or(Fact::Unknown, |b| Fact::Known(b.id)),
            apps: apps::rows(
                slow.installed.as_deref(),
                table.as_ref().map(|t| t.processes.as_slice()),
            ),
            processes,
            front,
            tools,
            // No provider app registers a URL scheme on Linux.
            schemes: Fact::Known(Vec::new()),
            surfaces,
            last_seen_host: self.history.last_seen(session),
            original_host: self.history.original(session),
            // Windows Terminal profiles do not exist on Linux.
            terminal_profile: Fact::Known(None),
            default_terminal: slow.default_terminal,
            reservation: reservations::read(sys, &common.paths.reservation_dir, session),
            cwd_exists: common::cwd_exists(sys, found.cwd.as_deref()),
            cwd: found.cwd.clone().map_or(Fact::Unknown, Fact::Known),
            // Only the configured default homes are searched.
            environment: if found.found {
                Fact::Known(BTreeMap::new())
            } else {
                Fact::Unknown
            },
            capabilities: Vec::new(),
            provider: found.evidence,
        }
    }
}
