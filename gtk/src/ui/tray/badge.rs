//! The profile marker on the tray icon. A tray icon is a few dozen pixels
//! square, too small for the profile id to be read, so a profile build's
//! mark carries a dot in its lower right corner instead, cut free of the
//! mark; the id itself is in the item's title (Linux) or tooltip
//! (Windows), as on the window. The default profile's icon is unchanged.

use gtk4::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk4::glib;

/// The dot's colour: the accent blue, legible on light and dark bars.
pub const BADGE_RGB: [u8; 3] = [0x3b, 0x82, 0xf6];

/// The mark with the profile dot drawn in, when this build is a profile
/// build ([`Profile::badge`](crate::build_profile::Profile::badge)); the
/// mark as it is for the default profile.
pub fn profile_marked(pixbuf: Pixbuf) -> Pixbuf {
    if crate::build_profile::Profile::BUILT.badge().is_none() {
        return pixbuf;
    }
    badged(&pixbuf).unwrap_or(pixbuf)
}

/// `pixbuf` with the dot drawn in its lower right corner.
pub fn badged(pixbuf: &Pixbuf) -> Option<Pixbuf> {
    let pixbuf = pixbuf.add_alpha(false, 0, 0, 0).ok()?;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let stride = pixbuf.rowstride();
    let mut data = pixbuf.read_pixel_bytes().to_vec();
    draw_dot(&mut data, width as usize, height as usize, stride as usize);
    Some(Pixbuf::from_bytes(
        &glib::Bytes::from_owned(data),
        Colorspace::Rgb,
        true,
        8,
        width,
        height,
        stride,
    ))
}

/// Draws the dot into RGBA rows `stride` bytes apart: a disc a quarter of
/// the icon across in the lower right corner, with a clear ring around it
/// so it stands apart from the mark (and from the tinted mark, which keeps
/// only the alpha).
pub fn draw_dot(rgba: &mut [u8], width: usize, height: usize, stride: usize) {
    let size = width.min(height) as f64;
    let radius = size * 0.16;
    let ring = radius + size * 0.07;
    let centre_x = width as f64 - ring;
    let centre_y = height as f64 - ring;
    for row in 0..height {
        for column in 0..width {
            let dx = column as f64 + 0.5 - centre_x;
            let dy = row as f64 + 0.5 - centre_y;
            // Pixel coordinates are bounded by the icon; squared distances
            // avoid a libm hypot symbol newer than the supported glibc floor.
            let distance_squared = dx * dx + dy * dy;
            let at = row * stride + column * 4;
            let Some(pixel) = rgba.get_mut(at..at + 4) else {
                continue;
            };
            if distance_squared <= radius * radius {
                pixel.copy_from_slice(&[BADGE_RGB[0], BADGE_RGB[1], BADGE_RGB[2], 0xff]);
            } else if distance_squared <= ring * ring {
                pixel[3] = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dot_sits_in_the_lower_right_corner_cut_free_of_the_mark() {
        let (width, height) = (32, 32);
        let mut rgba = vec![0xffu8; width * height * 4];
        draw_dot(&mut rgba, width, height, width * 4);
        let pixel = |x: usize, y: usize| &rgba[(y * width + x) * 4..(y * width + x) * 4 + 4];
        // The dot's centre is the accent, opaque.
        assert_eq!(pixel(25, 25), &[0x3b, 0x82, 0xf6, 0xff]);
        // Between dot and mark the ring is clear.
        assert_eq!(pixel(25, 18)[3], 0);
        // The mark elsewhere is untouched.
        assert_eq!(pixel(4, 4), &[0xff; 4]);
        assert_eq!(pixel(31, 0), &[0xff; 4]);
        // Padded rows are drawn at their stride.
        let mut padded = vec![0xffu8; height * (width * 4 + 8)];
        draw_dot(&mut padded, width, height, width * 4 + 8);
        let at = 25 * (width * 4 + 8) + 25 * 4;
        assert_eq!(&padded[at..at + 4], &[0x3b, 0x82, 0xf6, 0xff]);
    }
}
