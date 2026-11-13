//! Settings navigation shared with macOS.

/// Settings, one pane at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    General,
    Tools,
    Opening,
    About,
    Relay,
    Sharing,
    Account,
    Diagnostics,
}

impl Pane {
    /// The sidebar's groups, in order.
    pub const GROUPS: [(&'static str, &'static [Pane]); 3] = [
        ("Preferences", &[Pane::General]),
        ("Sessions", &[Pane::Relay]),
        (
            "Account & app",
            &[Pane::Account, Pane::About, Pane::Diagnostics],
        ),
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Tools => "Providers",
            Pane::Opening => "Opening sessions",
            Pane::About => "Updates",
            Pane::Diagnostics => "Diagnostics",
            Pane::Relay => "Messages",
            Pane::Sharing => "Session sharing",
            Pane::Account => "Account",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Pane::General => "Appearance, startup, sharing, privacy and session history.",
            // Each provider page names itself; a pane subtitle would repeat it.
            Pane::Tools => "",
            Pane::Opening => "Choose where local sessions open.",
            Pane::About => "Keep Agent Relay up to date.",
            Pane::Diagnostics => "Troubleshoot connections and collect a diagnostic report.",
            Pane::Relay => "The workspace your agents join and how messages reach them.",
            Pane::Sharing => "Which coding sessions Agent Relay shares with your workspace.",
            Pane::Account => "Your profile and connected workspace.",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Pane::General => "emblem-system-symbolic",
            Pane::Tools => "application-x-addon-symbolic",
            Pane::Opening => "window-new-symbolic",
            Pane::About => "software-update-available-symbolic",
            Pane::Diagnostics => "help-about-symbolic",
            Pane::Relay => "network-wireless-symbolic",
            Pane::Sharing => "send-to-symbolic",
            Pane::Account => "avatar-default-symbolic",
        }
    }
}
