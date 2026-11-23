//! The colors and glyphs a row's upload status and a stage are drawn in.

use crate::models::{SessionUploadState, StageState};

/// The palette color a row's status reads in, as the Mac's.
pub(super) fn state_color(state: SessionUploadState) -> &'static str {
    match state {
        SessionUploadState::Uploaded => "secondary",
        SessionUploadState::Uploading
        | SessionUploadState::Preparing
        | SessionUploadState::Waiting
        | SessionUploadState::Queued => "accent",
        SessionUploadState::Paused
        | SessionUploadState::Blocked
        | SessionUploadState::Stopped
        | SessionUploadState::Unknown => "orange",
        SessionUploadState::Discovering
        | SessionUploadState::Local
        | SessionUploadState::Checking => "secondary",
    }
}

pub(super) fn stage_glyph(state: StageState) -> (&'static str, &'static str) {
    match state {
        StageState::Waiting => ("content-loading-symbolic", "secondary"),
        StageState::Active => ("view-refresh-symbolic", "accent"),
        StageState::Complete => ("emblem-ok-symbolic", "ok"),
        StageState::Paused => ("media-playback-pause-symbolic", "warn"),
        StageState::Blocked => ("dialog-warning-symbolic", "warn"),
    }
}
