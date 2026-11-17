//! The session sidebar's note under the agent's name naming the repository
//! the agent's registration names (`agents.list` `repository`). The Mac's
//! `LocalSessionSidebar` shows the same line.

use crate::models::repository_line;
use agent_relay_node::relay::contract::agents::LocalAgent;
use gtk::prelude::*;
use gtk4 as gtk;

/// The repository line's widget name, by which it is found again.
const REPOSITORY: &str = "agent-repository";

/// Shows what the sidebar says of the session's agent beyond its name: the
/// button that copies the name, and the repository line.
pub(super) fn show_agent(copy_name: &gtk::Button, naming: &gtk::Box, agent: Option<&LocalAgent>) {
    copy_name.set_visible(agent.is_some());
    show_repository(naming, agent);
}

/// Shows, at the end of `naming`, the repository `agent`'s registration
/// names, or nothing; the line is made the first time it is needed.
fn show_repository(naming: &gtk::Box, agent: Option<&LocalAgent>) {
    let repository = agent.and_then(|agent| agent.repository.as_deref());
    let label = line(naming).or_else(|| {
        repository.map(|_| {
            let label = gtk::Label::new(None);
            label.set_widget_name(REPOSITORY);
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_selectable(true);
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            naming.append(&label);
            label
        })
    });
    if let Some(label) = label {
        label.set_text(&repository.map(repository_line).unwrap_or_default());
        label.set_visible(repository.is_some());
    }
}

/// The repository line under `naming`, once made.
fn line(naming: &gtk::Box) -> Option<gtk::Label> {
    let mut child = naming.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == REPOSITORY {
            return widget.downcast::<gtk::Label>().ok();
        }
        child = widget.next_sibling();
    }
    None
}
