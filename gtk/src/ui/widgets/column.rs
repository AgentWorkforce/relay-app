//! A column that holds one child at most a given width wide, as the Mac's
//! pages keep long rows readable on a wide window.

use super::zoom;
use gtk::{glib, prelude::*, subclass::prelude::*};
use gtk4 as gtk;

mod column_imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct Column {
        pub width: Cell<i32>,
        /// Asks for the whole width, not just what its content needs.
        pub fill: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Column {
        const NAME: &'static str = "AgentRelayColumn";
        type Type = super::Column;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Column {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Column {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            let width = zoom::px(self.width.get());
            if orientation == gtk::Orientation::Horizontal {
                let (minimum, natural, _, _) = child.measure(orientation, for_size);
                let natural = if self.fill.get() { width } else { natural };
                (minimum, natural.min(width).max(minimum), -1, -1)
            } else {
                let for_width = if for_size < 0 {
                    -1
                } else {
                    for_size.min(width)
                };
                let (minimum, natural, _, _) = child.measure(orientation, for_width);
                (minimum, natural, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let (minimum, _, _, _) = child.measure(gtk::Orientation::Horizontal, height);
                child.allocate(
                    width.min(zoom::px(self.width.get())).max(minimum),
                    height,
                    baseline,
                    None,
                );
            }
        }
    }
}

glib::wrapper! {
    /// Holds one child at most `width` wide, at the start of the space it
    /// gets, as the Mac's pages keep long rows readable on a wide window.
    pub struct Column(ObjectSubclass<column_imp::Column>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

impl Column {
    pub fn set_max_width(&self, width: i32) {
        if self.imp().width.replace(width) != width {
            self.queue_resize();
        }
    }

    /// Makes the column as wide as it may be whatever its content needs, so
    /// a box under a conversation lines up with the conversation's column.
    pub fn set_fill(&self, fill: bool) {
        if self.imp().fill.replace(fill) != fill {
            self.queue_resize();
        }
    }

    pub fn new(width: i32, child: &impl IsA<gtk::Widget>) -> Column {
        let column: Column = glib::Object::new();
        column.imp().width.set(width);
        child.set_parent(&column);
        column
    }
}
