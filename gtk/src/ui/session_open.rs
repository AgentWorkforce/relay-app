//! The Open button's look. What it says is the engine's answer
//! (`models::SessionOpens`, drawn by `spawn_join/location.rs`); it never
//! decides whether a session may be launched.
use super::widgets::{self, hbox};
use gtk::prelude::*;
use gtk4 as gtk;

pub(super) fn back_to_sessions_button() -> gtk::Button {
    let back = gtk::Button::with_label("‹ Back to sessions");
    widgets::action_cursor(&back);
    back.set_tooltip_text(Some("Back to sessions"));
    back.set_halign(gtk::Align::Start);
    back.add_css_class("flat");
    back.set_valign(gtk::Align::Center);
    back
}

/// Keep the selected session's artwork when an answer changes the wording.
/// A click that goes to a terminal shows the terminal icon instead, without
/// the arrow (the Mac's `SessionOpenActionLabel`).
pub(super) fn open_session_label(source: &str, title: &str, terminal: bool) -> gtk::Box {
    let label = hbox(6, &[]);
    label.set_halign(gtk::Align::Center);
    if terminal {
        let icon = widgets::icon("utilities-terminal-symbolic", 18);
        icon.set_widget_name("session-open-terminal");
        label.append(&icon);
    } else {
        label.append(&widgets::agent_badge(source, 18));
    }
    label.append(&gtk::Label::new(Some(title)));
    if !terminal {
        label.append(&widgets::icon("go-jump-symbolic", 12));
    }
    label
}
