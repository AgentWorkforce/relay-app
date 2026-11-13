//! Saving feedback for the Receive messages switch.
use super::*;
use crate::ui::state::State;

pub(super) fn build(parent: &gtk::Box) -> (gtk::Label, gtk::Spinner) {
    let receive_status = sidebar_note("Offline");
    let receive_progress = gtk::Spinner::new();
    receive_progress.set_visible(false);
    let receive_feedback = hbox(6, &[]);
    receive_feedback.append(&receive_progress);
    receive_feedback.append(&receive_status);
    parent.append(&receive_feedback);
    (receive_status, receive_progress)
}

pub(super) fn render(page: &TranscriptPage, state: &State, row: &LocalSessionRow) {
    let pending = row
        .relay
        .as_ref()
        .is_some_and(|session| state.busy_targets.contains(&session.target.key()));
    page.receive_progress.set_visible(pending);
    page.receive_progress.set_spinning(pending);
    page.receive.set_cursor_from_name(Some(if pending {
        "progress"
    } else if page.receive.is_sensitive() {
        "pointer"
    } else {
        "default"
    }));
}

pub(super) fn sidebar_note(value: &str) -> gtk::Label {
    let label = text(value, &["caption"]).wrapped();
    label.set_max_width_chars(28);
    label
}
