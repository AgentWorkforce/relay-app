//! Executable classification on Windows, and inside a WSL distribution.
//!
//! The kernel's image path (`Win32_Process.ExecutablePath`, or the image
//! name when the path is unreadable; `/proc/<pid>/exe` in WSL, else the
//! task `comm`) decides, never argv[0]. Two exceptions read one more word,
//! the script a runtime runs: a Node-hosted Claude CLI (`node … cli.js`
//! from `@anthropic-ai/claude-code`), whose kernel image is `node`. The
//! Codex and OpenCode npm entry points are wrappers that start the native
//! binary as a child, so their `node` stays `Other` (the child is the CLI).
//! All path shapes here are UNVERIFIED on a Windows machine.
use session_navigation::evidence::Exe;

/// The last path component, for `\` and `/`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Lower case with `/` as separator, for path-shape tests.
fn normalized(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// The Claude CLI script a Node process may run.
fn node_claude(argv: &[String]) -> bool {
    argv.iter()
        .skip(1)
        .take(3)
        .any(|a| normalized(a).ends_with("/node_modules/@anthropic-ai/claude-code/cli.js"))
}

/// Whether a console host is a pseudo-console (ConPTY) host without a
/// window of its own (Windows Terminal, VS Code, Alacritty and WezTerm start
/// one per tab with `--headless`). Such a host is not a classic console.
fn headless_console(argv: &[String]) -> bool {
    argv.iter().any(|a| a.eq_ignore_ascii_case("--headless"))
}

const WINDOWS_SHELLS: &[&str] = &[
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "bash.exe",
    "sh.exe",
];
const POSIX_SHELLS: &[&str] = &["zsh", "bash", "sh", "dash", "fish", "ksh", "tcsh", "csh"];

/// A native Windows process, from its image path (or name) and argv.
pub fn classify_windows(image: &str, argv: &[String]) -> Exe {
    let name = file_name(image);
    let lower = name.to_ascii_lowercase();
    let path = normalized(image);
    match lower.as_str() {
        "claude.exe" => {
            // The desktop app's Squirrel or MSIX install (desktop-core
            // `provider_application`); its bundled CLI lives elsewhere.
            if path.contains("/anthropicclaude/")
                || path.contains("/windowsapps/anthropicpbc.claude_")
            {
                Exe::ClaudeDesktopApp
            } else {
                Exe::ClaudeCli
            }
        }
        "codex.exe" => {
            if path.contains("/openai.codex_")
                || path.contains("/codex/app/")
                || path.contains("/programs/codex/")
            {
                Exe::CodexDesktopApp
            } else {
                Exe::CodexCli
            }
        }
        // The Tauri desktop app is `OpenCode.exe`; the CLI `opencode.exe`.
        "opencode.exe" if name == "OpenCode.exe" => Exe::OpenCodeDesktopApp,
        "opencode.exe" | "opencode-cli.exe" => Exe::OpenCodeCli,
        "grok.exe" => Exe::GrokCli,
        n if n.starts_with("grok-") && n.ends_with(".exe") => Exe::GrokCli,
        "windowsterminal.exe" => Exe::WindowsTerminal,
        "conhost.exe" | "openconsole.exe" if headless_console(argv) => Exe::Other,
        "conhost.exe" | "openconsole.exe" => Exe::ConsoleHost,
        "wezterm-gui.exe" => Exe::WezTerm,
        "alacritty.exe" => Exe::Alacritty,
        "node.exe" if node_claude(argv) => Exe::ClaudeCli,
        _ if path.contains("/claude/versions/") => Exe::ClaudeCli,
        n if WINDOWS_SHELLS.contains(&n) => Exe::Shell,
        _ => Exe::Other,
    }
}

/// A process inside a WSL distribution, from `/proc/<pid>/exe` (or the
/// task `comm` when the link is unreadable) and argv.
pub fn classify_posix(image: &str, argv: &[String]) -> Exe {
    let name = file_name(image);
    let path = normalized(image);
    match name {
        "claude" => Exe::ClaudeCli,
        "codex" => Exe::CodexCli,
        "opencode" | "opencode-cli" => Exe::OpenCodeCli,
        n if n == "grok" || n.starts_with("grok-") => Exe::GrokCli,
        "tmux" | "tmux: server" | "tmux: client" => Exe::Tmux,
        "login" => Exe::Login,
        "node" if node_claude(argv) => Exe::ClaudeCli,
        // The native installer runs `~/.local/share/claude/versions/<v>`.
        _ if path.contains("/claude/versions/") => Exe::ClaudeCli,
        n if POSIX_SHELLS.contains(&n) => Exe::Shell,
        _ => Exe::Other,
    }
}

pub fn is_agent_cli(exe: Exe) -> bool {
    matches!(
        exe,
        Exe::ClaudeCli | Exe::CodexCli | Exe::OpenCodeCli | Exe::GrokCli
    )
}
