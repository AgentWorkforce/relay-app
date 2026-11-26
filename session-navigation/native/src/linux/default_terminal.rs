//! The operating system's default terminal on Linux, from the first source
//! that says anything, in this order:
//!
//! 1. `xdg-terminals.list` (the xdg-terminal-exec proposal): in each XDG
//!    config directory (`$XDG_CONFIG_HOME`, then `$XDG_CONFIG_DIRS`), then
//!    each data directory's `xdg-terminal-exec/`, the files
//!    `<desktop>-xdg-terminals.list` for each `$XDG_CURRENT_DESKTOP` entry
//!    (lower-cased), then `xdg-terminals.list`. One desktop-file id per line
//!    (`kitty.desktop`, optionally `:action`), `#` comments, `-id` excluded,
//!    `+id` the same as `id`; the first id whose desktop file exists wins.
//! 2. The Debian `x-terminal-emulator` alternative (the target of
//!    `/etc/alternatives/x-terminal-emulator`).
//! 3. Under GNOME only, `gsettings get
//!    org.gnome.desktop.default-applications.terminal exec`.
//!
//! `$TERMINAL` is not read: it is Relay's own environment, not the person's
//! desktop session, and no desktop sets it.
//!
//! A terminal this crate does not model is `Known(None)` (the person chose
//! something else, which is not swapped for another); every source absent is
//! `Known(None)`; a source present but unreadable is `Unknown`.
//!
//! UNVERIFIED: none of this ran on a Linux machine. The list locations and
//! syntax follow the xdg-terminal-exec README; verify with
//! `ls ~/.config/*xdg-terminals.list /etc/xdg/*xdg-terminals.list
//! /usr/share/xdg-terminal-exec/` and `xdg-terminal-exec --print-id`
//! (newer versions). The alternative: `readlink
//! /etc/alternatives/x-terminal-emulator` (Debian/Ubuntu,
//! `update-alternatives --display x-terminal-emulator`). GNOME: `gsettings
//! get org.gnome.desktop.default-applications.terminal exec` (deprecated
//! upstream; its schema default is read on any machine with the schema, so
//! it counts only when `XDG_CURRENT_DESKTOP` names GNOME).
use session_navigation::evidence::Fact;
use session_navigation::model::{AppIdentifier, TerminalKind};

use super::apps::{desktop_file, launcher, linux_terminals, which};
use super::config::LinuxConfig;
use crate::system::{join, FsError, Run, System};

pub const ALTERNATIVE: &str = "/etc/alternatives/x-terminal-emulator";
pub const GSETTINGS_ARGS: [&str; 3] = [
    "get",
    "org.gnome.desktop.default-applications.terminal",
    "exec",
];
const LIST_CAP: usize = 64 * 1024;

/// What one source says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Said {
    Nothing,
    Terminal(Option<TerminalKind>),
    Unreadable,
}

/// The default terminal (see the module docs for the order).
pub fn read<S: System>(sys: &S, cfg: &LinuxConfig) -> Fact<Option<TerminalKind>> {
    let mut said = lists(sys, cfg);
    if said == Said::Nothing {
        said = alternative(sys);
    }
    if said == Said::Nothing {
        said = gnome(sys, cfg);
    }
    match said {
        Said::Nothing => Fact::Known(None),
        Said::Terminal(terminal) => Fact::Known(terminal),
        Said::Unreadable => Fact::Unknown,
    }
}

/// The modelled terminal with this desktop-file id (without `.desktop`).
pub fn by_desktop_id(id: &str) -> Option<TerminalKind> {
    linux_terminals().into_iter().find(|t| {
        matches!(
            t.app().identifier(super::apps::LINUX),
            Some(AppIdentifier::Linux { desktop_file, .. }) if desktop_file == id
        )
    })
}

/// The modelled terminal launched by this executable (a path or a name;
/// Debian's `gnome-terminal.wrapper` is GNOME Terminal).
pub fn by_executable(program: &str) -> Option<TerminalKind> {
    let name = program.rsplit('/').next().unwrap_or(program);
    let name = name.strip_suffix(".wrapper").unwrap_or(name);
    linux_terminals()
        .into_iter()
        .find(|t| launcher(*t) == Some(name))
}

/// The `xdg-terminals.list` files, highest precedence first.
pub fn list_paths(cfg: &LinuxConfig) -> Vec<String> {
    let desktops: Vec<String> = cfg
        .session_var("XDG_CURRENT_DESKTOP")
        .map(|d| {
            d.split(':')
                .filter(|x| !x.is_empty() && !x.contains('/'))
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default();
    let dirs = cfg
        .config_dirs
        .iter()
        .cloned()
        .chain(cfg.data_dirs.iter().map(|d| join(d, "xdg-terminal-exec")));
    let mut out = Vec::new();
    for dir in dirs {
        for desktop in &desktops {
            out.push(join(&dir, &format!("{desktop}-xdg-terminals.list")));
        }
        out.push(join(&dir, "xdg-terminals.list"));
    }
    out
}

/// The desktop-file ids a list names, in order (exclusions dropped).
pub fn list_entries(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
        .map(|l| l.strip_prefix('+').unwrap_or(l))
        .map(|l| l.split(':').next().unwrap_or(l))
        .filter_map(|l| l.strip_suffix(".desktop"))
        .filter(|id| !id.is_empty() && !id.contains('/') && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .collect()
}

fn lists<S: System>(sys: &S, cfg: &LinuxConfig) -> Said {
    for path in list_paths(cfg) {
        let bytes = match sys.read_file(&path, LIST_CAP) {
            Ok(bytes) => bytes,
            Err(FsError::NotFound) => continue,
            Err(_) => return Said::Unreadable,
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Said::Unreadable;
        };
        for id in list_entries(text) {
            match desktop_file(sys, cfg, &id) {
                Some(true) => return Said::Terminal(by_desktop_id(&id)),
                Some(false) => {}
                None => return Said::Unreadable,
            }
        }
    }
    Said::Nothing
}

fn alternative<S: System>(sys: &S) -> Said {
    match sys.read_link(ALTERNATIVE) {
        Ok(target) => Said::Terminal(by_executable(target.trim())),
        Err(FsError::NotFound) => Said::Nothing,
        Err(_) => Said::Unreadable,
    }
}

fn gnome<S: System>(sys: &S, cfg: &LinuxConfig) -> Said {
    let gnome = cfg
        .session_var("XDG_CURRENT_DESKTOP")
        .is_some_and(|d| d.split(':').any(|x| x.eq_ignore_ascii_case("GNOME")));
    if !gnome {
        return Said::Nothing;
    }
    let Some(program) = which(sys, &cfg.program_path, "gsettings") else {
        return Said::Nothing;
    };
    let mut run = Run::new(&program, &GSETTINGS_ARGS).timeout(cfg.timeouts().probe);
    run.env.clone_from(&cfg.session_env);
    let Ok(output) = sys.run(&run) else {
        return Said::Unreadable;
    };
    if !output.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // An uninstalled schema or key says nothing; anything else (no
        // session bus, …) is unreadable.
        return if stderr.contains("No such schema") || stderr.contains("No such key") {
            Said::Nothing
        } else {
            Said::Unreadable
        };
    }
    gsettings_exec(&output.stdout).map_or(Said::Nothing, |p| Said::Terminal(by_executable(&p)))
}

/// `'gnome-terminal'` → the program (first word); `None` for an empty value.
pub fn gsettings_exec(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?.trim();
    let inner = text
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .unwrap_or(text);
    inner.split_whitespace().next().map(str::to_owned)
}
