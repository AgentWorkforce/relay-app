//! Zooming the whole interface, as a browser does: the zoom level, the
//! sizes and spacing that follow it, the window that grows with it, and the
//! ways to change it (View-style actions with their shortcuts, Ctrl and the
//! mouse wheel, the tray and the Appearance pane).
//!
//! The stylesheet scales its own pixels (`theme::css_at`) and GTK's font
//! resolution follows the zoom (`Styling::set_zoom`). What code sets in
//! pixels goes through [`px`] when it is built, or through [`size`],
//! [`margins`], [`spacing`] and [`pixel_size`] when the widget outlives a
//! redraw (the header, the sidebar): those remember the size at 100% and
//! follow a later change of zoom.

use super::app::App;
use super::widgets::{self, hbox, icon_button, text};
use crate::prefs::UiZoom;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The smallest window at 100%, which the zoom scales.
pub const MIN_WINDOW: (i32, i32) = (980, 650);
/// The registry is swept of dropped widgets once it holds this many.
const SWEEP_FLOOR: usize = 512;

thread_local! {
    static CURRENT: Cell<UiZoom> = const { Cell::new(UiZoom::ACTUAL_SIZE) };
    static SIZED: RefCell<Registry> = const {
        RefCell::new(Registry {
            entries: Vec::new(),
            sweep_at: SWEEP_FLOOR,
        })
    };
}

/// The zoom the interface is drawn at.
pub fn current() -> UiZoom {
    CURRENT.with(Cell::get)
}

/// `pixels` at 100%, at the current zoom: whole pixels, and unset sizes
/// (zero or less) left alone.
pub fn px(pixels: i32) -> i32 {
    current().scale(pixels)
}

/// Draws at `zoom` from now on, resizing every remembered widget.
pub fn set_current(zoom: UiZoom) {
    CURRENT.with(|current| current.set(zoom));
    SIZED.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.sweep();
        for entry in &registry.entries {
            if let Some(widget) = entry.widget.upgrade() {
                entry.base.apply(&widget, zoom);
            }
        }
    });
}

/// `widget`'s size request, `width` by `height` at 100%.
pub fn size(widget: &impl IsA<gtk::Widget>, width: i32, height: i32) {
    remember(widget.upcast_ref(), Base::Size(width, height));
}

/// `widget`'s margins at 100%: top, end, bottom, start.
pub fn margins(widget: &impl IsA<gtk::Widget>, top: i32, end: i32, bottom: i32, start: i32) {
    remember(
        widget.upcast_ref(),
        Base::Margins([top, end, bottom, start]),
    );
}

/// The space between `container`'s children at 100%. A later call for the
/// same box wins, at this zoom and every later one: remembered sizes are
/// applied in the order they were given, so a box made by `hbox`/`vbox` can
/// be given other spacing afterwards.
pub fn spacing(container: &impl IsA<gtk::Box>, pixels: i32) {
    remember(container.as_ref().upcast_ref(), Base::Spacing(pixels));
}

/// A box laid out along `orientation`, its children `pixels` apart at 100%.
pub fn boxed(orientation: gtk::Orientation, pixels: i32) -> gtk::Box {
    let container = gtk::Box::new(orientation, 0);
    spacing(&container, pixels);
    container
}

/// An icon's size at 100%.
pub fn pixel_size(image: &gtk::Image, pixels: i32) {
    remember(image.upcast_ref(), Base::PixelSize(pixels));
}

/// A size remembered at 100%.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    Size(i32, i32),
    Margins([i32; 4]),
    Spacing(i32),
    PixelSize(i32),
}

impl Base {
    /// Whether the zoom changes it: one pixel and less never scales.
    fn scales(self) -> bool {
        match self {
            Base::Size(width, height) => width > 1 || height > 1,
            Base::Margins(sides) => sides.iter().any(|side| *side > 1),
            Base::Spacing(pixels) | Base::PixelSize(pixels) => pixels > 1,
        }
    }

    fn apply(self, widget: &gtk::Widget, zoom: UiZoom) {
        match self {
            Base::Size(width, height) => {
                widget.set_size_request(zoom.scale(width), zoom.scale(height));
            }
            Base::Margins([top, end, bottom, start]) => {
                widget.set_margin_top(zoom.scale(top));
                widget.set_margin_end(zoom.scale(end));
                widget.set_margin_bottom(zoom.scale(bottom));
                widget.set_margin_start(zoom.scale(start));
            }
            Base::Spacing(pixels) => {
                if let Some(container) = widget.downcast_ref::<gtk::Box>() {
                    container.set_spacing(zoom.scale(pixels));
                }
            }
            Base::PixelSize(pixels) => {
                if let Some(image) = widget.downcast_ref::<gtk::Image>() {
                    image.set_pixel_size(zoom.scale(pixels));
                }
            }
        }
    }
}

struct Entry {
    widget: glib::WeakRef<gtk::Widget>,
    base: Base,
}

struct Registry {
    entries: Vec<Entry>,
    /// Pages are rebuilt often: dropped widgets are swept out when the
    /// registry reaches this, which then doubles past what is left.
    sweep_at: usize,
}

impl Registry {
    fn sweep(&mut self) {
        self.entries
            .retain(|entry| entry.widget.upgrade().is_some());
        self.sweep_at = (self.entries.len() * 2).max(SWEEP_FLOOR);
    }
}

fn remember(widget: &gtk::Widget, base: Base) {
    base.apply(widget, current());
    if !base.scales() {
        return;
    }
    SIZED.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.entries.len() >= registry.sweep_at {
            registry.sweep();
        }
        registry.entries.push(Entry {
            widget: widget.downgrade(),
            base,
        });
    });
}

/// The share of the screen's width and height a zoomed window may claim as
/// its smallest size, leaving room for a taskbar, a top bar or a dock and the
/// window's own title bar. GTK 4 has no portable work area: `gdk::Monitor`
/// lost `workarea` in GTK 4, X11 and Win32 have it only through their own
/// backends, and Wayland has none.
const USABLE_SHARE: f64 = 0.85;

/// `size` at `zoom`, but never larger than `screen` unless it already was
/// at 100%: a window zoomed to 200% still fits a 1080p screen.
pub fn fitted(size: (i32, i32), zoom: UiZoom, screen: Option<(i32, i32)>) -> (i32, i32) {
    let scaled = (zoom.scale(size.0), zoom.scale(size.1));
    match screen {
        Some((width, height)) => (
            scaled.0.min(width.max(size.0)),
            scaled.1.min(height.max(size.1)),
        ),
        None => scaled,
    }
}

/// What windows may use of a monitor `width` by `height`: [`USABLE_SHARE`]
/// of each side.
pub fn usable(width: i32, height: i32) -> (i32, i32) {
    let share = |side: i32| (f64::from(side) * USABLE_SHARE).floor() as i32;
    (share(width), share(height))
}

/// What windows may use of the screen `window` is on, or of the first
/// screen: its size less room for the desktop's panels and the title bar.
pub fn screen(window: &gtk::Window) -> Option<(i32, i32)> {
    let display = WidgetExt::display(window);
    let monitor = window
        .surface()
        .and_then(|surface| display.monitor_at_surface(&surface))
        .or_else(|| display.monitors().item(0).and_downcast::<gdk::Monitor>())?;
    let geometry = monitor.geometry();
    Some(usable(geometry.width(), geometry.height()))
}

/// Sizes `window` for `zoom`: its smallest and its default size.
pub fn size_window(window: &gtk::Window, zoom: UiZoom) {
    let screen = screen(window);
    let (width, height) = fitted(MIN_WINDOW, zoom, screen);
    window.set_size_request(width, height);
    let (width, height) = fitted(
        (
            crate::models::DEFAULT_WINDOW_WIDTH,
            crate::models::DEFAULT_WINDOW_HEIGHT,
        ),
        zoom,
        screen,
    );
    window.set_default_size(width, height);
}

/// A step on the zoom ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomStep {
    In,
    Out,
    Reset,
}

impl ZoomStep {
    fn from(self, zoom: UiZoom) -> UiZoom {
        match self {
            ZoomStep::In => zoom.zoomed_in(),
            ZoomStep::Out => zoom.zoomed_out(),
            ZoomStep::Reset => UiZoom::ACTUAL_SIZE,
        }
    }
}

/// One scroll of `dy` notches, `travel` notches into a zoom: the travel
/// left over, and the step to take. Only Ctrl zooms; up zooms in. A touchpad
/// scrolls in fractions of a notch, which add up to a step per notch.
pub fn wheel_step(travel: f64, dy: f64, ctrl: bool) -> (f64, Option<ZoomStep>) {
    if !ctrl {
        return (0.0, None);
    }
    let travel = travel + dy;
    if travel.abs() < 1.0 {
        return (travel, None);
    }
    let step = if travel < 0.0 {
        ZoomStep::In
    } else {
        ZoomStep::Out
    };
    (0.0, Some(step))
}

/// The zoom actions and their shortcuts.
pub const ACTIONS: [(&str, ZoomStep, &[&str]); 3] = [
    (
        "zoom-in",
        ZoomStep::In,
        &["<Control>equal", "<Control>plus", "<Control>KP_Add"],
    ),
    (
        "zoom-out",
        ZoomStep::Out,
        &["<Control>minus", "<Control>KP_Subtract"],
    ),
    (
        "zoom-reset",
        ZoomStep::Reset,
        &["<Control>0", "<Control>KP_0"],
    ),
];

impl App {
    /// The zoom the interface is drawn at.
    pub fn ui_zoom(&self) -> UiZoom {
        self.inner.styling.zoom()
    }

    pub fn zoom_in(&self) {
        self.zoom(ZoomStep::In);
    }

    pub fn zoom_out(&self) {
        self.zoom(ZoomStep::Out);
    }

    pub fn zoom_reset(&self) {
        self.zoom(ZoomStep::Reset);
    }

    pub fn zoom(&self, step: ZoomStep) {
        self.set_ui_zoom(step.from(self.ui_zoom()));
    }

    /// Draws the whole interface at `zoom`, and remembers it.
    pub fn set_ui_zoom(&self, zoom: UiZoom) {
        if zoom == self.ui_zoom() {
            return;
        }
        // A preference that cannot be saved holds for this run.
        let _ = self.inner.prefs.borrow_mut().set_ui_zoom(zoom);
        self.inner.styling.set_zoom(zoom);
        size_window(&self.inner.window, zoom);
        self.enable_zoom_actions();
        // Pages, marks and the header are built at a size: rebuild them.
        self.theme_changed(self.inner.styling.theme());
    }

    /// Sizes the window for the saved zoom, and zooms with Ctrl and the
    /// mouse wheel.
    pub(super) fn prepare_zoom(&self) {
        size_window(&self.inner.window, self.ui_zoom());
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let travel = Rc::new(Cell::new(0.0));
        let weak = self.downgrade();
        wheel.connect_scroll(move |wheel, _, dy| {
            let Some(app) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = wheel
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            let (left, step) = wheel_step(travel.get(), dy, ctrl);
            travel.set(left);
            if let Some(step) = step {
                app.zoom(step);
            }
            if ctrl {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.inner.window.add_controller(wheel);
    }

    /// The zoom actions, `app.zoom-in`, `app.zoom-out` and `app.zoom-reset`,
    /// and their shortcuts.
    pub(super) fn install_zoom_actions(&self, application: &gtk::Application) {
        for (name, step, accels) in ACTIONS {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.downgrade();
            action.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    app.zoom(step);
                }
            });
            application.add_action(&action);
            application.set_accels_for_action(&format!("app.{name}"), accels);
        }
        self.enable_zoom_actions();
    }

    /// Zooming in stops at 200%, out and back at 100%.
    fn enable_zoom_actions(&self) {
        let zoom = self.ui_zoom();
        let Some(application) = self.inner.window.application() else {
            return;
        };
        for (name, enabled) in [
            ("zoom-in", zoom.can_zoom_in()),
            ("zoom-out", zoom.can_zoom_out()),
            ("zoom-reset", !zoom.is_actual_size()),
        ] {
            if let Some(action) = application
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(enabled);
            }
        }
    }

    /// Appends `options`, then the Zoom row, to the Appearance group.
    pub(super) fn append_zoom_row(&self, group: &gtk::Box, options: &gtk::Box) {
        group.append(options);
        widgets::settings_divider(group, 54);
        let zoom = self.ui_zoom();
        let stepper = hbox(6, &[]);
        let out = icon_button("zoom-out-symbolic", "Zoom Out", &[], self.on(App::zoom_out));
        out.set_sensitive(zoom.can_zoom_out());
        let level = text(&zoom.label(), &["settings-row-title"]);
        level.set_widget_name("zoom-level");
        level.set_width_chars(5);
        level.set_xalign(0.5);
        let zoom_in = icon_button("zoom-in-symbolic", "Zoom In", &[], self.on(App::zoom_in));
        zoom_in.set_sensitive(zoom.can_zoom_in());
        let reset = widgets::button("Reset", &[], self.on(App::zoom_reset));
        widgets::name(&reset, "Reset zoom");
        reset.set_sensitive(!zoom.is_actual_size());
        stepper.append(&out);
        stepper.append(&level);
        stepper.append(&zoom_in);
        // Reset goes under the stepper when the row is narrow.
        let controls = widgets::Reflow::new(&stepper, &reset, 6, 6, 0);
        controls.set_hexpand(false);
        controls.set_valign(gtk::Align::Center);
        let (row, _) = widgets::settings_row(
            "Zoom",
            "Make text and controls larger or smaller. Ctrl and the mouse wheel zoom too.",
            "zoom-in-symbolic",
            Some(controls.upcast_ref()),
            &[],
        );
        group.append(&row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_grows_with_the_zoom_but_fits_the_screen() {
        let zoom = UiZoom::from_percent(150);
        assert_eq!(fitted(MIN_WINDOW, zoom, None), (1470, 975));
        assert_eq!(
            fitted(MIN_WINDOW, UiZoom::from_percent(200), Some((1920, 1080))),
            (1920, 1080)
        );
        // A screen smaller than the window at 100% keeps today's size.
        assert_eq!(
            fitted(MIN_WINDOW, UiZoom::ACTUAL_SIZE, Some((800, 600))),
            MIN_WINDOW
        );
        assert_eq!(
            fitted(MIN_WINDOW, UiZoom::from_percent(125), Some((800, 600))),
            MIN_WINDOW
        );
    }

    #[test]
    fn a_zoomed_window_leaves_room_beside_the_taskbar() {
        let screen = usable(1920, 1080);
        assert_eq!(screen, (1632, 918));
        for percent in [175, 200] {
            let (width, height) = fitted(MIN_WINDOW, UiZoom::from_percent(percent), Some(screen));
            assert!(
                width < 1920 && height < 1080,
                "{percent}%: {width}x{height}"
            );
            assert!(
                height <= 1080 - 48 - 32,
                "{percent}%: a taskbar and a title bar fit"
            );
        }
        assert_eq!(
            fitted(MIN_WINDOW, UiZoom::from_percent(200), Some(screen)),
            (1632, 918)
        );
        assert_eq!(
            fitted(MIN_WINDOW, UiZoom::from_percent(125), Some(screen)),
            (1225, 813)
        );
        // The 100% minimum still holds on a small screen.
        assert_eq!(
            fitted(
                MIN_WINDOW,
                UiZoom::from_percent(150),
                Some(usable(1024, 700))
            ),
            (980, 650)
        );
    }

    #[test]
    fn ctrl_and_the_wheel_zoom_in_upwards_and_out_downwards() {
        assert_eq!(wheel_step(0.0, -1.0, true), (0.0, Some(ZoomStep::In)));
        assert_eq!(wheel_step(0.0, 1.0, true), (0.0, Some(ZoomStep::Out)));
        assert_eq!(wheel_step(0.0, -3.0, true), (0.0, Some(ZoomStep::In)));
        // Without Ctrl the wheel scrolls, and any travel is dropped.
        assert_eq!(wheel_step(0.0, -1.0, false), (0.0, None));
        assert_eq!(wheel_step(0.75, 1.0, false), (0.0, None));
    }

    #[test]
    fn a_touchpad_zooms_a_step_per_notch_of_travel() {
        let mut travel = 0.0;
        let mut steps = Vec::new();
        for _ in 0..8 {
            let (left, step) = wheel_step(travel, -0.25, true);
            travel = left;
            steps.extend(step);
        }
        assert_eq!(steps, [ZoomStep::In, ZoomStep::In]);
        assert_eq!(travel, 0.0);
        let (travel, step) = wheel_step(0.0, 0.5, true);
        assert_eq!((travel, step), (0.5, None));
        // Turning back cancels the travel rather than zooming the other way.
        assert_eq!(wheel_step(travel, -0.5, true), (0.0, None));
    }

    #[test]
    fn the_steps_walk_the_ladder() {
        let at = UiZoom::from_percent(125);
        assert_eq!(ZoomStep::In.from(at).percent(), 150);
        assert_eq!(ZoomStep::Out.from(at).percent(), 110);
        assert_eq!(ZoomStep::Reset.from(at), UiZoom::ACTUAL_SIZE);
        assert_eq!(ZoomStep::In.from(UiZoom::from_percent(200)).percent(), 200);
        assert_eq!(ZoomStep::Out.from(UiZoom::ACTUAL_SIZE).percent(), 100);
    }

    #[test]
    fn one_pixel_and_less_is_not_remembered() {
        assert!(!Base::Size(1, -1).scales());
        assert!(Base::Size(-1, 36).scales());
        assert!(!Base::Margins([0, 1, 0, 1]).scales());
        assert!(Base::Margins([0, 0, 0, 24]).scales());
        assert!(!Base::Spacing(0).scales());
        assert!(Base::PixelSize(14).scales());
    }

    #[test]
    fn every_zoom_action_has_a_shortcut() {
        let names: Vec<_> = ACTIONS.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, ["zoom-in", "zoom-out", "zoom-reset"]);
        assert!(ACTIONS[0].2.contains(&"<Control>plus"));
        assert!(ACTIONS[2].2.contains(&"<Control>KP_0"));
    }
}
