//! A row of two children that stacks when the window is too narrow for both.

use super::super::zoom;
use gtk::{glib, prelude::*, subclass::prelude::*};
use gtk4 as gtk;
use std::cell::Cell;

mod reflow_imp {
    use super::*;

    #[derive(Default)]
    pub struct Reflow {
        /// The space between the two children at 100%: beside, then above.
        pub spacing: Cell<(i32, i32)>,
        /// The narrowest the first child gets beside the second, at 100%.
        pub floor: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Reflow {
        const NAME: &'static str = "AgentRelayReflow";
        type Type = super::Reflow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Reflow {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl Reflow {
        /// The first child, and the second if it is shown.
        fn children(&self) -> Option<(gtk::Widget, Option<gtk::Widget>)> {
            let start = self.obj().first_child()?;
            let end = start.next_sibling().filter(|end| end.should_layout());
            Some((start, end))
        }

        /// The first child's width when the second goes beside it in
        /// `width`, or `None` when the second goes under it.
        fn beside(&self, start: &gtk::Widget, end: &gtk::Widget, width: i32) -> Option<i32> {
            let (start_min, _, _, _) = start.measure(gtk::Orientation::Horizontal, -1);
            let (_, end_natural, _, _) = end.measure(gtk::Orientation::Horizontal, -1);
            let room = width - zoom::px(self.spacing.get().0) - end_natural;
            (room >= start_min.max(zoom::px(self.floor.get()))).then_some(room)
        }
    }

    /// The second child's width in `width`: its natural width, at least
    /// its minimum.
    fn end_width(end: &gtk::Widget, width: i32) -> i32 {
        let (minimum, natural, _, _) = end.measure(gtk::Orientation::Horizontal, -1);
        natural.min(width).max(minimum)
    }

    impl WidgetImpl for Reflow {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some((start, end)) = self.children() else {
                return (0, 0, -1, -1);
            };
            let Some(end) = end else {
                let (minimum, natural, _, _) = start.measure(orientation, for_size);
                return (minimum, natural, -1, -1);
            };
            let (beside, above) = self.spacing.get();
            if orientation == gtk::Orientation::Horizontal {
                let (start_min, start_natural, _, _) = start.measure(orientation, -1);
                let (end_min, end_natural, _, _) = end.measure(orientation, -1);
                return (
                    start_min.max(end_min),
                    start_natural + zoom::px(beside) + end_natural,
                    -1,
                    -1,
                );
            }
            let width = (for_size >= 0).then_some(for_size);
            let end_width = width.map_or(-1, |width| end_width(&end, width));
            match width.and_then(|width| self.beside(&start, &end, width)) {
                Some(room) => {
                    let (start_min, start_natural, _, _) = start.measure(orientation, room);
                    let (end_min, end_natural, _, _) = end.measure(orientation, end_width);
                    (
                        start_min.max(end_min),
                        start_natural.max(end_natural),
                        -1,
                        -1,
                    )
                }
                None => {
                    let (start_min, start_natural, _, _) =
                        start.measure(orientation, width.unwrap_or(-1));
                    let (end_min, end_natural, _, _) = end.measure(orientation, end_width);
                    let gap = zoom::px(above);
                    (
                        start_min + gap + end_min,
                        start_natural + gap + end_natural,
                        -1,
                        -1,
                    )
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some((start, end)) = self.children() else {
                return;
            };
            let Some(end) = end else {
                start.allocate(width, height, -1, None);
                return;
            };
            let at = |x: i32, y: i32| {
                gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(x as f32, y as f32))
            };
            let end_width = end_width(&end, width);
            match self.beside(&start, &end, width) {
                Some(room) => {
                    start.allocate(room, height, -1, None);
                    end.allocate(end_width, height, -1, Some(at(width - end_width, 0)));
                }
                None => {
                    let (start_height, _, _, _) = start.measure(gtk::Orientation::Vertical, width);
                    let top = start_height + zoom::px(self.spacing.get().1);
                    start.allocate(width, start_height, -1, None);
                    end.allocate(end_width, (height - top).max(0), -1, Some(at(0, top)));
                }
            }
        }
    }
}

/// The narrowest a row's text gets, at 100%, before the control beside it
/// goes under it.
pub const ROW_TEXT_WIDTH: i32 = 120;

glib::wrapper! {
    /// Two children side by side, the second at its natural width at the
    /// end, while the first keeps room; in a narrower window (a small screen
    /// at a high zoom) the second goes under the first, at the start,
    /// instead of clipping the page.
    pub struct Reflow(ObjectSubclass<reflow_imp::Reflow>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

impl Reflow {
    /// `first`, then `second`, `beside` apart side by side and `above`
    /// apart one under the other; side by side only while `first` keeps
    /// `floor` and its own minimum. Sizes are at 100% and follow the zoom.
    pub fn new(
        first: &impl IsA<gtk::Widget>,
        second: &impl IsA<gtk::Widget>,
        beside: i32,
        above: i32,
        floor: i32,
    ) -> Reflow {
        let reflow: Reflow = glib::Object::new();
        reflow.imp().spacing.set((beside, above));
        reflow.imp().floor.set(floor);
        first.set_parent(&reflow);
        second.set_parent(&reflow);
        reflow.set_hexpand(true);
        reflow
    }

    /// A settings row's text, with `accessory` beside it or under it.
    pub(super) fn settings_row(words: &gtk::Box, accessory: Option<&gtk::Widget>) -> gtk::Widget {
        let Some(accessory) = accessory else {
            return words.clone().upcast();
        };
        accessory.set_valign(gtk::Align::Start);
        Reflow::new(words, accessory, 14, 10, ROW_TEXT_WIDTH).upcast()
    }
}
