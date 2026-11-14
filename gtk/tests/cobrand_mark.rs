//! `build.rs` tells a co-brand mark's kind by its content, not its name.
#[path = "../cobrand_mark.rs"]
mod cobrand_mark;

use cobrand_mark::mark_kind;

#[test]
fn a_png_signature_is_a_png() {
    assert_eq!(mark_kind(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
}

#[test]
fn data_holding_an_svg_element_is_an_svg() {
    assert_eq!(
        mark_kind(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        Some("svg")
    );
    assert_eq!(
        mark_kind(
            b"<?xml version=\"1.0\"?>\n<!-- Example Customer -->\n<svg viewBox=\"0 0 8 8\"></svg>"
        ),
        Some("svg")
    );
}

#[test]
fn anything_else_is_neither() {
    assert_eq!(mark_kind(b""), None);
    assert_eq!(mark_kind(b"not an image"), None);
    assert_eq!(mark_kind(b"GIF89a"), None);
    assert_eq!(mark_kind(b"\x89PN"), None);
}
