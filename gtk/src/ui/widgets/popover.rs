//! Shared popover construction and cleanup after GTK closes a menu.
use super::{action_cursor, add_classes, pad, vbox, Callback};
use gtk::{glib, prelude::*};
use gtk4 as gtk;

/// A popover of flat buttons under `anchor`; `None` as an action draws an
/// insensitive line, `-` as a label a separator.
pub fn popup_menu(
    anchor: &impl IsA<gtk::Widget>,
    items: Vec<(&str, Option<Callback>)>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    let content = vbox(2, &[]);
    pad(&content, 6, None, None);
    for (label, action) in items {
        if label == "-" {
            content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            continue;
        }
        let item = gtk::Button::with_label(label);
        action_cursor(&item);
        add_classes(&item, &["flat"]);
        if let Some(child) = item.child() {
            child.set_halign(gtk::Align::Start);
        }
        match action {
            Some(action) => {
                let popover = popover.downgrade();
                item.connect_clicked(move |_| {
                    if let Some(popover) = popover.upgrade() {
                        popover.popdown();
                    }
                    action();
                });
            }
            None => item.set_sensitive(false),
        }
        content.append(&item);
    }
    popover.set_child(Some(&content));
    unparent_when_closed(&popover);
    popover.popup();
    popover
}

/// Remove a closed manually-parented menu after GTK finishes its signal.
pub fn unparent_when_closed(popover: &gtk::Popover) {
    // The signal holds a weak reference to avoid a parent → popover → parent
    // cycle. Only the pending idle callback retains the closing menu's parent.
    let parent = popover.parent().map(|parent| parent.downgrade());
    popover.connect_closed(move |popover| {
        let Some(parent) = parent.as_ref().and_then(|parent| parent.upgrade()) else {
            return;
        };
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            if popover.parent().as_ref() == Some(&parent) {
                popover.unparent();
            }
        });
    });
}
