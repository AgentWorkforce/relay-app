//! The sharing modes as Settings offers and words them.

use crate::contract::account::SharingMode;

/// The order Settings offers the sharing modes in.
pub const SHARING_DISPLAY_ORDER: [SharingMode; 3] =
    [SharingMode::Selected, SharingMode::New, SharingMode::All];

/// How Settings words a sharing mode.
pub trait SharingModeText {
    fn title(self) -> &'static str;
    fn explanation(self) -> &'static str;
    /// The question asked before switching to it.
    fn confirmation_title(self) -> &'static str;
    /// The button that confirms the switch.
    fn confirmation_action(self) -> &'static str;
}

impl SharingModeText for SharingMode {
    fn title(self) -> &'static str {
        match self {
            SharingMode::All => "Automatically upload all sessions, past and future",
            SharingMode::New => "Automatically upload new sessions",
            SharingMode::Selected => "Only sessions I choose",
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            SharingMode::All => "Existing and future local coding sessions upload automatically. Sessions you turned off stay off until you enable Upload session again.",
            SharingMode::New => "Upload new sessions automatically. Existing sessions stay local unless you choose to share them.",
            SharingMode::Selected => "New sessions stay local until you turn on Upload session. Once enabled, those sessions keep uploading changes until you turn it off.",
        }
    }

    fn confirmation_title(self) -> &'static str {
        match self {
            SharingMode::Selected => "Upload only sessions you choose?",
            SharingMode::New => "Automatically upload new sessions?",
            SharingMode::All => "Automatically upload all sessions?",
        }
    }

    fn confirmation_action(self) -> &'static str {
        match self {
            SharingMode::Selected => "Use selected sessions only",
            SharingMode::New => "Enable automatic uploads",
            SharingMode::All => "Upload all past and future sessions",
        }
    }
}
