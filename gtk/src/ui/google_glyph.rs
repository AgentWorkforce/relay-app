//! The identity provider marks drawn by the sign-in buttons.

use gtk::prelude::*;
use gtk4 as gtk;

/// A plain four-arc approximation of the Google mark: no artwork files.
pub(super) fn google_glyph(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |_, context, _, _| {
        let size = size as f64;
        let line = size * 0.24;
        let radius = (size - line) / 2.0;
        let center = size / 2.0;
        context.set_line_width(line);
        let tau = 2.0 * std::f64::consts::PI;
        for (start, end, (red, green, blue)) in [
            (0.0, 0.16, (0.259, 0.522, 0.957)),
            (0.16, 0.45, (0.204, 0.659, 0.325)),
            (0.45, 0.66, (0.984, 0.737, 0.020)),
            (0.66, 0.95, (0.918, 0.263, 0.208)),
        ] {
            context.set_source_rgb(red, green, blue);
            context.arc(center, center, radius, start * tau, end * tau);
            let _ = context.stroke();
        }
        context.set_source_rgb(0.259, 0.522, 0.957);
        context.rectangle(center, center - line / 2.0, radius + line / 2.0, line);
        let _ = context.fill();
    });
    area
}

/// Microsoft's four-color square mark.
pub(super) fn microsoft_glyph(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |_, context, _, _| {
        let half = (f64::from(size) - 2.0) / 2.0;
        for (x, y, (r, g, b)) in [
            (0.0, 0.0, (0.953, 0.314, 0.133)),
            (half + 2.0, 0.0, (0.498, 0.729, 0.0)),
            (0.0, half + 2.0, (0.0, 0.643, 0.937)),
            (half + 2.0, half + 2.0, (1.0, 0.725, 0.0)),
        ] {
            context.set_source_rgb(r, g, b);
            context.rectangle(x, y, half, half);
            let _ = context.fill();
        }
    });
    area
}
