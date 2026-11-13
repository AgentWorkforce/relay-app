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
use crate::build_profile::{Profile, PRODUCT_NAME};
use gtk::prelude::*;
use gtk4 as gtk;

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

/// The agentrelay wordmark, `height` points tall; the name in text when the
/// SVG loader is missing, or when the build's profile names another product
/// (the wordmark is Agent Relay's).
pub fn wordmark(height: i32) -> gtk::Widget {
    if Profile::BUILT.names_in_text() {
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
