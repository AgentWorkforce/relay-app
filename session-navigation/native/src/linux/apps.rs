//! Installed and running applications on Linux: the terminals the core
//! knows (no provider desktop app is modelled on Linux), found by their
//! desktop-file id in the XDG data directories or by their launcher on the
//! program path, and running when a process of their executable is in the
//! table.
//!
//! UNVERIFIED: the desktop-file ids are the core's (`App::identifier`);
//! some distributions rename them (Debian ships `debian-xterm.desktop`),
//! which the launcher fallback covers. Verify with
//! `ls /usr/share/applications | grep -i -e kitty -e wezterm -e alacritty -e terminal -e konsole -e xterm -e ghostty`.
use session_navigation::evidence::{AppInfo, Exe, Fact, ProcessInfo};
use session_navigation::model::{App, AppIdentifier, DisplayServer, Platform, TerminalKind};

use super::config::LinuxConfig;
use crate::system::{join, FsError, System};

/// Any Linux platform value: `App::identifier` does not depend on the
/// display server.
pub const LINUX: Platform = Platform::Linux {
    session: Fact::Known(DisplayServer::X11),
};

/// The apps that exist on Linux, in `App::ALL` order.
pub fn linux_apps() -> Vec<App> {
    App::ALL
        .into_iter()
        .filter(|a| a.identifier(LINUX).is_some())
        .collect()
}

/// The terminals that exist on Linux, in `TerminalKind::ALL` order.
pub fn linux_terminals() -> Vec<TerminalKind> {
    TerminalKind::ALL
        .into_iter()
        .filter(|t| t.exists_on(LINUX))
        .collect()
}

/// The program a terminal is launched with (its own CLI, which for GNOME
/// Terminal is not the server that owns the windows).
pub fn launcher(terminal: TerminalKind) -> Option<&'static str> {
    Some(match terminal {
        TerminalKind::Ghostty => "ghostty",
        TerminalKind::Kitty => "kitty",
        TerminalKind::WezTerm => "wezterm",
        TerminalKind::Alacritty => "alacritty",
        TerminalKind::GnomeTerminal => "gnome-terminal",
        TerminalKind::Konsole => "konsole",
        TerminalKind::Xterm => "xterm",
        _ => return None,
    })
}

/// The executable class of a terminal's own processes.
pub fn terminal_exe(terminal: TerminalKind) -> Option<Exe> {
    Some(match terminal {
        TerminalKind::Ghostty => Exe::Ghostty,
        TerminalKind::Kitty => Exe::Kitty,
        TerminalKind::WezTerm => Exe::WezTerm,
        TerminalKind::Alacritty => Exe::Alacritty,
        TerminalKind::GnomeTerminal => Exe::GnomeTerminal,
        TerminalKind::Konsole => Exe::Konsole,
        TerminalKind::Xterm => Exe::Xterm,
        _ => return None,
    })
}

/// The app an executable class belongs to.
pub fn app_of(exe: Exe) -> Option<App> {
    linux_terminals()
        .into_iter()
        .find(|t| terminal_exe(*t) == Some(exe))
        .map(TerminalKind::app)
}

/// The first executable file `name` on `dirs`. Unreadable entries count
/// as absent.
pub fn which<S: System>(sys: &S, dirs: &[String], name: &str) -> Option<String> {
    dirs.iter().find_map(|dir| {
        let path = join(dir, name);
        sys.metadata(&path)
            .ok()
            .filter(|m| m.is_file && m.executable)
            .map(|_| path)
    })
}

/// Whether `<dir>/applications/<id>.desktop` exists in any data dir.
/// `None` when a directory could not be read.
pub fn desktop_file<S: System>(sys: &S, cfg: &LinuxConfig, id: &str) -> Option<bool> {
    for dir in &cfg.data_dirs {
        let path = join(&join(dir, "applications"), &format!("{id}.desktop"));
        match sys.metadata(&path) {
            Ok(meta) if meta.is_file => return Some(true),
            Ok(_) | Err(FsError::NotFound) => {}
            Err(_) => return None,
        }
    }
    Some(false)
}

/// Whether each Linux app is installed (a desktop file, or its launcher on
/// the program path), in [`linux_apps`] order. `None` when a data
/// directory is unreadable. Slow-changing: the observer reuses it briefly.
pub fn installed<S: System>(sys: &S, cfg: &LinuxConfig) -> Option<Vec<(App, bool)>> {
    let mut rows = Vec::new();
    for app in linux_apps() {
        let Some(AppIdentifier::Linux {
            desktop_file: id, ..
        }) = app.identifier(LINUX)
        else {
            continue;
        };
        let by_file = desktop_file(sys, cfg, id)?;
        let by_launcher = app
            .terminal()
            .and_then(launcher)
            .is_some_and(|name| which(sys, &cfg.program_path, name).is_some());
        rows.push((app, by_file || by_launcher));
    }
    Some(rows)
}

/// The app rows from [`installed`] and a fresh process table (`running`).
pub fn rows(
    installed: Option<&[(App, bool)]>,
    table: Option<&[ProcessInfo]>,
) -> Fact<Vec<AppInfo>> {
    let (Some(installed), Some(table)) = (installed, table) else {
        return Fact::Unknown;
    };
    let rows = installed
        .iter()
        .map(|(app, installed)| {
            let exe = app.terminal().and_then(terminal_exe);
            AppInfo {
                app: *app,
                installed: *installed,
                // A desktop file's `Version` is the spec's, not the app's.
                version: None,
                running: exe.is_some_and(|e| table.iter().any(|p| p.exe == e)),
            }
        })
        .collect();
    Fact::Known(rows)
}

/// One row per Linux app. `Unknown` without a process table (`running`
/// cannot be said) or when a data directory is unreadable.
pub fn apps<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    table: Option<&[ProcessInfo]>,
) -> Fact<Vec<AppInfo>> {
    let Some(table) = table else {
        return Fact::Unknown;
    };
    rows(installed(sys, cfg).as_deref(), Some(table))
}
