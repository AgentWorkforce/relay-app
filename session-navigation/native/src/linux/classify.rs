//! Executable classification on Linux, from the kernel's `/proc/<pid>/exe`
//! target (never argv[0]).
//!
//! UNVERIFIED install layouts (verify with `readlink /proc/<pid>/exe` for a
//! running agent of each kind):
//! - the Claude native installer runs `~/.local/share/claude/versions/<ver>`
//!   behind a `claude` link, so a file under `/claude/versions/` is Claude;
//! - the Codex npm package spawns `codex-<target triple>`;
//! - Grok installs `grok-<version>-<os>-<arch>` behind `grok`;
//! - an npm-installed Claude runs as `node …/cli.js`: its kernel executable
//!   is `node`, so it is `Other` (as on macOS) and cannot be stopped or
//!   located as an agent.
use session_navigation::evidence::Exe;

const SHELLS: &[&str] = &[
    "zsh", "bash", "sh", "dash", "fish", "ksh", "tcsh", "csh", "nu",
];

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Classify a resolved executable path.
pub fn classify_exe(path: &str) -> Exe {
    let name = base(path);
    if path.contains("/claude/versions/") {
        return Exe::ClaudeCli;
    }
    match name {
        "claude" => Exe::ClaudeCli,
        "codex" => Exe::CodexCli,
        n if n.starts_with("codex-") && n.contains("linux") => Exe::CodexCli,
        "opencode" | "opencode-cli" => Exe::OpenCodeCli,
        n if n == "grok" || n.starts_with("grok-") => Exe::GrokCli,
        "tmux" => Exe::Tmux,
        "ghostty" => Exe::Ghostty,
        "kitty" => Exe::Kitty,
        // The GUI, or the mux server whose panes `wezterm cli` lists.
        "wezterm-gui" | "wezterm-mux-server" => Exe::WezTerm,
        "alacritty" => Exe::Alacritty,
        // One server process owns every GNOME Terminal window.
        "gnome-terminal-server" => Exe::GnomeTerminal,
        "konsole" => Exe::Konsole,
        "xterm" => Exe::Xterm,
        "login" => Exe::Login,
        n if SHELLS.contains(&n) => Exe::Shell,
        _ => Exe::Other,
    }
}

/// Classify a process whose `exe` link could not be read (another user's
/// or a setuid process). Only the kernel `comm` is left, which the process
/// can rename, so it may only make it `Login`, `Shell` or `Other`: never an
/// agent CLI, terminal or tmux, which the adapter would act on.
pub fn classify_comm(comm: &str) -> Exe {
    match comm {
        "login" => Exe::Login,
        n if SHELLS.contains(&n) => Exe::Shell,
        _ => Exe::Other,
    }
}

pub fn is_agent_cli(exe: Exe) -> bool {
    matches!(
        exe,
        Exe::ClaudeCli | Exe::CodexCli | Exe::OpenCodeCli | Exe::GrokCli
    )
}
