//! World variables: one toggle per UNVERIFIED behaviour in the facts files.
//! The simulator runs with either answer and the invariants must hold under
//! both.
use serde::{Deserialize, Serialize};

macro_rules! world_vars {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub struct WorldVars {
            $($(#[$doc])* pub $name: bool,)*
        }

        impl WorldVars {
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name)),*];

            pub fn from_bits(bits: u32) -> Self {
                let mut vars = Self::default();
                let mut index = 0u32;
                $(
                    vars.$name = bits & (1 << index) != 0;
                    index += 1;
                )*
                let _ = index;
                vars
            }

            pub fn bits(&self) -> u32 {
                let mut bits = 0u32;
                let mut index = 0u32;
                $(
                    if self.$name {
                        bits |= 1 << index;
                    }
                    index += 1;
                )*
                let _ = index;
                bits
            }
        }
    };
}

world_vars! {
    /// Claude facts B / summary 1: `claude://resume` unarchives at runtime.
    /// VERIFIED true for Claude.app 5.1231.2 (native N5); this answer only
    /// applies to another app version, where the planner never uses the link.
    claude_resume_link_unarchives,
    /// Claude facts B / summary 2: `resume` with an id only in prior ids imports
    /// a duplicate desktop session (else it selects the owner).
    claude_resume_prior_imports_duplicate,
    /// Claude summary 9: a pre-warmed spare has a registry file before it is
    /// claimed (naming a session it has not claimed).
    claude_spare_registry_before_claim,
    /// Claude summary 5: the registry `sessionId` is rewritten after /clear.
    claude_registry_rewritten_on_clear,
    /// Claude facts C: selecting a desktop session restarts its process at
    /// once (N5 saw none within 14 s for `resume`; `continue` untested). No
    /// longer decides whether a link can be verified: `lastFocusedAt` does.
    claude_continue_starts_process,
    /// Codex facts 17: a TUI holds the writer lock itself. VERIFIED false for
    /// the 0.167.0 daemon (native N8); this answer applies only to TUIs
    /// started under another CLI version (`--no-daemon` is not modelled).
    codex_tui_holds_lock,
    /// Codex facts 26: a second writer on a thread is refused. Moot between
    /// clients of the daemon, which share its one writer (VERIFIED, N10);
    /// still open for the daemon next to the desktop's app-server or a TUI
    /// with its own lock.
    codex_second_writer_refused,
    /// Codex facts 6: `codex resume <archived id>` works without unarchiving.
    codex_resume_archived_works,
    /// Codex facts 6/9: the desktop link on an archived thread shows that
    /// thread (else the home screen).
    codex_link_archived_shows_thread,
    /// Codex facts 13: IPC `thread-owner-discovery` works on build 25894.
    codex_ipc_owner_discovery_works,
    /// OpenCode facts A: the background service outlives the desktop app.
    opencode_server_survives_quit,
    /// OpenCode facts F: the TUI opens an archived or child id given to `-s`
    /// (else it errors).
    opencode_tui_opens_archived_or_child,
    /// Codex facts 13, OpenCode facts D: which conversation a desktop app
    /// shows can be observed. Never for Claude, whose view is not readable;
    /// its record's `lastFocusedAt` is the signal instead (native N1).
    desktop_view_readable,
    /// Codex: a TUI can start or switch to another thread after launch, so
    /// its argv no longer names what it shows (UNVERIFIED either way; Claude
    /// facts D.3 shows the analogous behaviour for Claude).
    codex_tui_switches_thread,
    /// facts-grok 4: `grok --resume <id>` resumes the session (desktop-core
    /// runs it; relayhistory writes `grok resume <id>`). Else it errors out.
    grok_resume_flag_works,
    /// facts-grok 4: `grok --resume` with an id it cannot find silently
    /// starts a new session under that id (as OpenCode `-s` does).
    grok_resume_missing_creates,
    /// facts-grok 2: sessions are stored per working directory, so a resume
    /// from another directory does not find the session.
    grok_resume_needs_cwd,
    /// facts-grok 3: the TUI rewrites its `active_sessions.json` record when
    /// it moves to another session (`/new`, `/resume`).
    grok_registry_follows_session,
    /// facts-grok 3: a TUI started with `--resume` records itself in
    /// `active_sessions.json` like a fresh one.
    grok_resumed_tui_registers,
    /// facts-grok 6: `grok --resume <subagent id>` opens the child session.
    grok_subagent_resumable,
    /// Kitty `focus-window` / `wezterm cli activate-pane` also raise the OS
    /// window (X11, macOS). The old GTK code only claims `Dispatched`.
    pane_api_focus_raises,
    /// Wayland: the same raise the compositor may refuse (focus-stealing
    /// prevention); no generic raise is assumed either way.
    wayland_pane_focus_raises,
    /// X11: the window manager honours `xdotool windowactivate`
    /// (`_NET_ACTIVE_WINDOW`).
    x11_activation_honored,
    /// Windows: `SetForegroundWindow` from Relay is allowed (the foreground
    /// lock may refuse it).
    windows_foreground_allowed,
    /// Windows: a classic console window reports its first client as owner
    /// (not `conhost.exe`).
    console_window_owner_is_client,
    /// Windows Terminal `wt new-tab` joins an existing window (its
    /// `windowingBehavior`) instead of opening a new one.
    wt_new_tab_joins_window,
}

impl WorldVars {
    pub const COUNT: u32 = 26;
    /// The variables of the first round (bits 0..14).
    pub const LEGACY: u32 = 14;
    /// Grok's (bits 14..20).
    pub const GROK: std::ops::Range<u32> = 14..20;
    /// Terminal hosts' (bits 20..26).
    pub const HOSTS: std::ops::Range<u32> = 20..26;
}
