//! [`Observe`] for a real Mac, assembled from [`System`] calls and pure
//! parsers. Every failure, timeout, oversize or parse error becomes
//! `Fact::Unknown` (never a default, never a panic).
use std::collections::BTreeMap;
use std::time::Duration;

use session_navigation::evidence::{AppInfo, Fact, Front, SchemeHandler, Snapshot, ToolInfo};
use session_navigation::hosts::Surfaces;
use session_navigation::model::{
    App, BootId, Platform, Provider, Runtime, SessionRef, TerminalKind, Tool,
};
use session_navigation::ports::Observe;

use super::config::MacConfig;
use super::hosts::{self, HostView};
use super::kernel::KernelTable;
use super::machine::{self, FrontRead};
use super::parse::AppsReport;
use super::registry::{self, RegistryCache};
use crate::common::par::{self, Reuse};
use crate::common::tools::ToolResolver;
use crate::common::{self, grok, reservations, HostHistory, NoHistory};
use crate::system::System;

/// The host surfaces of a Mac where nothing beyond the defaults was probed:
/// no Kitty, WezTerm or tmux running, no Automation refusal seen. Each
/// observation reports its own ([`super::hosts::observe`]).
pub fn surfaces() -> Surfaces {
    Surfaces {
        terminals: Fact::Known(hosts::support(hosts::Probed::default())),
        panes: Fact::Known(Vec::new()),
        tmux: Fact::Known(Vec::new()),
        windows: Fact::Known(Vec::new()),
    }
}

pub struct MacObserver<S, H = NoHistory> {
    sys: S,
    cfg: MacConfig,
    history: H,
    tools: ToolResolver,
    /// Tool paths and versions, reused for a short window.
    tool_reuse: Reuse<Vec<ToolInfo>>,
    /// The app registry on the fast path (kernel reads).
    registry: RegistryCache,
    /// Never run terminal AppleScript (see [`MacObserver::set_passive`]).
    passive: bool,
}

impl<S: System> MacObserver<S, NoHistory> {
    pub fn new(sys: S, cfg: MacConfig) -> Self {
        Self::with_history(sys, cfg, NoHistory)
    }
}

impl<S: System, H: HostHistory> MacObserver<S, H> {
    pub fn with_history(sys: S, cfg: MacConfig, history: H) -> Self {
        Self {
            sys,
            cfg,
            history,
            tools: ToolResolver::new(),
            tool_reuse: Reuse::default(),
            registry: RegistryCache::new(),
            passive: false,
        }
    }

    /// A passive observer never scripts a terminal: the front-pane read of
    /// Ghostty, Terminal and iTerm2 (`osascript -e`, which can raise a macOS
    /// Automation prompt the first time) is skipped and `front.pane_tty` is
    /// `Unknown`. For looks that must stay silent, such as rendering a
    /// button. The JXA app registry (`osascript -l JavaScript`, no
    /// Automation target) still runs.
    pub fn set_passive(&mut self, passive: bool) {
        self.passive = passive;
    }

    pub fn config(&self) -> &MacConfig {
        &self.cfg
    }

    /// How long slow-changing facts (installed apps, versions, schemes, the
    /// default terminal, tool paths and versions) are reused between looks;
    /// 2 s unless set, zero for never.
    pub fn set_reuse(&mut self, window: Duration) {
        self.tool_reuse.set_window(window);
        self.registry = RegistryCache::new();
        self.registry.reuse = Some(window);
    }
}

/// The frontmost app, and the front-pane read of a scripted terminal.
fn front_app<S: System>(
    sys: &S,
    cfg: &MacConfig,
    report: Option<&AppsReport>,
    passive: bool,
) -> (Fact<Option<App>>, Option<FrontRead>) {
    let Some(report) = report else {
        return (Fact::Unknown, None);
    };
    let app = report.front.as_deref().and_then(App::from_bundle_id);
    let read = app
        .filter(|_| !passive)
        .and_then(App::terminal)
        .filter(|t| machine::front_script(*t).is_some())
        .map(|t| machine::front_pane_read(sys, cfg, t));
    (Fact::Known(app), read)
}

/// Boot, the process table and agent cwds: kernel reads when the system
/// has them, else `sysctl`, `ps` and `lsof`.
fn machine_look<S: System>(sys: &S, cfg: &MacConfig) -> (Option<u64>, Option<KernelTable>) {
    let boot = machine::boot(sys, cfg);
    let table = boot.and_then(|sec| machine::full_table(sys, cfg, BootId(sec)));
    let table = table.map(|mut t| {
        // A table from `ps` (the kernel could not list it) gets its cwds
        // from `lsof` too: the whole tool path.
        match sys.kernel().filter(|_| t.from_kernel) {
            Some(kernel) => super::kernel::fill_cwds(kernel, &mut t.processes),
            None => machine::lsof_cwds_into(sys, cfg, &mut t.processes),
        }
        t
    });
    (boot, table)
}

/// The registry report as this look sees it. Tool path: the JXA query,
/// every look. Fast path: the reused registry with a fresh front app and
/// the running apps told from the kernel table (a fresh query when they
/// cannot be told).
fn registry_report<S: System>(
    sys: &S,
    cfg: &MacConfig,
    look: registry::Look,
    table: Option<&KernelTable>,
) -> Option<AppsReport> {
    let mut report = look.report?;
    let running =
        table.and_then(|t| registry::running(&look.executables, &report, &t.executables, cfg.uid));
    match running {
        Some(running) => report.running = running,
        None if look.fresh => {}
        None => return machine::apps(sys, cfg),
    }
    Some(report)
}

fn front(app: &Fact<Option<App>>, read: Option<FrontRead>, hosts: &HostView) -> Front {
    let Fact::Known(app) = app else {
        return Front {
            app: Fact::Unknown,
            pane_tty: Fact::Unknown,
            pane: Fact::Unknown,
            window: Fact::Known(None),
            view: Fact::Unknown,
        };
    };
    let (pane_tty, pane, view) = match app.and_then(App::terminal) {
        Some(TerminalKind::Kitty) => (
            hosts.kitty.front_tty.clone(),
            hosts.kitty.front.clone(),
            Fact::Known(None),
        ),
        Some(TerminalKind::WezTerm) => (
            hosts.wezterm.front_tty.clone(),
            hosts.wezterm.front.clone(),
            Fact::Known(None),
        ),
        // A scripted terminal (its read), or Alacritty (unreadable).
        Some(_) => (
            read.map_or(Fact::Unknown, |r| r.tty),
            Fact::Known(None),
            Fact::Known(None),
        ),
        // What a desktop app shows is not observable from outside.
        None if app.is_some() => (Fact::Known(None), Fact::Known(None), Fact::Unknown),
        None => (Fact::Known(None), Fact::Known(None), Fact::Known(None)),
    };
    Front {
        app: Fact::Known(*app),
        pane_tty,
        pane,
        // No window tracking on macOS.
        window: Fact::Known(None),
        view,
    }
}

fn apps(report: &AppsReport) -> Vec<AppInfo> {
    machine::mac_apps()
        .into_iter()
        .map(|app| {
            let entry = report.apps.get(app.bundle_id()).cloned().flatten();
            AppInfo {
                app,
                installed: entry.is_some(),
                version: entry.and_then(|e| e.version),
                running: report.running.iter().any(|b| b == app.bundle_id()),
            }
        })
        .collect()
}

/// The default terminal: the `public.unix-executable` handler mapped
/// through the bundle-id table. A handler this crate does not model is
/// `Known(None)`; an unreadable one (or no report) `Unknown`.
fn default_terminal(report: Option<&AppsReport>) -> Fact<Option<TerminalKind>> {
    match report.and_then(|r| r.terminal.as_ref()) {
        Some(handler) => Fact::Known(
            handler
                .bundle
                .as_deref()
                .and_then(App::from_bundle_id)
                .and_then(App::terminal),
        ),
        None => Fact::Unknown,
    }
}

fn schemes(report: &AppsReport) -> Vec<SchemeHandler> {
    report
        .schemes
        .iter()
        .map(|(scheme, bundle)| SchemeHandler {
            scheme: scheme.clone(),
            bundle: bundle.clone(),
        })
        .collect()
}

impl<S: System + Sync, H: HostHistory> Observe for MacObserver<S, H> {
    fn observe(&mut self, session: &SessionRef) -> Snapshot {
        let observed_at = self.sys.now();
        let (sys, cfg) = (&self.sys, &self.cfg);
        let fast = sys.kernel().is_some();
        let passive = self.passive;
        let (resolver, tool_reuse, cache) =
            (&mut self.tools, &mut self.tool_reuse, &mut self.registry);
        // Independent reads at once: the process table, the app registry
        // (a failed query makes apps, schemes and the front app `Unknown`;
        // the table stands on its own) and the tools.
        let common = &cfg.common;
        // Only Claude's evidence needs the table (its registry names live
        // pids); the other providers' stores are read alongside it.
        let early = session.provider != Provider::Claude;
        let (((boot_time, table), look, tools), early_found) = par::join(
            sys,
            || {
                par::join3(
                    sys,
                    || machine_look(sys, cfg),
                    || {
                        if fast {
                            registry::look(sys, cfg, cache)
                        } else {
                            registry::Look {
                                report: machine::apps(sys, cfg),
                                executables: BTreeMap::new(),
                                fresh: true,
                            }
                        }
                    },
                    || tool_reuse.get_or_keep(|| resolver.all_at_once(sys, &cfg.common), |_| true),
                )
            },
            || early.then(|| common::observe_provider(sys, common, session, None, &Fact::Unknown)),
        );
        let report = registry_report(sys, cfg, look, table.as_ref());
        let processes = table.map_or(Fact::Unknown, |t| Fact::Known(t.processes));
        let tmux = tools
            .iter()
            .find(|t| t.tool == Tool::Tmux)
            .and_then(|t| t.path.clone());
        // Then what needs the table: Claude's evidence, the front pane and
        // the terminal hosts.
        let (found, (front_app, read, hosts)) = par::join(
            sys,
            || match early_found {
                Some(found) => found,
                None => common::observe_provider(sys, common, session, boot_time, &processes),
            },
            || {
                let (front_app, read) = front_app(sys, cfg, report.as_ref(), passive);
                let denied = match (&front_app, &read) {
                    (Fact::Known(Some(app)), Some(r)) if r.denied => app.terminal(),
                    _ => None,
                };
                let hosts = hosts::observe(
                    sys,
                    cfg,
                    processes.known().map(Vec::as_slice),
                    report.as_ref(),
                    tmux.as_deref(),
                    denied,
                );
                (front_app, read, hosts)
            },
        );
        let environment = match session.provider {
            Provider::Grok => {
                grok::environment(&common.paths.grok_home, &grok::default_home(&cfg.home))
            }
            _ => BTreeMap::new(),
        };
        Snapshot {
            session: session.clone(),
            observed_at,
            platform: Platform::MacOs,
            runtime: Fact::Known(Runtime::Native),
            boot: boot_time.map_or(Fact::Unknown, |s| Fact::Known(BootId(s))),
            processes,
            front: front(&front_app, read, &hosts),
            apps: report
                .as_ref()
                .map_or(Fact::Unknown, |r| Fact::Known(apps(r))),
            tools,
            schemes: report
                .as_ref()
                .map_or(Fact::Unknown, |r| Fact::Known(schemes(r))),
            surfaces: hosts.surfaces,
            last_seen_host: self.history.last_seen(session),
            original_host: self.history.original(session),
            // Windows Terminal profiles do not exist on macOS.
            terminal_profile: Fact::Known(None),
            default_terminal: default_terminal(report.as_ref()),
            reservation: reservations::read(sys, &common.paths.reservation_dir, session),
            cwd_exists: common::cwd_exists(sys, found.cwd.as_deref()),
            cwd: found.cwd.clone().map_or(Fact::Unknown, Fact::Known),
            // Only the configured default homes are searched, so a session
            // found at all was found there.
            environment: if found.found {
                Fact::Known(environment)
            } else {
                Fact::Unknown
            },
            capabilities: Vec::new(),
            provider: found.evidence,
        }
    }
}
