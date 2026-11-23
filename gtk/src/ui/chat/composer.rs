//! The message box under a conversation, as in Pear's: a paperclip that
//! attaches files, chips for what is attached (each with ×), the growing
//! input and Send, and the note under them. A pasted image is attached as
//! `pasted-<time>.png`.

use super::super::app::App;
use super::super::spawn_join::ChatInput;
use super::super::widgets::{self, clear, hbox, text, vbox, LabelStyle};
use crate::models::{
    chat_attachment_label, CHAT_ATTACH, CHAT_ATTACH_TITLE, CHAT_REMOVE_ATTACHMENT, CHAT_SEND,
};
use gtk::{gdk, gio, glib, prelude::*};
use gtk4 as gtk;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub struct Composer {
    pub dock: gtk::Box,
    pub input: ChatInput,
    pub attach: gtk::Button,
    pub send: gtk::Button,
    pub note: gtk::Label,
    chips: gtk::Box,
    /// The files attached to the next message, in order.
    pub files: Rc<RefCell<Vec<PathBuf>>>,
}

impl Composer {
    pub fn build() -> Composer {
        let dock = vbox(6, &["chat-dock"]);
        widgets::pad(&dock, 8, Some(24), Some(14));
        let (column, holder) = widgets::column(760);
        widgets::pad(&column, 0, Some(0), Some(0));
        holder.set_halign(gtk::Align::Center);
        super::super::zoom::size(&column, 560, -1);
        let chips = hbox(6, &["chat-attachments"]);
        chips.set_visible(false);
        column.append(&chips);
        let line = hbox(8, &["chat-composer"]);
        let attach =
            widgets::icon_button("mail-attachment-symbolic", CHAT_ATTACH, &["flat"], || {});
        attach.set_valign(gtk::Align::End);
        line.append(&attach);
        let input = ChatInput::build();
        // About three lines to start, growing to about ten.
        input.grow(72, 240);
        input.view.add_css_class("chat-message-input");
        line.append(&input.root);
        input.focus_within(&line);
        let send = gtk::Button::with_label(CHAT_SEND);
        widgets::action_cursor(&send);
        send.add_css_class("primary");
        send.set_valign(gtk::Align::End);
        line.append(&send);
        column.append(&line);
        let note = text("", &["caption"]).wrapped();
        column.append(&note);
        dock.append(&holder);
        Composer {
            dock,
            input,
            attach,
            send,
            note,
            chips,
            files: Rc::default(),
        }
    }

    /// Draws a chip per attached file; the box hides when none is.
    pub fn render_chips(&self) {
        render_chips(&self.chips, &self.files);
    }

    /// Adds `paths` to what the next message attaches.
    pub fn add(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.files.borrow_mut().extend(paths);
        self.render_chips();
    }

    /// Takes what is attached for a send; the chips clear.
    pub fn take(&self) -> Vec<PathBuf> {
        let files = std::mem::take(&mut *self.files.borrow_mut());
        self.render_chips();
        files
    }
}

/// Draws `files` as chips in `chips`, each removing its file and redrawing.
fn render_chips(chips: &gtk::Box, files: &Rc<RefCell<Vec<PathBuf>>>) {
    clear(chips);
    let listed = files.borrow().clone();
    chips.set_visible(!listed.is_empty());
    for (index, path) in listed.iter().enumerate() {
        let chip = hbox(4, &["chat-attachment"]);
        chip.append(&text(&chat_attachment_label(path), &["caption"]).ellipsized());
        let (held, weak) = (files.clone(), chips.downgrade());
        let remove = widgets::icon_button(
            "window-close-symbolic",
            CHAT_REMOVE_ATTACHMENT,
            &["flat"],
            move || {
                if index < held.borrow().len() {
                    held.borrow_mut().remove(index);
                }
                if let Some(chips) = weak.upgrade() {
                    render_chips(&chips, &held);
                }
            },
        );
        chip.append(&remove);
        chips.append(&chip);
    }
}

impl App {
    pub(super) fn connect_chat_composer(&self, composer: &Composer) {
        let choose = self.on(App::choose_chat_files);
        composer.attach.connect_clicked(move |_| choose());
        // Ctrl+V with an image on the clipboard attaches it; text pastes
        // as usual.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let paste = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(key, gdk::Key::v | gdk::Key::V);
            let image = gdk::Display::default()
                .map(|display| display.clipboard())
                .filter(|clipboard| {
                    clipboard
                        .formats()
                        .contains_type(gdk::Texture::static_type())
                });
            match (paste, image, weak.upgrade()) {
                (true, Some(clipboard), Some(app)) => {
                    app.attach_pasted_image(clipboard);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        composer.input.view.add_controller(keys);
    }

    fn choose_chat_files(&self) {
        let chooser = gtk::FileChooserNative::new(
            Some(CHAT_ATTACH_TITLE),
            Some(self.window()),
            gtk::FileChooserAction::Open,
            Some(CHAT_ATTACH),
            None,
        );
        chooser.set_select_multiple(true);
        // The chooser must outlive this call; its answer releases it.
        let held = Rc::new(RefCell::new(Some(chooser.clone())));
        let weak = self.downgrade();
        chooser.connect_response(move |chooser, response| {
            if response == gtk::ResponseType::Accept {
                let files = chooser.files();
                let paths: Vec<PathBuf> = (0..files.n_items())
                    .filter_map(|index| files.item(index).and_downcast::<gio::File>())
                    .filter_map(|file| file.path())
                    .collect();
                if let Some(app) = weak.upgrade() {
                    app.with_chat(|page| page.composer.add(paths));
                }
            }
            held.borrow_mut().take();
        });
        chooser.show();
    }

    /// Saves the clipboard's image as a PNG in the app's cache and attaches
    /// it.
    fn attach_pasted_image(&self, clipboard: gdk::Clipboard) {
        let weak = self.downgrade();
        clipboard.read_texture_async(gio::Cancellable::NONE, move |result| {
            let (Some(app), Ok(Some(texture))) = (weak.upgrade(), result) else {
                return;
            };
            let folder = glib::user_cache_dir()
                .join("agent-relay")
                .join("chat-attachments");
            let name = format!(
                "pasted-{}.png",
                chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
            );
            let path = folder.join(name);
            let saved =
                std::fs::create_dir_all(&folder).is_ok() && texture.save_to_png(&path).is_ok();
            app.with_chat(|page| {
                if saved {
                    page.composer.add([path]);
                } else {
                    page.composer
                        .note
                        .set_text("Could not attach the pasted image.");
                }
            });
        });
    }
}
