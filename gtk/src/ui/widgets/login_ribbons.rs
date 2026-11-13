//! Native counterpart of the Mac's animated login ribbons.
use gtk::{cairo, glib, prelude::*};
use gtk4 as gtk;
use std::{cell::Cell, rc::Rc};

pub fn login_ribbons() -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_widget_name("login-ribbons");
    let elapsed = Rc::new(Cell::new(14.0));
    let drawing_time = elapsed.clone();
    area.set_draw_func(move |_, context, width, height| {
        draw(context, width as f64, height as f64, drawing_time.get());
    });
    let start = Cell::new(None);
    let last = Cell::new(0);
    let last_enabled = Cell::new(None);
    area.add_tick_callback(move |area, clock| {
        let now = clock.frame_time();
        let since = start.get().unwrap_or(now);
        start.set(Some(since));
        let enabled = area.settings().is_gtk_enable_animations();
        if (enabled && now - last.get() >= 33_333) || last_enabled.get() != Some(enabled) {
            elapsed.set(if enabled { 14.0 + (now - since) as f64 / 1_000_000.0 } else { 14.0 });
            area.queue_draw();
            last.set(now);
        }
        last_enabled.set(Some(enabled));
        glib::ControlFlow::Continue
    });
    area.upcast()
}

fn ribbon(x: f64, index: usize, aspect: f64, time: f64) -> f64 {
    let fi = index as f64;
    let converge = x * x * (3.0 - 2.0 * x);
    let wave = 0.07 * (x * aspect * 1.7 + time * 0.22 + fi * 1.3).sin()
        + 0.04 * (x * aspect * 3.3 - time * 0.17 + fi * 2.1).sin()
        + 0.05 * (noise(x * aspect * 1.2 + fi * 7.0, time * 0.08) - 0.5);
    (0.12 + fi * 0.095) * (1.0 - converge * 0.94) + 0.5 * converge * 0.94
        + wave * (1.0 - 0.9 * converge)
}

fn noise(x: f64, y: f64) -> f64 {
    let hash = |x: f64, y: f64| {
        let value = (x * 127.1 + y * 311.7).sin() * 43758.5453;
        value - value.floor()
    };
    let (i, j, fx, fy) = (x.floor(), y.floor(), x - x.floor(), y - y.floor());
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    (hash(i, j) * (1.0 - u) + hash(i + 1.0, j) * u) * (1.0 - v)
        + (hash(i, j + 1.0) * (1.0 - u) + hash(i + 1.0, j + 1.0) * u) * v
}

fn draw(context: &cairo::Context, width: f64, height: f64, time: f64) {
    context.set_source_rgb(0.012, 0.027, 0.05);
    let _ = context.paint();
    let haze = cairo::RadialGradient::new(width, height / 2.0, 0.0, width, height / 2.0, width.max(height));
    haze.add_color_stop_rgba(0.0, 0.08, 0.19, 0.29, 1.0);
    haze.add_color_stop_rgba(1.0, 0.08, 0.19, 0.29, 0.0);
    let _ = context.set_source(&haze);
    let _ = context.paint();
    let aspect = width / height.max(1.0);
    for index in 0..9 {
        let fi = index as f64;
        let mix = (fi * 0.37).fract();
        let (r, g, b) = if [2, 6].contains(&index) { (0.86, 0.55, 0.42) }
            else { (0.20 - 0.18 * mix, 0.50 + 0.30 * mix, 0.78 + 0.18 * mix) };
        let point = |x| (x * width, (1.0 - ribbon(x, index, aspect, time)) * height);
        for (line, alpha) in [(5.0, 0.08), (1.0, 0.36)] {
            context.new_path();
            let (x, y) = point(0.0);
            context.move_to(x, y);
            for step in 1..=128 {
                let (x, y) = point(step as f64 / 128.0);
                context.line_to(x, y);
            }
            context.set_source_rgba(r, g, b, alpha);
            context.set_line_width(line);
            let _ = context.stroke();
        }
        let speed = 0.07 + 0.025 * (fi * 0.73).fract();
        let x = (0.5 + time * speed - fi * 0.41).rem_euclid(1.0) / 0.9;
        if x < 1.0 {
            let (x, y) = point(x);
            let pulse = cairo::RadialGradient::new(x, y, 0.0, x, y, 12.0);
            pulse.add_color_stop_rgba(0.0, r, g, b, 0.85);
            pulse.add_color_stop_rgba(1.0, r, g, b, 0.0);
            let _ = context.set_source(&pulse);
            context.rectangle(x - 12.0, y - 5.0, 24.0, 10.0);
            let _ = context.fill();
        }
    }
}
