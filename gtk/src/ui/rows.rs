//! Pointer handling and context menus shared by list rows.

use super::widgets::{self, Callback};
use gtk::prelude::*;
use gtk4 as gtk;

/// Runs `on` with the pointer button and position of each press on
/// `widget`. A primary press on a control inside it (a chevron, a switch)
/// is the control's own: the row acting too would redraw the list and take
/// the control away before its click lands.
pub(super) fn on_press(widget: &impl IsA<gtk::Widget>, on: impl Fn(u32, f64, f64) + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.connect_pressed(move |gesture, presses, x, y| {
        let button = gesture.current_button();
        if presses != 1 {
            return;
        }
        if button == 1 {
            if let Some(row) = gesture.widget() {
                if within_control(row.pick(x, y, gtk::PickFlags::DEFAULT), &row) {
                    return;
                }
            }
        }
        on(button, x, y);
    });
    widget.add_controller(click);
}

/// Whether `picked` is, or is inside, a control that handles its own
/// clicks, below `row`.
pub fn within_control(picked: Option<gtk::Widget>, row: &gtk::Widget) -> bool {
    let mut current = picked;
    while let Some(widget) = current {
        if &widget == row {
            return false;
        }
        if widget.is::<gtk::Button>()
            || widget.is::<gtk::Switch>()
            || widget.is::<gtk::CheckButton>()
            || widget.is::<gtk::DropDown>()
            || widget.is::<gtk::Entry>()
            || widget.is::<gtk::Text>()
        {
            return true;
        }
        current = widget.parent();
    }
    false
}

/// A menu's items: a label and what it runs, `None` for an insensitive
/// line, `-` for a separator.
pub type MenuItems = Vec<(String, Option<Callback>)>;

/// A menu under `widget`, or at `at` within it.
pub(super) fn menu(widget: &impl IsA<gtk::Widget>, at: Option<(f64, f64)>, items: MenuItems) {
    let (labels, actions): (Vec<String>, Vec<Option<Callback>>) = items.into_iter().unzip();
    let popover = widgets::popup_menu(
        widget,
        labels.iter().map(String::as_str).zip(actions).collect(),
    );
    if let Some((x, y)) = at {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
}
