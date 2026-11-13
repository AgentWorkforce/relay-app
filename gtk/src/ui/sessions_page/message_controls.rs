//! Mouse input for Messages stays independent of bulk row selection.
use super::*;

enum Hit {
    Button(gtk::Button),
    Padding,
}

fn hit_test(list: &gtk::Widget, x: f64, y: f64) -> Option<Hit> {
    let mut picked = list.pick(x, y, gtk::PickFlags::INSENSITIVE);
    while let Some(widget) = picked {
        if widget.has_css_class("message-control") {
            return widget.downcast::<gtk::Button>().ok().map(Hit::Button);
        }
        if widget.has_css_class("session-message-column") {
            return Some(Hit::Padding);
        }
        if &widget == list {
            break;
        }
        picked = widget.parent();
    }
    None
}

fn released_button(list: &gtk::Widget, name: &str, x: f64, y: f64) -> Option<gtk::Button> {
    match hit_test(list, x, y) {
        Some(Hit::Button(button))
            if button.is_sensitive() && button.widget_name().as_str() == name =>
        {
            Some(button)
        }
        _ => None,
    }
}

pub(super) fn connect(list: &gtk::ListBox) {
    // Rows are replaced during status updates. Handle message clicks on
    // the stable list, including when GTK's pointer target is the list
    // after the row under a stationary pointer was replaced.
    let click = gtk::GestureClick::new();
    click.set_name(Some("session-message-click"));
    click.set_button(1);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pending = std::rc::Rc::new(RefCell::new(None::<String>));
    let pressed = pending.clone();
    click.connect_pressed(move |gesture, presses, x, y| {
        pressed.borrow_mut().take();
        if presses != 1 {
            return;
        }
        let Some(list) = gesture.widget() else {
            return;
        };
        if let Some(hit) = hit_test(&list, x, y) {
            if let Hit::Button(button) = hit {
                if button.is_sensitive() {
                    *pressed.borrow_mut() = Some(button.widget_name().to_string());
                }
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    let released = pending.clone();
    click.connect_released(move |gesture, presses, x, y| {
        let name = released.borrow_mut().take();
        if presses != 1 {
            return;
        }
        if let (Some(name), Some(list)) = (name, gesture.widget()) {
            if let Some(button) = released_button(&list, &name, x, y) {
                // Finish the release before the action replaces its row.
                glib::idle_add_local_once(move || button.emit_clicked());
            }
        }
    });
    click.connect_stopped(move |_| {
        pending.borrow_mut().take();
    });
    list.add_controller(click);
}

pub(super) fn set_cursor(button: &gtk::Button, presence: Presence) {
    if presence.is_changing() {
        button.set_cursor_from_name(Some("progress"));
    }
}

impl App {
    pub(super) fn live_session_columns(&self) -> SessionColumnLayout {
        self.shell()
            .and_then(|shell| {
                shell
                    .pages
                    .sessions
                    .resizing
                    .borrow()
                    .as_ref()
                    .map(|(_, _, layout)| layout.clone())
            })
            .unwrap_or_else(|| self.session_columns())
    }
}
