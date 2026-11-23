//! The status bar along the window's foot: the session count and the live
//! agents on the relay, which opens the sessions table filtered to them.

use super::*;

pub(super) fn status_bar(app: &App) -> (gtk::Box, Dot, gtk::Label) {
    // Its 6-point insets are CSS padding, so the bar's color reaches the edges.
    let bar = hbox(4, &["statusbar"]);
    bar.append(&widgets::spacer());
    let relay_count_button = gtk::Button::new();
    crate::ui::widgets::action_cursor(&relay_count_button);
    let counted = hbox(6, &[]);
    let relay_count_dot = Dot::colored(6, widgets::Color::Grey);
    counted.append(&relay_count_dot.widget);
    let relay_count_text = text("Live Agents on Relay: 0", &[]);
    counted.append(&relay_count_text);
    relay_count_button.set_child(Some(&counted));
    relay_count_button.set_tooltip_text(Some("Show your agents"));
    let on_relay = app.on(|app| app.show_chat_page(None));
    relay_count_button.connect_clicked(move |_| on_relay());
    bar.append(&relay_count_button);
    (bar, relay_count_dot, relay_count_text)
}
