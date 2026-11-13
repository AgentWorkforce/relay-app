//! Rebuild the editable transcript heading for the selected native session.
use super::*;
use crate::models::Presence;

impl App {
    pub(super) fn install_transcript_title(&self, page: &TranscriptPage, row: &LocalSessionRow) {
        clear(&page.title_group);
        page.title_cell.borrow_mut().take();
        if let Some(parent) = page.title.parent().and_downcast::<gtk::Button>() {
            parent.set_child(None::<&gtk::Widget>);
        }
        if let Some(parent) = page.title.parent().and_downcast::<gtk::Stack>() {
            parent.remove(&page.title);
        }
        if let Some(parent) = page.title.parent().and_downcast::<gtk::Box>() {
            parent.remove(&page.title);
        }
        let cell = crate::ui::session_titles::SessionTitleCell::build(self, row, &page.title);
        page.title_group.append(&cell.root);
        page.title_group.append(&page.identity);
        *page.title_cell.borrow_mut() = Some(cell);
        page.title.set_text(&row.title());
        page.title.set_tooltip_text(Some(&row.title()));
        let directory = row.directory();
        // The agent name and presence badge follow once the relay is read.
        page.subtitle
            .set_text(&session_header_identity(&source_name(row.source()), None));
        let folder = if directory.is_empty() {
            row.project()
        } else {
            directory
        };
        page.subtitle
            .set_tooltip_text((!folder.is_empty()).then_some(folder.as_str()));
    }
}

/// The header's second line: the provider and the agent's @name, then the
/// presence badge, whose tooltip explains it.
pub(super) fn render_identity(
    page: &TranscriptPage,
    row: &LocalSessionRow,
    presence: Option<Presence>,
    shown_name: Option<&str>,
) {
    page.subtitle.set_text(&session_header_identity(
        &source_name(row.source()),
        shown_name,
    ));
    let badge = presence.and_then(|presence| presence.header_badge());
    page.presence_badge.set_text(badge.unwrap_or_default());
    page.presence_badge.set_visible(badge.is_some());
    for class in ["online", "connecting", "offline"] {
        page.presence_badge.remove_css_class(class);
    }
    if let Some(presence) = presence.filter(|_| badge.is_some()) {
        page.presence_badge.add_css_class(presence.message_class());
        page.presence_badge
            .set_tooltip_text(Some(presence.detail()));
    }
}
