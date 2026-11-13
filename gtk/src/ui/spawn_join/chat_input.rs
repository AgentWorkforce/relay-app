//! The session composer's message box, as in Cloud's chat.

use super::super::widgets::{text, LabelStyle};
use gtk::{glib, prelude::*};
use gtk4 as gtk;

/// The message box: a text view that grows with what is typed, up to about
/// eight lines, then scrolls, as the Mac's `SpawnComposer` field does. Return
/// sends and Shift+Return starts a new line, as in Cloud's composer.
pub struct ChatInput {
    pub root: gtk::Overlay,
    pub view: gtk::TextView,
    placeholder: gtk::Label,
}

impl ChatInput {
    pub(super) fn build() -> ChatInput {
        let view = gtk::TextView::new();
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_accepts_tab(false);
        view.set_hexpand(true);
        view.add_css_class("chat-input");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(160);
        scroll.set_hexpand(true);
        scroll.set_valign(gtk::Align::Center);
        scroll.set_child(Some(&view));
        let placeholder = text("", &["tertiary"]).ellipsized();
        placeholder.set_halign(gtk::Align::Start);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_can_target(false);
        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_child(Some(&scroll));
        root.add_overlay(&placeholder);
        let shown = placeholder.clone();
        view.buffer()
            .connect_changed(move |buffer| shown.set_visible(buffer.char_count() == 0));

        ChatInput {
            root,
            view,
            placeholder,
        }
    }

    pub fn text(&self) -> glib::GString {
        let buffer = self.view.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false)
    }

    pub fn set_text(&self, value: &str) {
        self.view.buffer().set_text(value);
    }

    pub fn placeholder_text(&self) -> Option<glib::GString> {
        Some(self.placeholder.text())
    }

    pub fn set_placeholder_text(&self, value: Option<&str>) {
        self.placeholder.set_text(value.unwrap_or_default());
    }

    /// The shell around the input: its focus ring follows the text, not the
    /// Send button beside it, and a click anywhere in it starts typing, as on
    /// the Mac.
    pub fn focus_within(&self, shell: &gtk::Box) {
        let focus = gtk::EventControllerFocus::new();
        let ring = shell.downgrade();
        focus.connect_enter(move |_| {
            if let Some(ring) = ring.upgrade() {
                ring.add_css_class("typing");
            }
        });
        let ring = shell.downgrade();
        focus.connect_leave(move |_| {
            if let Some(ring) = ring.upgrade() {
                ring.remove_css_class("typing");
            }
        });
        self.view.add_controller(focus);
        let click = gtk::GestureClick::new();
        let view = self.view.downgrade();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(view) = view.upgrade() {
                view.grab_focus();
            }
        });
        shell.add_controller(click);
    }

    /// `send` runs on Return; Shift+Return still starts a new line, and
    /// Return that commits input-method text being composed never sends.
    pub fn connect_activate(&self, send: impl Fn() + 'static) {
        let keys = gtk::EventControllerKey::new();
        keys.set_name(Some("chat-input-send"));
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |keys, key, _, modifiers| {
            let enter = matches!(
                key,
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter | gtk::gdk::Key::ISO_Enter
            );
            if !enter || modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }
            // Capture runs before the text view's own controller, which hands
            // keys to the input method: let it see Return first.
            if let (Some(event), Some(view)) = (
                keys.current_event(),
                keys.widget().and_downcast::<gtk::TextView>(),
            ) {
                if view.im_context_filter_keypress(&event) {
                    return glib::Propagation::Stop;
                }
            }
            send();
            glib::Propagation::Stop
        });
        self.view.add_controller(keys);
    }
}
