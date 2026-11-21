//! Which Settings row a `settings.set` failure is reported under.

/// Which row reports a setting's failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingRow {
    DirectDelivery,
    GrokLeader,
    OpenCodePlugin,
    OpenClaw,
    TrustedSenders,
    Hermes,
    DevinDelivery,
    CursorDelivery,
    ClaudeFolders,
    ClaudeSessionHook,
    QuestionEscalation,
    CodexHook,
    /// The pane's own banner.
    Pane,
}
