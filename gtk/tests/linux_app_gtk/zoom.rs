//! Whole-interface zoom: the window and the main pages at 100%, 150% and
//! 200%.

use super::*;
use agent_relay_app::prefs::UiZoom;
use agent_relay_app::ui::zoom::{self, MIN_WINDOW};

/// xvfb-run's default screen, which the CI runners use, and a small
/// laptop's. Every page must fit the window's minimum on it, whatever screen
/// the tests run on: a window manager may let the window grow past a small
/// screen, which hides a page wider than the window's minimum.
const SMALL_SCREEN: (i32, i32) = (1280, 1024);

/// Every Settings pane.
const PANES: [Pane; 8] = [
    Pane::General,
    Pane::Tools,
    Pane::Opening,
    Pane::About,
    Pane::Relay,
    Pane::Sharing,
    Pane::Account,
    Pane::Diagnostics,
];

/// The window keeps the scaled minimum size and holds the page's own
/// minimum without clipping it, here and in the window's minimum on a
/// [`SMALL_SCREEN`].
#[track_caller]
fn fits(case: &Case, zoom: UiZoom, page: &str) {
    let window = case.app.window();
    let (width, height) = zoom::fitted(MIN_WINDOW, zoom, zoom::screen(window));
    assert_eq!(
        window.size_request(),
        (width, height),
        "{page} at {}: the window's minimum follows the zoom",
        zoom.label()
    );
    let root = case.app.shell().unwrap().root.clone();
    let (min_width, _, _, _) = root.measure(gtk::Orientation::Horizontal, -1);
    let (min_height, _, _, _) = root.measure(gtk::Orientation::Vertical, window.width());
    assert!(
        window.width() >= width && window.height() >= height,
        "{page} at {}: window {}x{} is below its minimum {width}x{height}",
        zoom.label(),
        window.width(),
        window.height()
    );
    assert!(
        min_width <= window.width() && min_height <= window.height(),
        "{page} at {}: content needs {min_width}x{min_height}, window is {}x{}",
        zoom.label(),
        window.width(),
        window.height()
    );
    assert!(
        root.width() <= window.width() && root.height() <= window.height(),
        "{page} at {}: the page is wider or taller than the window",
        zoom.label()
    );
    let (small_width, small_height) = zoom::fitted(
        MIN_WINDOW,
        zoom,
        Some(zoom::usable(SMALL_SCREEN.0, SMALL_SCREEN.1)),
    );
    let (min_height, _, _, _) = root.measure(gtk::Orientation::Vertical, small_width);
    assert!(
        min_width <= small_width && min_height <= small_height,
        "{page} at {}: content needs {min_width}x{min_height}, the window's minimum on a {}x{} screen is {small_width}x{small_height}",
        zoom.label(),
        SMALL_SCREEN.0,
        SMALL_SCREEN.1
    );
}

fn sessions_and_settings_fit_at_every_zoom(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.sign_in();
    case.app.show_window();
    for percent in UiZoom::LEVELS {
        let zoom = UiZoom::from_percent(i64::from(percent));
        case.app.set_ui_zoom(zoom);
        assert_eq!(zoom::current(), zoom);
        case.app.go(Page::Sessions);
        case.settle();
        fits(&case, zoom, "Sessions");
        for pane in PANES {
            case.app.go(Page::Settings(pane));
            case.pump(|case| case.app.with_state(|state| state.settings.is_some()));
            case.settle();
            fits(&case, zoom, &format!("Settings > {}", pane.title()));
        }
        case.app.go(Page::Settings(Pane::General));
        case.settle();
        let column = case.app.shell().unwrap().settings.column.clone();
        let mut level = Vec::new();
        walk(column.upcast_ref(), &mut |widget| {
            if widget.widget_name() == "zoom-level" {
                if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                    level.push(label.clone());
                }
            }
        });
        assert_eq!(level.len(), 1, "Settings has one zoom level");
        assert_eq!(level[0].text(), format!("{percent}%"));
        let reset = buttons(&column, "Reset");
        assert_eq!(reset.len(), 1, "Settings has one Reset zoom button");
        assert_eq!(reset[0].is_sensitive(), percent != 100);
        assert_eq!(
            Preferences::load(case.prefs_path.clone()).ui_zoom(),
            zoom,
            "The zoom is remembered"
        );
    }
    case.app.zoom_reset();
    assert_eq!(case.app.ui_zoom(), UiZoom::ACTUAL_SIZE);
    assert_eq!(
        case.app.window().size_request(),
        zoom::fitted(
            MIN_WINDOW,
            UiZoom::ACTUAL_SIZE,
            zoom::screen(case.app.window())
        ),
        "Actual Size restores today's minimum"
    );
}

fn the_settings_zoom_buttons_step_the_ladder(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    case.sign_in();
    case.app.go(Page::Settings(Pane::General));
    case.pump(|case| case.app.with_state(|state| state.settings.is_some()));
    case.settle();
    let column = || case.app.shell().unwrap().settings.column.clone();
    let named = |tooltip: &str| {
        let mut found = Vec::new();
        walk(column().upcast_ref(), &mut |widget| {
            if widget.tooltip_text().as_deref() == Some(tooltip) {
                if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                    found.push(button.clone());
                }
            }
        });
        found
    };
    let zoom_out = named("Zoom Out");
    assert_eq!(zoom_out.len(), 1);
    assert!(
        !zoom_out[0].is_sensitive(),
        "100% is the bottom of the ladder"
    );
    named("Zoom In")[0].emit_clicked();
    case.settle();
    assert_eq!(case.app.ui_zoom().percent(), 110);
    named("Zoom In")[0].emit_clicked();
    case.settle();
    assert_eq!(case.app.ui_zoom().percent(), 125);
    named("Zoom Out")[0].emit_clicked();
    case.settle();
    assert_eq!(case.app.ui_zoom().percent(), 110);
    buttons(&column(), "Reset")[0].emit_clicked();
    case.settle();
    assert_eq!(case.app.ui_zoom(), UiZoom::ACTUAL_SIZE);
    assert_eq!(
        Preferences::load(case.prefs_path.clone()).ui_zoom(),
        UiZoom::ACTUAL_SIZE
    );
}

fn a_saved_zoom_is_applied_at_launch(runtime: &tokio::runtime::Runtime) {
    let saved = UiZoom::from_percent(150);
    let case = Case::seeded(
        runtime,
        |_| {},
        Arc::new(FakeService::default()),
        |prefs| {
            prefs.set_ui_zoom(saved).unwrap();
        },
    );
    assert_eq!(case.app.ui_zoom(), saved);
    assert_eq!(zoom::current(), saved);
    assert_eq!(zoom::px(100), 150);
    let window = case.app.window();
    assert_eq!(
        window.size_request(),
        zoom::fitted(MIN_WINDOW, saved, zoom::screen(window)),
        "The window opens at the saved zoom's minimum"
    );
    case.app.zoom_reset();
    assert_eq!(zoom::current(), UiZoom::ACTUAL_SIZE);
}

fn remembered_widgets_follow_a_zoom_change(_: &tokio::runtime::Runtime) {
    zoom::set_current(UiZoom::ACTUAL_SIZE);
    let column = zoom::boxed(gtk::Orientation::Vertical, 16);
    zoom::spacing(&column, 20);
    zoom::size(&column, 200, -1);
    zoom::margins(&column, 4, 8, 12, 16);
    let image = gtk::Image::from_icon_name("zoom-in-symbolic");
    zoom::pixel_size(&image, 16);
    assert_eq!(
        column.spacing(),
        20,
        "a later spacing wins over the box's own"
    );
    zoom::set_current(UiZoom::from_percent(150));
    assert_eq!(column.spacing(), 30);
    assert_eq!(column.size_request(), (300, -1));
    assert_eq!(
        (
            column.margin_top(),
            column.margin_end(),
            column.margin_bottom(),
            column.margin_start()
        ),
        (6, 12, 18, 24)
    );
    assert_eq!(image.pixel_size(), 24);
    zoom::set_current(UiZoom::ACTUAL_SIZE);
    assert_eq!(column.spacing(), 20);
    assert_eq!(column.size_request(), (200, -1));
    assert_eq!(column.margin_start(), 16);
    assert_eq!(image.pixel_size(), 16);
}

fn the_zoom_actions_have_shortcuts_and_stop_at_the_ends(runtime: &tokio::runtime::Runtime) {
    let case = Case::new(runtime);
    let application = gtk::Application::new(
        Some("com.agentrelay.ZoomTest"),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    application.register(gtk::gio::Cancellable::NONE).unwrap();
    case.app.window().set_application(Some(&application));
    case.app.install_actions(&application);
    for (name, accel) in [
        ("app.zoom-in", "<Control>plus"),
        ("app.zoom-out", "<Control>minus"),
        ("app.zoom-reset", "<Control>0"),
    ] {
        let accels = application.accels_for_action(name);
        assert!(
            accels.iter().any(|value| value.as_str() == accel),
            "{name} has {accel}: {accels:?}"
        );
    }
    let enabled = |name: &str| application.is_action_enabled(name);
    assert!(enabled("zoom-in"));
    assert!(
        !enabled("zoom-out") && !enabled("zoom-reset"),
        "100% is the bottom"
    );
    for _ in 0..10 {
        application.activate_action("zoom-in", None);
    }
    assert_eq!(case.app.ui_zoom().percent(), 200);
    assert!(!enabled("zoom-in"), "200% is the top");
    assert!(enabled("zoom-out") && enabled("zoom-reset"));
    application.activate_action("zoom-out", None);
    assert_eq!(case.app.ui_zoom().percent(), 175);
    application.activate_action("zoom-reset", None);
    assert_eq!(case.app.ui_zoom(), UiZoom::ACTUAL_SIZE);
    assert!(enabled("zoom-in") && !enabled("zoom-reset"));
    case.app.window().set_application(None::<&gtk::Application>);
}

fn the_text_resolution_follows_the_zoom(runtime: &tokio::runtime::Runtime) {
    let settings = gtk::Settings::default().unwrap();
    let desktop = settings.gtk_xft_dpi();
    let base = if desktop > 0 { desktop } else { 96 * 1024 };
    let case = Case::new(runtime);
    case.app.set_ui_zoom(UiZoom::from_percent(150));
    assert_eq!(
        settings.gtk_xft_dpi(),
        (f64::from(base) * 1.5).round() as i32
    );
    case.app.set_ui_zoom(UiZoom::from_percent(200));
    assert_eq!(
        settings.gtk_xft_dpi(),
        base * 2,
        "scaled from the desktop's, not from 150%"
    );
    case.app.zoom_reset();
    assert_eq!(
        settings.gtk_xft_dpi(),
        desktop,
        "100% restores the desktop's own"
    );
}

pub const TESTS: &[(&str, Test)] = &[
    (
        "sessions_and_settings_fit_at_every_zoom",
        sessions_and_settings_fit_at_every_zoom,
    ),
    (
        "the_settings_zoom_buttons_step_the_ladder",
        the_settings_zoom_buttons_step_the_ladder,
    ),
    (
        "a_saved_zoom_is_applied_at_launch",
        a_saved_zoom_is_applied_at_launch,
    ),
    (
        "remembered_widgets_follow_a_zoom_change",
        remembered_widgets_follow_a_zoom_change,
    ),
    (
        "the_zoom_actions_have_shortcuts_and_stop_at_the_ends",
        the_zoom_actions_have_shortcuts_and_stop_at_the_ends,
    ),
    (
        "the_text_resolution_follows_the_zoom",
        the_text_resolution_follows_the_zoom,
    ),
];
