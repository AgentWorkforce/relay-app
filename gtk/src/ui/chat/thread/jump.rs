//! A round arrow over a conversation scrolled away from its end, as in
//! Claude's and Codex's apps: it brings the newest messages back into view,
//! and hides once they are.

use super::*;

/// How far from the end, in pixels, the arrow shows.
const AWAY: f64 = 240.0;

/// `scroll` with the arrow over its bottom centre.
pub(super) fn overlay(scroll: &gtk::ScrolledWindow) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(scroll));
    let jump = widgets::icon_button("go-down-symbolic", CHAT_JUMP_LATEST, &["chat-jump"], || {});
    jump.set_halign(gtk::Align::Center);
    jump.set_valign(gtk::Align::End);
    jump.set_margin_bottom(super::super::super::zoom::px(16));
    jump.set_visible(false);
    jump.set_widget_name("chat-jump");
    overlay.add_overlay(&jump);
    let adjustment = scroll.vadjustment();
    let held = adjustment.clone();
    jump.connect_clicked(move |_| held.set_value(held.upper()));
    let shown = jump.downgrade();
    let update = move |adjustment: &gtk::Adjustment| {
        if let Some(jump) = shown.upgrade() {
            let away = adjustment.upper() - adjustment.page_size() - adjustment.value();
            jump.set_visible(away > AWAY);
        }
    };
    let changed = update.clone();
    adjustment.connect_value_changed(move |adjustment| update(adjustment));
    adjustment.connect_changed(move |adjustment| changed(adjustment));
    overlay
}
