//! Presentation of the shared core's opening decision, never launch authority.
use super::widgets::{self, hbox};
use agent_relay_desktop_core::session_locations::{SessionOpenAction, SessionOpenPlan};
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

/// Keep the selected session's artwork when a verified plan changes the wording.
pub(super) fn open_session_label(source: &str, title: &str) -> gtk::Box {
    let label = hbox(6, &[]);
    label.set_halign(gtk::Align::Center);
    label.append(&widgets::agent_badge(source, 18));
    label.append(&gtk::Label::new(Some(title)));
    label.append(&widgets::icon("go-jump-symbolic", 12));
    label
}

/// No ordinary row is described as planned until its own reply has arrived.
pub(super) fn title_for(
    key: &str,
    known: Option<&SessionOpenPlan>,
    last: &Option<(String, SessionOpenPlan)>,
) -> String {
    if let Some(plan) = known {
        return title(plan);
    }
    last.as_ref()
        .filter(|(row, _)| row == key)
        .map(|(_, plan)| title(plan))
        .unwrap_or_else(|| "Open session".into())
}

/// Word for word the Mac's SessionLocationNavigation.title, from the actual plan.
fn title(plan: &SessionOpenPlan) -> String {
    let application = plan
        .location
        .as_ref()
        .map(|location| location.application.as_str());
    match plan.action {
        SessionOpenAction::Focus => {
            format!("Open in {}", application.unwrap_or("original terminal"))
        }
        SessionOpenAction::ShowApplication => {
            format!("Show {}", application.unwrap_or("application"))
        }
        SessionOpenAction::OpenLink => {
            if plan
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("codex:"))
            {
                "Open in Codex".into()
            } else {
                "Open in Claude".into()
            }
        }
        SessionOpenAction::Resume => {
            if plan
                .command
                .as_ref()
                .is_some_and(|command| command.args.iter().any(|arg| arg == "--desktop"))
            {
                "Open in Claude".into()
            } else {
                format!("Resume in {}", application.unwrap_or("Terminal"))
            }
        }
        SessionOpenAction::Choose => "Open running session".into(),
        SessionOpenAction::Unavailable => "Session opening unavailable".into(),
    }
}
