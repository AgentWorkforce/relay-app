//! The sign-in window's co-brand line: "for", the customer's mark fitted
//! into 16 points and the customer's name, read out as "for <name>"; and
//! the workspace header, which draws it for every co-branded build, beside
//! the wordmark in the badge's place or beside the name in text.

use super::*;
use agent_relay_app::build_profile::{CoBrand, MarkKind, Profile, DEFAULT_PRODUCT_NAME};
use agent_relay_app::ui::widgets::art::{cobrand_line_for, header_brand_for};
use gtk::{gdk, gdk_pixbuf};

pub const TESTS: &[(&str, Test)] = &[
    (
        "the_cobrand_line_reads_for_the_customer_with_its_mark",
        the_cobrand_line_reads_for_the_customer_with_its_mark,
    ),
    (
        "the_cobrand_line_leaves_out_an_undecodable_mark",
        the_cobrand_line_leaves_out_an_undecodable_mark,
    ),
    (
        "the_header_draws_the_cobrand_line_in_place_of_the_badge",
        the_header_draws_the_cobrand_line_in_place_of_the_badge,
    ),
];

/// A 32 by 16 mark, twice as wide as tall.
const SVG_MARK: &[u8] =
    b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 32 16\"><rect width=\"32\" height=\"16\"/></svg>";

extern "C" {
    /// GTK 4.0 API; gtk4-rs binds it only for GTK 4.10 and later. None when
    /// `accessible`'s `property` holds the value, else what it holds.
    fn gtk_test_accessible_check_property(
        accessible: *mut gtk::ffi::GtkAccessible,
        property: gtk::ffi::GtkAccessibleProperty,
        ...
    ) -> *mut std::ffi::c_char;
}

/// The accessible label `widget` is read out by, checked against `label`.
#[track_caller]
fn assert_read_out_as(widget: &gtk::Box, label: &str) {
    use glib::translate::ToGlibPtr;
    let expected = std::ffi::CString::new(label).unwrap();
    let widget: *mut gtk::ffi::GtkWidget = widget.upcast_ref::<gtk::Widget>().to_glib_none().0;
    // SAFETY: a live widget, which is a `GtkAccessible`, and the label
    // property's value as the NUL-terminated string the check reads; the
    // message it returns on a mismatch is ours to free.
    let mismatch = unsafe {
        let message = gtk_test_accessible_check_property(
            widget.cast(),
            gtk::ffi::GTK_ACCESSIBLE_PROPERTY_LABEL,
            expected.as_ptr(),
        );
        if message.is_null() {
            None
        } else {
            let text = std::ffi::CStr::from_ptr(message)
                .to_string_lossy()
                .into_owned();
            glib::ffi::g_free(message.cast());
            Some(text)
        }
    };
    assert_eq!(mismatch, None, "the line is read out as {label:?}");
}

/// The line's children, in order.
fn children(line: &gtk::Box) -> Vec<gtk::Widget> {
    let mut found = Vec::new();
    let mut child = line.first_child();
    while let Some(item) = child {
        child = item.next_sibling();
        found.push(item);
    }
    found
}

/// The "for" label and the name label, single-line and ellipsized at the
/// end, around whatever is between them.
#[track_caller]
fn assert_for_and_name(first: &gtk::Widget, last: &gtk::Widget) {
    let word = first
        .downcast_ref::<gtk::Label>()
        .expect("the line starts with a label");
    assert_eq!(word.text(), "for");
    let name = last
        .downcast_ref::<gtk::Label>()
        .expect("the line ends with a label");
    assert_eq!(name.text(), "Example Customer");
    assert!(name.is_single_line_mode(), "the name is one line");
    assert_eq!(name.ellipsize(), gtk::pango::EllipsizeMode::End);
}

/// The fitted texture the mark between "for" and the name draws.
#[track_caller]
fn mark_texture(mark: &gtk::Widget) -> gdk::Paintable {
    assert!(!mark.is::<gtk::Label>(), "the mark is a picture, not text");
    let mut textures = Vec::new();
    walk(mark, &mut |widget| {
        if let Some(picture) = widget.downcast_ref::<gtk::Picture>() {
            textures.extend(picture.paintable());
        }
    });
    assert_eq!(textures.len(), 1, "the mark draws one picture");
    textures.remove(0)
}

/// A PNG twice as wide as tall, built here so no binary fixture is checked in.
fn png_mark() -> &'static [u8] {
    let pixbuf = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 64, 32).unwrap();
    pixbuf.fill(0x3366ccff);
    Box::leak(
        pixbuf
            .save_to_bufferv("png", &[])
            .unwrap()
            .into_boxed_slice(),
    )
}

fn the_cobrand_line_reads_for_the_customer_with_its_mark(_: &tokio::runtime::Runtime) {
    for (mark_kind, mark) in [(MarkKind::Svg, SVG_MARK), (MarkKind::Png, png_mark())] {
        let cobrand = CoBrand {
            name: "Example Customer",
            mark_kind,
            mark,
        };
        let line = cobrand_line_for(&cobrand);
        assert_read_out_as(&line, "for Example Customer");
        let parts = children(&line);
        assert_eq!(
            parts.len(),
            3,
            "{mark_kind:?}: \"for\", the mark and the name"
        );
        assert_for_and_name(&parts[0], &parts[2]);
        // Fitted into 16 points at its own aspect, at the display's scale.
        let texture = mark_texture(&parts[1]);
        let scale = line.scale_factor().max(1);
        assert_eq!(
            (texture.intrinsic_width(), texture.intrinsic_height()),
            (16 * scale, 8 * scale),
            "{mark_kind:?}: the mark is fitted into its square"
        );
        // A second line draws the same fitted mark, decoded once.
        let again = children(&cobrand_line_for(&cobrand));
        assert_eq!(
            mark_texture(&again[1]),
            texture,
            "{mark_kind:?}: the fitted mark is kept"
        );
    }
}

fn the_cobrand_line_leaves_out_an_undecodable_mark(_: &tokio::runtime::Runtime) {
    for mark_kind in [MarkKind::Svg, MarkKind::Png] {
        let cobrand = CoBrand {
            name: "Example Customer",
            mark_kind,
            mark: b"not an image",
        };
        let line = cobrand_line_for(&cobrand);
        assert_read_out_as(&line, "for Example Customer");
        let parts = children(&line);
        assert_eq!(
            parts.len(),
            2,
            "{mark_kind:?}: \"for\" and the name, without the mark"
        );
        assert_for_and_name(&parts[0], &parts[1]);
    }
}

fn the_header_draws_the_cobrand_line_in_place_of_the_badge(_: &tokio::runtime::Runtime) {
    // A site profile's build named Agent Relay, badged "example-7".
    let plain = Profile {
        product_name: DEFAULT_PRODUCT_NAME,
        releases_repository: "example-org/example-desktop-releases",
        asset_prefix: "Desktop-example-7",
        release_feed: "github:example-org/example-desktop-releases",
        cobrand: None,
    };
    let header = header_brand_for(plain);
    let parts = children(&header);
    assert_eq!(parts.len(), 2, "the mark and the name, with no line");
    assert_eq!(labels(&parts[1]), ["Agent Relay", "example-7"]);

    let cobranded = Profile {
        cobrand: Some(CoBrand {
            name: "Example Customer",
            mark_kind: MarkKind::Svg,
            mark: SVG_MARK,
        }),
        ..plain
    };
    let header = header_brand_for(cobranded);
    let parts = children(&header);
    assert_eq!(parts.len(), 3, "the mark, the wordmark and the line");
    assert_eq!(
        labels(&parts[1]),
        Vec::<String>::new(),
        "the wordmark is drawn, not the name in text with its badge"
    );
    let line = parts[2]
        .downcast_ref::<gtk::Box>()
        .expect("the line is beside the wordmark");
    assert_read_out_as(line, "for Example Customer");
    let line_parts = children(line);
    assert_for_and_name(&line_parts[0], &line_parts[2]);
    // Like the mark and the wordmark, the line has no tooltip of its own:
    // the header's button is named "Sessions".
    assert!(parts.iter().all(|part| !part.has_tooltip()));

    // A branded, co-branded build: its name in text, with no badge, and the
    // line beside it, as on sign-in.
    let branded = Profile {
        product_name: "Example Desktop",
        ..cobranded
    };
    let header = header_brand_for(branded);
    let parts = children(&header);
    assert_eq!(parts.len(), 3, "the mark, the name and the line");
    assert_eq!(labels(&parts[1]), ["Example Desktop"]);
    let line = parts[2]
        .downcast_ref::<gtk::Box>()
        .expect("the line is beside the name");
    assert_read_out_as(line, "for Example Customer");
}
