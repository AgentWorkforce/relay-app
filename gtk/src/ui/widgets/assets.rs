//! The app's artwork, embedded at build time.

/// Single-color marks drawn in the text color, as the Mac's template images
/// are.
pub(super) const TEMPLATE_MARKS: [&str; 11] = [
    "copilot",
    "grok",
    "cursor",
    "opencode",
    "openai",
    "muse",
    "muse-assistant",
    "devin",
    "pi",
    "replicas",
    "amp",
];

/// The app's mark: the build's profile's (`DESKTOP_ICON`, `build.rs`), else
/// `assets/agent-relay-mark.svg`.
pub const MARK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mark.svg"));
pub const SYMBOLIC: &[u8] = include_bytes!("../../../resources/linux/agent-relay-symbolic.svg");
pub const WORDMARK: &[u8] = include_bytes!("../../../../assets/agent-relay-wordmark.svg");
/// The harness marks the Mac app bundles, by file name.
pub const AGENT_MARKS: [(&str, &[u8]); 20] = [
    (
        "copilot",
        include_bytes!("../../../resources/linux/agents/copilot.svg"),
    ),
    (
        "muse-assistant",
        include_bytes!("../../../resources/linux/agents/muse.svg"),
    ),
    (
        "replicas",
        include_bytes!("../../../resources/linux/agents/replicas.svg"),
    ),
    (
        "conductor",
        include_bytes!("../../../resources/linux/agents/conductor.png"),
    ),
    (
        "paseo",
        include_bytes!("../../../resources/linux/agents/paseo.png"),
    ),
    (
        "grokbot",
        include_bytes!("../../../resources/linux/agents/grokbot.png"),
    ),
    (
        "capy",
        include_bytes!("../../../resources/linux/agents/capy.svg"),
    ),
    (
        "claude",
        include_bytes!("../../../resources/linux/agents/claude.svg"),
    ),
    (
        "claudecode",
        include_bytes!("../../../resources/linux/agents/claudecode.svg"),
    ),
    (
        "codex",
        include_bytes!("../../../resources/linux/agents/codex.svg"),
    ),
    (
        "devin",
        include_bytes!("../../../resources/linux/agents/devin.svg"),
    ),
    (
        "openclaw",
        include_bytes!("../../../resources/linux/agents/openclaw.svg"),
    ),
    (
        "hermes",
        include_bytes!("../../../resources/linux/agents/hermes.svg"),
    ),
    (
        "pi",
        include_bytes!("../../../resources/linux/agents/pi.svg"),
    ),
    (
        "cursor",
        include_bytes!("../../../resources/linux/agents/cursor.svg"),
    ),
    (
        "grok",
        include_bytes!("../../../resources/linux/agents/grok.svg"),
    ),
    (
        "muse",
        include_bytes!("../../../resources/linux/agents/muse.svg"),
    ),
    (
        "openai",
        include_bytes!("../../../resources/linux/agents/openai.svg"),
    ),
    (
        "opencode",
        include_bytes!("../../../resources/linux/agents/opencode.svg"),
    ),
    // Amp's official logo (ampcode.com/logo-light.svg, from its press kit),
    // square-framed and drawn in currentColor; see
    // assets/agent-icons-LICENSE.txt.
    (
        "amp",
        include_bytes!("../../../resources/linux/agents/amp.svg"),
    ),
];
/// Single-color glyphs drawn in `currentColor`, named for the SF Symbols
/// the Mac draws in their place.
pub const GLYPHS: [(&str, &[u8]); 15] = [
    (
        "sparkles",
        include_bytes!("../../../resources/linux/glyphs/sparkles.svg"),
    ),
    (
        "square.and.pencil",
        include_bytes!("../../../resources/linux/glyphs/square.and.pencil.svg"),
    ),
    (
        "rectangle.split.3x1",
        include_bytes!("../../../resources/linux/glyphs/rectangle.split.3x1.svg"),
    ),
    (
        "checkmark.circle.fill",
        include_bytes!("../../../resources/linux/glyphs/checkmark.circle.fill.svg"),
    ),
    (
        "arrow.up.circle.fill",
        include_bytes!("../../../resources/linux/glyphs/arrow.up.circle.fill.svg"),
    ),
    (
        "clock",
        include_bytes!("../../../resources/linux/glyphs/clock.svg"),
    ),
    (
        "desktopcomputer",
        include_bytes!("../../../resources/linux/glyphs/desktopcomputer.svg"),
    ),
    (
        "tray.and.arrow.up",
        include_bytes!("../../../resources/linux/glyphs/tray.and.arrow.up.svg"),
    ),
    (
        "arrow.triangle.2.circlepath",
        include_bytes!("../../../resources/linux/glyphs/arrow.triangle.2.circlepath.svg"),
    ),
    (
        "pause.circle",
        include_bytes!("../../../resources/linux/glyphs/pause.circle.svg"),
    ),
    (
        "stop.circle",
        include_bytes!("../../../resources/linux/glyphs/stop.circle.svg"),
    ),
    (
        "exclamationmark.circle",
        include_bytes!("../../../resources/linux/glyphs/exclamationmark.circle.svg"),
    ),
    (
        "questionmark.circle",
        include_bytes!("../../../resources/linux/glyphs/questionmark.circle.svg"),
    ),
    (
        "folder",
        include_bytes!("../../../resources/linux/glyphs/folder.svg"),
    ),
    (
        "arrow.triangle.branch",
        include_bytes!("../../../resources/linux/glyphs/arrow.triangle.branch.svg"),
    ),
];

/// The bundled mark for a harness or source name.
pub fn mark_file(source: &str) -> Option<&'static str> {
    Some(match source.to_lowercase().as_str() {
        "capy" => "capy",
        "muse-assistant" => "muse-assistant",
        "conductor" => "conductor",
        "paseo" => "paseo",
        "replicas" => "replicas",
        "copilot" => "copilot",
        "codex" => "codex",
        "devin" => "devin",
        "openclaw" => "openclaw",
        "hermes" => "hermes",
        "pi" => "pi",
        "claude" | "claude-cloud" => "claude",
        "claude-code" | "claude_code" | "claudecode" => "claudecode",
        "grok" | "xai" => "grok",
        "grok-bot" => "grokbot",
        "cursor" | "cursor-cloud" | "cursor_cloud" => "cursor",
        "opencode" | "open-code" | "open_code" => "opencode",
        "dot" | "openai" | "open-ai" | "open_ai" => "openai",
        "muse" | "muse-open" => "muse",
        "opencode-open" => "opencode",
        "amp" => "amp",
        _ => return None,
    })
}

/// The solid tile a mark sits on in session lists: the provider's primary
/// colour behind its white mark. Providers whose artwork already carries
/// its own tile (Codex, Grok Bot) and the Claude Code crab, whose orange
/// is the mark itself, have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrandShape {
    Circle,
    RoundedSquare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkBrand {
    pub background: &'static str,
    pub shape: BrandShape,
    pub class: &'static str,
}

impl MarkBrand {
    pub fn shape_class(self) -> &'static str {
        match self.shape {
            BrandShape::Circle => "brand-circle",
            BrandShape::RoundedSquare => "brand-square",
        }
    }
}

/// The brand a badge draws with: the default badge and tile (tinted
/// `secondary`/`text`) sit on the provider's tile, while an explicit tint
/// (the sign-in splash) keeps its single-colour marks.
pub fn badge_brand(source: &str, tint: &str) -> Option<MarkBrand> {
    if tint == "secondary" || tint == "text" {
        mark_brand(source)
    } else {
        None
    }
}

pub fn mark_brand(source: &str) -> Option<MarkBrand> {
    Some(match source.to_lowercase().as_str() {
        "muse" | "muse-open" => MarkBrand {
            background: "#4091ff",
            shape: BrandShape::Circle,
            class: "brand-muse",
        },
        "grok" | "xai" => MarkBrand {
            background: "#000000",
            shape: BrandShape::Circle,
            class: "brand-grok",
        },
        "claude" => MarkBrand {
            background: "#d97757",
            shape: BrandShape::RoundedSquare,
            class: "brand-claude",
        },
        _ => return None,
    })
}

pub(super) fn mark_label(mark: &str) -> &'static str {
    match mark {
        "capy" => "Capy",
        "muse-assistant" => "Muse",
        "conductor" => "Conductor",
        "paseo" => "Paseo",
        "replicas" => "Replicas",
        "copilot" => "GitHub Copilot",
        "codex" => "Codex",
        "devin" => "Devin",
        "openclaw" => "OpenClaw",
        "hermes" => "Hermes",
        "pi" => "Pi",
        "claude" => "Claude",
        "claudecode" => "Claude Code",
        "grok" => "Grok",
        "grokbot" => "Grok Bot",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "openai" => "OpenAI",
        "amp" => "Amp",
        _ => "Muse Code",
    }
}

// The brand widgets built from these assets; the product name comes from
// the build's profile.
use crate::build_profile::{CoBrand, Profile, PRODUCT_NAME};
use gtk::{gdk, gdk_pixbuf, prelude::*};
use gtk4 as gtk;
use std::{cell::RefCell, collections::HashMap};

/// Each co-brand mark fitted into its square, by the mark's file and the
/// side in pixels; none for a mark the image loaders cannot read.
type FittedMarks = HashMap<(&'static [u8], i32), Option<gdk::Texture>>;

thread_local! {
    static COBRAND_MARKS: RefCell<FittedMarks> = RefCell::default();
}

/// The app's mark, `size` points square.
pub fn brand(size: i32) -> gtk::Widget {
    match super::svg_picture("agent-relay-mark", MARK, size, PRODUCT_NAME) {
        Some(mark) => {
            mark.set_halign(gtk::Align::Center);
            mark.upcast()
        }
        None => super::text(PRODUCT_NAME, &["headline"]).upcast(),
    }
}

/// The agentrelay wordmark of the sign-in lockup, `height` points tall; the
/// name in text when the SVG loader is missing, or when the build's profile
/// names another product (the wordmark is Agent Relay's). The lockup draws
/// the co-brand line ([`cobrand_line`]) under it, so a co-branded build
/// named Agent Relay draws the wordmark, without its badge, as macOS's
/// `BrandWordmarkView` with `coBranded`.
pub fn wordmark(height: i32) -> gtk::Widget {
    if !Profile::BUILT.draws_wordmark_over_cobrand_line() {
        return profile_name(Profile::BUILT).upcast();
    }
    match super::svg_picture("agent-relay-wordmark", WORDMARK, height, PRODUCT_NAME) {
        Some(picture) => picture.upcast(),
        None => super::text(PRODUCT_NAME, &["headline"]).upcast(),
    }
}

/// A profile's name in text, followed by its id (smaller, secondary) for any
/// profile but the default; read out as the window title.
fn profile_name(profile: Profile) -> gtk::Box {
    let name = super::hbox(6, &[]);
    let product = super::text(profile.product_name, &["headline"]);
    product.set_valign(gtk::Align::Baseline);
    name.append(&product);
    if let Some(badge) = profile.badge() {
        let id = super::text(badge, &["caption", "secondary"]);
        id.set_valign(gtk::Align::Baseline);
        name.append(&id);
    }
    super::name(&name, &profile.window_title());
    name
}

/// The sign-in window's lockup: the app's mark and the wordmark, read out
/// as "Sign in to Agent Relay", and under it a co-branded build's "for
/// <customer>" line ([`cobrand_line`]).
pub fn signin_lockup() -> gtk::Box {
    let lockup = super::vbox(0, &[]);
    let row = super::hbox(12, &[]);
    row.append(&brand(42));
    row.append(&wordmark(28));
    super::name(&row, "Sign in to Agent Relay");
    lockup.append(&row);
    if let Some(line) = cobrand_line() {
        crate::ui::zoom::margins(&line, 12, 0, 0, 0);
        lockup.append(&line);
    }
    lockup
}

/// The line a co-branded build draws under its Agent Relay lockup: "for",
/// the customer's mark fitted into 16 points, and the customer's name, read
/// out as "for <name>", as macOS's `CoBrandLineView`. None for a build
/// without a co-brand.
pub fn cobrand_line() -> Option<gtk::Box> {
    Profile::BUILT
        .cobrand
        .map(|cobrand| cobrand_line_for(&cobrand))
}

/// [`cobrand_line`] for `cobrand`.
pub fn cobrand_line_for(cobrand: &CoBrand) -> gtk::Box {
    let line = super::hbox(6, &["cobrand-line"]);
    line.set_halign(gtk::Align::Start);
    let word = super::text("for", &["secondary", "cobrand-for"]);
    word.set_valign(gtk::Align::Center);
    line.append(&word);
    // A mark the image loaders cannot read is left out, as on macOS.
    if let Some(mark) = cobrand_mark(cobrand, 16) {
        line.append(&mark);
    }
    let name = super::text(cobrand.name, &["cobrand-name"]);
    name.set_valign(gtk::Align::Center);
    name.set_single_line_mode(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&name);
    super::name(&line, &cobrand.accessible_label());
    line
}

/// The customer's mark, an SVG or a PNG, fitted into `size` points square
/// at its own aspect and sharp at the display's scale.
fn cobrand_mark(cobrand: &CoBrand, size: i32) -> Option<super::Sized> {
    let texture = fitted_mark(cobrand.mark, size * super::display_scale())?;
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(true);
    picture.set_keep_aspect_ratio(true);
    let sized = super::Sized::new(size, size, &picture);
    sized.set_valign(gtk::Align::Center);
    Some(sized)
}

/// `data` decoded once, fitted into a `side` pixels square at its own
/// aspect: the loader learns the mark's size before it decodes and is told
/// the fitted one then. Kept by `data` and `side` for the next lockup.
fn fitted_mark(data: &'static [u8], side: i32) -> Option<gdk::Texture> {
    if let Some(found) = COBRAND_MARKS.with(|marks| marks.borrow().get(&(data, side)).cloned()) {
        return found;
    }
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.connect_size_prepared(move |loader, width, height| {
        let (width, height) = fitted(width, height, side);
        loader.set_size(width, height);
    });
    let texture = loader
        .write(data)
        .and_then(|_| loader.close())
        .ok()
        .and_then(|_| loader.pixbuf())
        .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf));
    COBRAND_MARKS.with(|marks| marks.borrow_mut().insert((data, side), texture.clone()));
    texture
}

/// `width` by `height` fitted into a `side` square, keeping its aspect.
fn fitted(width: i32, height: i32, side: i32) -> (i32, i32) {
    let (width, height) = (width.max(1) as f64, height.max(1) as f64);
    let scale = side as f64 / width.max(height);
    (
        ((width * scale).round() as i32).max(1),
        ((height * scale).round() as i32).max(1),
    )
}

/// Canonical mark and wordmark lockup for the shared application header.
pub fn header_brand() -> gtk::Box {
    let lockup = super::hbox(10, &[]);
    lockup.append(&brand(24));
    let tint = super::color("text");
    let data = String::from_utf8_lossy(WORDMARK).replace("#FFFFFF", tint);
    let key = format!("header-wordmark-{tint}");
    let wordmark = (!Profile::BUILT.names_in_text())
        .then(|| super::svg_picture(&key, data.as_bytes(), 19, PRODUCT_NAME))
        .flatten();
    if let Some(wordmark) = wordmark {
        lockup.append(&wordmark);
    } else {
        lockup.append(&profile_name(Profile::BUILT));
    }
    // The Mac's logo has no tooltip; its button is named "Sessions".
    let mut child = lockup.first_child();
    while let Some(widget) = child {
        widget.set_has_tooltip(false);
        child = widget.next_sibling();
    }
    lockup
}

#[cfg(test)]
mod cobrand_tests {
    use super::fitted;

    #[test]
    fn a_mark_is_fitted_into_its_square_at_its_own_aspect() {
        assert_eq!(fitted(64, 64, 32), (32, 32));
        assert_eq!(fitted(200, 100, 16), (16, 8));
        assert_eq!(fitted(10, 40, 16), (4, 16));
        assert_eq!(fitted(1, 1000, 16), (1, 16));
        assert_eq!(fitted(0, 0, 16), (16, 16));
    }
}
