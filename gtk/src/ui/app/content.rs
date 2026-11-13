//! Swapping top-level window content.

use super::*;

impl App {
    pub(in crate::ui) fn replace(&self, content: &impl IsA<gtk::Widget>) {
        self.inner.signing.borrow_mut().take();
        // The window decoration follows the welcome split (theme.rs).
        if content.has_css_class("welcome") {
            self.inner.window.add_css_class("welcome");
        } else {
            self.inner.window.remove_css_class("welcome");
        }
        self.inner.window.set_child(Some(content));
    }
}
