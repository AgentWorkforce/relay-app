//! The search popover, which grows out of the search bar and collapses into
//! it again when it closes.
use gtk4 as gtk;

mod imp {
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk4 as gtk;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct AnimatedSearch {
        generation: Cell<u64>,
        closing: Cell<bool>,
    }
    #[gtk::glib::object_subclass]
    impl ObjectSubclass for AnimatedSearch {
        const NAME: &'static str = "RelayAnimatedSearchPopover";
        type Type = super::AnimatedSearchPopover;
        type ParentType = gtk::Popover;
    }
    impl AnimatedSearch {
        pub fn resume(&self) {
            if self.closing.replace(false) {
                self.generation.set(self.generation.get() + 1);
                self.obj().remove_css_class("search-collapsed");
            }
        }
    }
    impl ObjectImpl for AnimatedSearch {}
    impl PopoverImpl for AnimatedSearch {}
    impl WidgetImpl for AnimatedSearch {
        fn show(&self) {
            self.generation.set(self.generation.get() + 1);
            self.closing.set(false);
            let obj = self.obj();
            obj.add_css_class("search-collapsed");
            self.parent_show();
            let generation = self.generation.get();
            let weak = obj.downgrade();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(16), move || {
                if let Some(obj) = weak.upgrade() {
                    if obj.imp().generation.get() == generation {
                        obj.remove_css_class("search-collapsed");
                    }
                }
            });
        }
        fn hide(&self) {
            if self.closing.replace(true) {
                return;
            }
            self.generation.set(self.generation.get() + 1);
            let obj = self.obj();
            obj.add_css_class("search-collapsed");
            let animated = gtk::Settings::default()
                .is_some_and(|settings| settings.property::<bool>("gtk-enable-animations"));
            if !animated {
                self.parent_hide();
                self.closing.set(false);
                return;
            }
            let generation = self.generation.get();
            let weak = obj.downgrade();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                if let Some(obj) = weak.upgrade() {
                    if obj.imp().generation.get() == generation {
                        obj.imp().parent_hide();
                        obj.imp().closing.set(false);
                    }
                }
            });
        }
    }
}
gtk::glib::wrapper! {
    pub struct AnimatedSearchPopover(ObjectSubclass<imp::AnimatedSearch>)
        @extends gtk::Popover, gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::ShortcutManager;
}
