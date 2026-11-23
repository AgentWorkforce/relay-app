//! Images in a conversation shown where they are, as in Pear and Claude's
//! apps: an attached image opens already expanded, and a screenshot a tool
//! returned is drawn under the run of activity it came from, so it is seen
//! without opening the Activity disclosure.

use gtk::prelude::*;
use gtk4 as gtk;

/// The tallest an inline image is drawn; wider ones scale down to the
/// conversation's column.
const MAX_HEIGHT: i32 = 360;

/// Opens every attached-image disclosure inside `widget`.
pub fn expand_attachments(widget: &gtk::Widget) {
    if widget.widget_name() == "transcript-attached-image" {
        if let Some(disclosure) = widget.downcast_ref::<gtk::Expander>() {
            disclosure.set_expanded(true);
        }
        return;
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        expand_attachments(&current);
        child = current.next_sibling();
    }
}

/// The image at `path`, drawn inline at most [`MAX_HEIGHT`] tall.
pub fn inline_image(path: &str) -> gtk::Picture {
    let picture = gtk::Picture::for_filename(path);
    picture.set_can_shrink(true);
    picture.set_keep_aspect_ratio(true);
    picture.set_size_request(-1, super::super::zoom::px(MAX_HEIGHT));
    picture.set_halign(gtk::Align::Start);
    picture.add_css_class("chat-inline-image");
    picture.set_alternative_text(Some("Screenshot"));
    picture
}
