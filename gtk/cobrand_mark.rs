//! What a co-brand mark is, by its content: `build.rs` embeds an SVG or a
//! PNG and refuses anything else (`tests/cobrand_mark.rs` checks it).

/// `png` for a PNG's signature, `svg` for data holding `<svg`, none for
/// anything else.
pub fn mark_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG") {
        Some("png")
    } else if String::from_utf8_lossy(data).contains("<svg") {
        Some("svg")
    } else {
        None
    }
}
