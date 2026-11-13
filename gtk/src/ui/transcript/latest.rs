//! Follow the newest message, and offer a way back to it from scrollback.
use super::*;

/// The conversation column: the transcript scroll, then the Latest message
/// row shown while the reader is away from the newest message.
pub(super) fn follow_latest(
    scroll: &gtk::ScrolledWindow,
    scroll_position: &Rc<RefCell<TranscriptScrollState>>,
) -> (gtk::Button, gtk::Box, gtk::Box) {
    let latest = gtk::Button::with_label("Latest message");
    latest.add_css_class("flat");
    widgets::action_cursor(&latest);
    latest.add_css_class("transcript-latest");
    latest.set_halign(gtk::Align::Center);
    latest.set_margin_top(8);
    latest.set_margin_bottom(8);
    // Its own row under the conversation, so it never covers message text.
    let latest_row = vbox(0, &["transcript-latest-row"]);
    latest_row.append(&latest);
    latest_row.set_visible(false);
    let conversation = vbox(0, &[]);
    conversation.set_vexpand(true);
    conversation.append(scroll);
    conversation.append(&latest_row);
    let on_scroll = {
        let position = scroll_position.clone();
        let latest_row = latest_row.clone();
        Rc::new(move |adjustment: &gtk::Adjustment| {
            let target = position.borrow_mut().update(
                adjustment.value(),
                adjustment.upper(),
                adjustment.page_size(),
            );
            if let Some(target) = target {
                if (target - adjustment.value()).abs() > 0.5 {
                    adjustment.set_value(target);
                }
            }
            // Shown once the reader is back in history; hidden once they are
            // within the row's own height of the bottom, so the room it gives
            // back is taken up by the scroll's own end, never by pulling the
            // reader down mid-scroll.
            let rest = adjustment.upper() - adjustment.page_size() - adjustment.value();
            let bottom = rest <= f64::from(latest_row.height()).max(1.0);
            if !position.borrow().at_latest() {
                latest_row.set_visible(true);
            } else if bottom {
                latest_row.set_visible(false);
            }
        })
    };
    let update = on_scroll.clone();
    scroll
        .vadjustment()
        .connect_changed(move |adjustment| update(adjustment));
    scroll
        .vadjustment()
        .connect_value_changed(move |adjustment| on_scroll(adjustment));
    let position = scroll_position.clone();
    let adjustment = scroll.vadjustment();
    // Reaching the bottom hides the row, through the scroll handler above.
    latest.connect_clicked(move |_| {
        position.borrow_mut().jump_to_latest();
        adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
    });
    (latest, latest_row, conversation)
}
