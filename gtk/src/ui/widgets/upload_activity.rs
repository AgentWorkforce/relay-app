//! Animated upload progress arrow shared by session rows.
use super::{art, color, display_scale, raster_tinted};
use gtk::{gdk, glib, prelude::*, subclass::prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};

mod upload_activity_imp {
    use super::*;

    #[derive(Default)]
    pub struct UploadActivityIcon {
        pub size: Cell<i32>,
        pub elapsed: Cell<f64>,
        pub base: RefCell<Option<gdk::Texture>>,
        pub accent: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for UploadActivityIcon {
        const NAME: &'static str = "AgentRelayUploadActivityIcon";
        type Type = super::UploadActivityIcon;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for UploadActivityIcon {}

    impl WidgetImpl for UploadActivityIcon {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (self.size.get(), self.size.get(), -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let size = self.size.get() as f32;
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, size, size);
            if let Some(base) = self.base.borrow().as_ref() {
                snapshot.push_opacity(0.35);
                snapshot.append_texture(base, &bounds);
                snapshot.pop();
            }
            let (fill, opacity) = crate::models::upload_activity_frame(
                self.elapsed.get(),
                !self.obj().settings().is_gtk_enable_animations(),
            );
            if let Some(accent) = self.accent.borrow().as_ref() {
                let height = size * fill as f32;
                snapshot.push_clip(&gtk::graphene::Rect::new(0.0, size - height, size, height));
                snapshot.push_opacity(opacity);
                snapshot.append_texture(accent, &bounds);
                snapshot.pop();
                snapshot.pop();
            }
        }
    }
}

glib::wrapper! {
    pub struct UploadActivityIcon(ObjectSubclass<upload_activity_imp::UploadActivityIcon>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

/// Fill the upload arrow from bottom to top, then gently fade before repeating.
/// The frame clock stops driving an unmapped widget; the callback holds no widget
/// reference and dies with the row. Reduced motion displays a steady arrow.
pub fn upload_activity_icon(size: i32) -> UploadActivityIcon {
    let widget: UploadActivityIcon = glib::Object::new();
    widget.imp().size.set(size);
    widget.set_widget_name("session-upload-activity");
    widget.set_valign(gtk::Align::Center);
    let data = art::GLYPHS
        .iter()
        .find(|(name, _)| *name == "arrow.up.circle.fill")
        .unwrap()
        .1;
    let pixels = size * display_scale();
    *widget.imp().base.borrow_mut() = raster_tinted(
        "glyph:arrow.up.circle.fill",
        data,
        pixels,
        pixels,
        Some(color("secondary")),
    );
    *widget.imp().accent.borrow_mut() = raster_tinted(
        "glyph:arrow.up.circle.fill",
        data,
        pixels,
        pixels,
        Some(color("accent")),
    );
    let start = Cell::new(None);
    let last_enabled = Cell::new(None);
    widget.add_tick_callback(move |widget, clock| {
        let now = clock.frame_time();
        let since = *start.get().get_or_insert(now);
        start.set(Some(since));
        let enabled = widget.settings().is_gtk_enable_animations();
        widget.imp().elapsed.set((now - since) as f64 / 1_000_000.0);
        if enabled || last_enabled.get() != Some(enabled) {
            widget.queue_draw();
        }
        last_enabled.set(Some(enabled));
        glib::ControlFlow::Continue
    });
    widget
}
