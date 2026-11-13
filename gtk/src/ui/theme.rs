//! Light and dark, or whatever the desktop uses: the Mac's Appearance
//! setting.
//!
//! The window's regions and controls are styled from one palette per theme.
//! "Match system" follows the XDG desktop portal's `color-scheme` (GNOME,
//! KDE, Hyprland's portal and others publish it) and, without a portal, the
//! GTK theme's name. On Windows it follows the app mode in the registry
//! (`AppsUseLightTheme`), which Windows has no portal for.

use crate::prefs::{Appearance, UiZoom};
use gtk::{gdk, gio, glib, prelude::*};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A theme's colors, by the names the stylesheet uses.
pub struct Palette(pub [(&'static str, &'static str); 29]);

// Effective Cloud workspace tokens, from workspace-chrome.module.css.
// Keep these paired with macOS CloudTheme, including the light-theme 600 primary.
pub const LIGHT: Palette = Palette([
    ("window", "#f9fafb"),
    ("panel", "#ffffff"),
    ("panel_alt", "#f3f4f6"),
    ("card", "#ffffff"),
    ("border", "rgba(74, 144, 194, 0.16)"),
    ("text", "#111827"),
    ("secondary", "#4b5563"),
    ("tertiary", "#6b7280"),
    ("accent", "#2d6a9c"),
    ("accent_text", "#ffffff"),
    ("accent_soft", "#e7eff7"),
    ("hover", "#e7eff7"),
    ("pressed", "#e7eff7"),
    ("green", "#1d9f73"),
    ("message_connecting", "#aa7d12"),
    ("orange", "#c1674b"),
    ("red", "#d95b63"),
    ("input", "#f9fafb"),
    ("warn_soft", "rgba(193, 103, 75, 0.14)"),
    ("error_soft", "rgba(217, 91, 99, 0.14)"),
    ("selection", "#e7eff7"),
    ("switch_off", "#c9ddee"),
    ("switch_thumb", "#f9fafb"),
    ("hero", "#0a1a26"),
    ("sidebar", "#ffffff"),
    ("input_border", "#c9ddee"),
    ("ring", "#4a90c2"),
    ("code_bg", "#0d1723"),
    ("code_fg", "#dbe5f0"),
]);

pub const DARK: Palette = Palette([
    ("window", "#14191f"),
    ("panel", "#1a2129"),
    ("panel_alt", "#1d252e"),
    ("card", "#1a2129"),
    ("border", "rgba(168, 184, 200, 0.13)"),
    ("text", "#edf4fb"),
    ("secondary", "#a8b8c8"),
    ("tertiary", "#a8b8c8"),
    ("accent", "#74b8e2"),
    ("accent_text", "#08111a"),
    ("accent_soft", "#223e58"),
    ("hover", "#223e58"),
    ("pressed", "#223e58"),
    ("green", "#63d18b"),
    ("message_connecting", "#e5bf55"),
    ("orange", "#ce9178"),
    ("red", "#f0727f"),
    ("input", "#14191f"),
    ("warn_soft", "rgba(206, 145, 120, 0.14)"),
    ("error_soft", "rgba(240, 114, 127, 0.14)"),
    ("selection", "#223e58"),
    ("switch_off", "rgba(116, 184, 226, 0.32)"),
    ("switch_thumb", "#edf4fb"),
    ("hero", "#0a1a26"),
    ("sidebar", "#10151b"),
    ("input_border", "rgba(116, 184, 226, 0.32)"),
    ("ring", "#74b8e2"),
    ("code_bg", "#0d1723"),
    ("code_fg", "#dbe5f0"),
]);

/// The stylesheet, with `$name` for each palette color.
const TEMPLATE: &str = r#"
window, window.background, popover > contents { background-color: $window; color: $text; font-size: 13px; }
/* The Mac hides its title bar: GTK's own decoration is flat, in the window's color, with the window controls and no title. */
window > .titlebar, headerbar.default-decoration { background: $window; color: $text; border: none; box-shadow: none; min-height: 34px; padding: 0 4px; }
/* Signed in, the Mac's header runs to the top edge under the window controls: the decoration takes the header's color, so the two read as one 64-point band. */
window.workspace:not(.welcome) > .titlebar, window.workspace:not(.welcome) headerbar.default-decoration { background: $sidebar; }
headerbar.default-decoration .title, headerbar.default-decoration windowtitle { opacity: 0; }
windowcontrols button { background: transparent; border: none; box-shadow: none; border-radius: 6px; min-height: 24px; min-width: 24px; padding: 2px; margin: 0 1px; }
windowcontrols button:hover:not(:disabled) { background-color: $hover; }
windowcontrols button:active:not(:disabled) { background-color: $pressed; }
/* Over the signed-out split the decoration continues both halves: the brand panel's color, then the light sign-in panel's, whose controls are dark. */
window.welcome > .titlebar, window.welcome headerbar.default-decoration { background: linear-gradient(to right, #03070d 56.5%, #071016 56.5%); }
window.welcome windowcontrols button { color: #edf4fb; }
window.welcome windowcontrols button:hover:not(:disabled) { background-color: #172836; }
window.welcome windowcontrols button:active:not(:disabled) { background-color: #d9e6f2; }
tooltip { font-size: 13px; }
label { color: $text; }
label.secondary, .secondary, label.subtitle, label.muted { color: $secondary; }
label.tertiary { color: $tertiary; }
label.ok, .ok { color: $green; }
label.warn, .warn { color: $orange; }
label.error, .error { color: $red; }
label.accent { color: $accent; }
label.mono { font-family: monospace; }
separator { background-color: $border; min-width: 1px; min-height: 1px; }



.sidebar { background-color: $sidebar; }
.workspace-menu > button { min-height: 44px; padding: 0 10px; border-radius: 7px; background: transparent; border-color: transparent; }
.workspace-menu > button:hover { background-color: $hover; }
.workspace-badge { background-color: $accent_soft; color: $accent; border-radius: 8px; min-width: 32px; min-height: 32px; }
.section-title { font-size: 11px; font-weight: 500; color: $secondary; }
.sidebar button.sidebar-row { background: transparent; border: 0; border-radius: 7px; padding: 0 10px; min-height: 34px; box-shadow: none; }
.sidebar button.sidebar-row:hover:not(:disabled) { background-color: $hover; }
.sidebar button.sidebar-row.selected { background-color: $accent_soft; }
.sidebar button.sidebar-row.selected label.row-title { font-weight: 600; }
.sidebar button.sidebar-row image { color: $secondary; }
.sidebar button.sidebar-row.selected image { color: $accent; }
.count { font-size: 11px; color: $secondary; }
.badge { background-color: $accent; color: $accent_text; border-radius: 8px; padding: 0 6px; font-size: 10px; font-weight: 600; }

.topbar { min-height: 64px; }
button.header-account { padding: 5px; }
/* Full bleed, like the Mac's: the 24-point gutters are padding inside the band, not margins around it. */
.global-header { min-height: 64px; background: $sidebar; padding: 0 24px; }
.page-title { font-size: 15px; font-weight: 600; }
.page-scope { font-size: 15px; color: $secondary; }
.page-subtitle { font-size: 11px; color: $secondary; }

.statusbar { background-color: $panel; min-height: 28px; padding: 0 6px; }
.statusbar button { background: transparent; border: 0; box-shadow: none; padding: 0 8px; min-height: 22px; border-radius: 6px; }
.statusbar button.status-icon { padding: 0; min-width: 22px; }
.statusbar button:hover:not(:disabled) { background-color: $hover; }
.statusbar label { font-size: 11px; }

.dot { border-radius: 50%; min-width: 6px; min-height: 6px; }
.dot-green { background-color: $green; }
.dot-orange { background-color: $orange; }
.dot-accent { background-color: $accent; }
.dot-grey { background-color: $tertiary; }
.ring-green { border: 2px solid $green; border-radius: 50%; }
.ring-orange { border: 2px solid $orange; border-radius: 50%; }
.ring-accent { border: 2px solid $accent; border-radius: 50%; }
.ring-grey { border: 2px solid $tertiary; border-radius: 50%; }

button { background-image: none; background-color: $panel_alt; color: $text; border: 1px solid $border; border-radius: 7px; padding: 4px 12px; box-shadow: none; text-shadow: none; min-height: 24px; }
button:hover:not(:disabled) { background-color: $hover; }
button:active:not(:disabled), button:checked { background-color: $pressed; }
button:disabled { opacity: 0.5; }
/* A text button is 28 points overall, the Mac's control and primary height. */
button.text-button { padding-top: 2px; padding-bottom: 2px; min-height: 22px; }
button label { color: inherit; }
button.session-action { min-height: 18px; padding: 9px 12px; border-radius: 6px; }
button.session-action .agent-icon { min-width: 0; min-height: 0; }
button.destructive { color: $red; }
button.destructive label { color: $red; }
button.flat, button.icon { background: transparent; border-color: transparent; padding: 2px 6px; }
button.flat:hover:not(:disabled), button.icon:hover:not(:disabled) { background-color: $hover; }
button.link { background: transparent; border: 0; padding: 0; color: $accent; }
button.link label { color: $accent; }
button.link:hover:not(:disabled) label { text-decoration-line: underline; }

entry, textview.compose { background-color: $input; color: $text; border: 1px solid $input_border; border-radius: 7px; padding: 4px 8px; box-shadow: none; caret-color: $text; }
textview.compose text { background-color: $input; color: $text; }
entry:focus { border-color: $ring; }
entry:hover:not(:disabled) { border-color: $secondary; }
entry.search { font-size: 12px; }
entry selection, textview selection { background-color: $selection; color: $text; }
entry.search { border-radius: 7px; min-height: 26px; }

.row { border-radius: 8px; }
.row.hover { background-color: $hover; }
.row-title { font-size: 13px; }
.row-subtitle { font-size: 11px; color: $secondary; }
.agent-icon { background-color: $panel_alt; border-radius: 8px; min-width: 32px; min-height: 32px; font-size: 11px; font-weight: 700; color: $secondary; }
.section-header { font-size: 11px; font-weight: 600; color: $secondary; min-height: 26px; }
.section-detail { font-size: 11px; color: $secondary; }
.empty-note { font-size: 11px; color: $secondary; }
.detail-panel { background-color: $card; border-radius: 8px; }
.detail-label { font-size: 11px; color: $secondary; }
.detail-value { font-size: 11px; }
.overview { background-color: $accent_soft; border-radius: 7px; padding: 9px; }
.tool-heading { font-size: 11px; font-weight: 600; color: $secondary; }
.caption { font-size: 11px; color: $secondary; }

.banner { border-radius: 8px; padding: 8px; }
.banner label { font-size: 11px; }
.banner-warn { background-color: $warn_soft; }
.banner-neutral { background-color: $panel_alt; }

.card { background-color: $card; border: 1px solid $border; border-radius: 10px; padding: 16px; }
.settings-section-title { font-size: 13px; font-weight: 600; }
.settings-section-description { font-size: 12px; color: $secondary; }
.settings-group { background-color: $card; border: 1px solid $border; border-radius: 8px; }
.settings-icon { color: $secondary; min-width: 24px; min-height: 28px; }
.settings-profile-name { font-size: 15px; font-weight: 600; }
.settings-content button:not(.flat) { min-height: 32px; }
/* The Mac's settings actions are 32 points tall overall, 12-point medium labels: the height is the border box, not the content. */
.settings-content button.text-button:not(.flat) { min-height: 30px; padding-top: 0; padding-bottom: 0; font-size: 12px; font-weight: 500; }
.settings-content button:not(.flat):not(.primary) { border-radius: 6px; }
.settings-content button.text-button:not(.flat):not(.primary):not(.destructive) { background-color: $window; border-color: $input_border; }
.settings-content button.text-button:not(.flat):not(.primary):hover:not(:disabled) { background-color: $hover; }
.diagnostic-disclosure { background: $card; border: 1px solid $border; border-radius: 8px; padding: 16px; }
.settings-row-title { font-weight: 500; }
.settings-row-detail { font-size: 12px; color: $secondary; }
.policy-row { border-radius: 0; }
.policy-row.selected { background-color: $accent_soft; }
.appearance-option { background: transparent; border: 2px solid $border; border-radius: 10px; padding: 6px; }
.appearance-option.selected { border-color: $accent; }
.person-card { background-color: $card; border: 1px solid $border; border-radius: 10px; }
.presence-count { background-color: alpha($green, 0.15); border-radius: 9px; padding: 0 7px; font-size: 11px; }
.avatar { background-color: $accent_soft; color: $accent; border-radius: 50%; font-weight: 600; }
.progress-card { background-color: $card; border-radius: 10px; padding: 16px; }
.headline { font-size: 16px; font-weight: 600; }
.pill { background-color: $panel_alt; border-radius: 12px; padding: 2px 10px; font-size: 12px; }
/* A quiet card, not a band: the Mac's muted bar with a hairline border. */
.selection-bar { background-color: $panel_alt; border: 1px solid $border; border-radius: 8px; padding: 4px 6px 4px 12px; min-height: 32px; }
.selection-bar label { font-size: 12px; }
.selection-bar button.selection-action { min-height: 24px; padding: 0 10px; border-radius: 6px; }
image.accent { color: $accent; }
list.sessions, list.conversations { background-color: transparent; }
list.sessions > row { border-radius: 0; }
list.sessions > row.striped { background-color: $panel_alt; }
.local-session-group { padding: 12px 16px; margin-top: 22px; margin-bottom: 4px; background-color: mix($window, $accent, 0.07); border: 1px solid $border; border-left: 3px solid mix($border, $accent, 0.55); border-radius: 6px; }
list.conversations > row { border-radius: 8px; }
.local-session-group button.session-group-collapse:hover:not(:disabled) { background-color: $accent_soft; }
.local-session-group button.session-group-collapse:focus-visible { outline: 2px solid $accent; }
.title-action { min-width: 16px; min-height: 18px; padding: 2px; color: $secondary; }
.session-title-cell .title-action { min-height: 20px; padding: 0 3px; }
.session-header-title, entry.session-title-entry, entry.session-title-entry text { font-size: 13px; font-weight: 500; }
entry.session-title-entry, entry.session-title-entry:focus, entry.session-title-entry:focus-within { min-height: 0; padding: 0; margin: 0; border: 0; border-radius: 0; background: transparent; box-shadow: none; outline: none; }
entry.session-title-entry text { margin: 0; padding: 0; background: transparent; }
button.session-open { background: transparent; border: 0; box-shadow: none; padding: 0; border-radius: 4px; }
.session-activity label { font-size: 10px; }
button.session-open .row-title { font-weight: 500; }
button.session-open .agent-name { color: $text; font-weight: 500; }
button.session-open:hover:not(:disabled) { background: transparent; color: $text; }
button.session-open:active:not(:disabled) { background: transparent; }
button.session-open:focus-visible { outline: 2px solid $accent; }
.transcript-user { padding: 12px 20px; background-color: alpha($panel_alt, 0.6); border-radius: 16px; }
.transcript-author { font-size: 12px; font-weight: 600; color: $secondary; }
.transcript-tool-group > title { padding: 8px 0; font-size: 12px; color: $secondary; }
.transcript-text { font-size: 14px; line-height: 2; }
/* The session composer: one chat-bubble shell (Cloud's --radius-bubble-lg) around the growing input and Send, with one focus ring. */
.chat-composer { background-color: $card; border: 1px solid $border; border-radius: 20px 20px 20px 3px; padding: 6px 6px 6px 14px; }
.chat-composer.typing { border-color: $ring; box-shadow: inset 0 0 0 1px $ring; }
textview.chat-input, textview.chat-input text { background: transparent; color: $text; border: 0; outline: none; box-shadow: none; padding: 0; caret-color: $text; }
textview.chat-input { padding-top: 5px; padding-bottom: 5px; }
.presence-badge { border-radius: 9px; padding: 0 7px; font-size: 10px; font-weight: 600; color: $secondary; background-color: alpha($secondary, 0.14); }
.presence-badge.online { color: $green; background-color: alpha($green, 0.14); }
.transcript-latest-row { border-top: 1px solid $border; }
.transcript-latest { background-color: $panel_alt; border: 1px solid $border; border-radius: 18px; padding: 6px 12px; }
.transcript-activity { padding: 12px; background-color: $panel; border: 1px solid $border; border-radius: 10px; }
.transcript-activity > title:hover:not(:disabled) { background-color: $hover; }
.transcript-code-block { padding: 10px; background-color: $code_bg; border: 1px solid $border; border-radius: 8px; }
.transcript-code-block label { color: $code_fg; }
.transcript-code-block .caption { color: #7c8a9b; }
.transcript-code { font-size: 12px; padding: 6px; }
.transcript-heading { font-size: 14px; font-weight: 600; margin-top: 6px; }
.transcript-heading-one { font-size: 20px; }
.transcript-heading-two { font-size: 18px; }
.transcript-quote { border-left: 3px solid $accent; padding-left: 10px; color: $secondary; }

list.sessions > row:hover:not(:disabled), list.conversations > row:hover:not(:disabled) { background-color: $hover; }
list.sessions > row:selected, list.conversations > row:selected { background-color: $selection; color: $text; }
.table-header { border-bottom: 1px solid $border; padding: 0 10px 5px 10px; }
.session-column-resize { border-right: 2px solid $border; margin-top: 8px; margin-bottom: 8px; }
.session-column-resize:hover, .session-column-resize:focus { border-color: $accent; }
.session-sort-header { min-width: 0; min-height: 0; padding: 0; border: none; box-shadow: none; }
.table-header-title { font-size: 12px; font-weight: 500; color: $secondary; }
.table-divider { background-color: $border; min-width: 1px; margin-top: 2px; margin-bottom: 0; }
list.sessions > row.session-row { min-height: 52px; padding: 0; margin: 0 0; }
.session-row.session-hidden { opacity: 0.5; }
.session-row .agent-icon { min-width: 0; min-height: 0; }
list.sessions > row.session-row:hover:not(:disabled) { background-color: $hover; }
/* Selected rows keep their stripe and hover underneath a light accent tint (the Mac's 18% primary, a little lighter), so rows stay separate and text keeps its colors. */
list.sessions > row.session-row:selected { background-color: transparent; background-image: linear-gradient(alpha($accent, 0.14), alpha($accent, 0.14)); color: $text; }
list.sessions > row.session-row.striped:selected { background-color: $panel_alt; }
list.sessions > row.session-row:selected:hover:not(:disabled) { background-color: $hover; background-image: linear-gradient(alpha($accent, 0.1), alpha($accent, 0.1)); }
button.select-all-check { min-width: 16px; min-height: 16px; padding: 0; border-radius: 4px; background: alpha($text, 0.1); border: 1px solid alpha($text, 0.12); box-shadow: none; font-size: 10px; font-weight: 700; }
button.select-all-check label { color: transparent; }
button.select-all-check:hover:not(:disabled) { background: alpha($text, 0.16); }
button.select-all-check.checked, button.select-all-check.checked:hover:not(:disabled) { background: #2d6a9c; border-color: #2d6a9c; }
button.select-all-check.checked label { color: #ffffff; }
button.select-all-check:focus:not(:focus-visible), checkbutton:focus:not(:focus-visible), checkbutton:focus:not(:focus-visible) check { outline: none; }
button.select-all-check:focus-visible { outline: 2px solid $ring; outline-offset: 1px; }
.agent-tile { background-color: alpha($text, 0.045); border-radius: 8px; min-width: 32px; min-height: 32px; font-size: 11px; font-weight: 700; color: $secondary; }
.agent-icon.brand-muse, .agent-tile.brand-muse { background-color: #4091ff; color: #ffffff; }
.agent-icon.brand-grok, .agent-tile.brand-grok { background-color: #000000; color: #ffffff; border: 1px solid alpha($text, 0.14); }
.agent-icon.brand-claude, .agent-tile.brand-claude { background-color: #d97757; color: #ffffff; }
.agent-icon.brand-circle, .agent-tile.brand-circle { border-radius: 50%; }
.agent-icon.brand-square, .agent-tile.brand-square { border-radius: 6px; }
.session-title { font-size: 13px; font-weight: 500; }
.session-active { font-size: 12px; color: $secondary; }
.status-pill { border-radius: 999px; padding: 3px 8px; }
.status-pill .status-title { font-size: 11px; font-weight: 500; }
.status-pill.status-green { background-color: alpha($green, 0.12); }
.status-pill.status-green label { color: $green; }
.status-pill.status-accent { background-color: alpha($accent, 0.12); }
.status-pill.status-accent label { color: $accent; }
.status-pill.status-orange { background-color: alpha($orange, 0.12); }
.status-pill.status-orange label { color: $orange; }
.status-pill.status-secondary { background-color: alpha($secondary, 0.12); }
.status-pill.status-secondary.faint { background-color: alpha($secondary, 0.08); }
.status-pill.status-secondary label { color: $secondary; }
progressbar.mini trough, progressbar.mini progress { min-height: 3px; }
progressbar.mini.status-orange progress { background-color: $orange; }
progressbar.mini.status-secondary progress { background-color: $secondary; }
switch.mini { min-width: 28px; min-height: 16px; border-radius: 8px; }
switch.mini slider { min-width: 14px; min-height: 14px; margin: 1px; }

columnview, listview { background-color: $window; color: $text; }
columnview > listview > row:hover:not(:disabled) { background-color: $hover; }
columnview > listview > row:selected { background-color: $selection; color: $text; }
columnview > header > button { background-color: $window; color: $secondary; border: 0; border-bottom: 1px solid $border; border-radius: 0; font-size: 11px; font-weight: 600; padding: 6px 8px; }
scrolledwindow, scrolledwindow viewport { background-color: transparent; border: 0; }
scrolledwindow undershoot, scrolledwindow overshoot { background: none; }
popover.menu > contents { background-color: $window; color: $text; border: 1px solid $border; }
popover.menu modelbutton { padding: 6px 12px; }
popover.menu modelbutton:hover:not(:disabled) { background-color: $accent; color: $accent_text; }
popover.menu modelbutton:hover:not(:disabled) label { color: $accent_text; }
tooltip, tooltip.background { background-color: $panel_alt; color: $text; border: 1px solid $border; }
tooltip label { color: $text; }
progressbar trough { background-color: $panel_alt; border: 0; border-radius: 3px; min-height: 4px; }
progressbar progress { background-color: $accent; border: 0; border-radius: 3px; min-height: 4px; margin: 0; }
button.message-control { border-radius: 6px; padding: 6px 8px; min-height: 16px; border: 1px solid alpha($secondary, 0.3); background: alpha($secondary, 0.04); }
button.message-control.online { border-color: alpha($green, 0.3); background: alpha($green, 0.08); }
button.message-control.connecting { border-color: alpha($message_connecting, 0.3); background: alpha($message_connecting, 0.08); }
button.message-control .online { color: $green; }
button.message-control .connecting { color: $message_connecting; }
button.message-control .offline { color: $secondary; }
switch { background-color: $switch_off; border: 0; border-radius: 14px; box-shadow: none; }
switch:checked { background-color: #245580; }
switch:hover:not(:disabled), switch:focus-visible { outline: 2px solid $ring; outline-offset: 2px; }
switch:checked slider { background-color: $switch_thumb; }
switch slider { background-image: none; background-color: $switch_thumb; border: 0; border-radius: 50%; box-shadow: 0 1px 2px alpha(#000000, 0.25); min-width: 16px; min-height: 16px; }
dropdown > button { padding: 2px 8px; }
.cloud-select > button, button.cloud-control { background: $window; border: 1px solid $input_border; border-radius: 6px; font-size: 12px; min-height: 24px; padding: 1px 10px; box-shadow: none; }
.cloud-select > button:hover:not(:disabled), button.cloud-control:hover:not(:disabled), .cloud-option:hover:not(:disabled) { background: $hover; }
.session-search-bar { background-color: $window; border: 1px solid $border; border-radius: 7px; padding: 9px 14px; min-height: 26px; }
.bookmarked > button { color: $accent; }
.saved-view-delete { opacity: 0.6; }
.saved-view-row:hover .saved-view-delete, .saved-view-delete:focus { opacity: 1; }
.session-search-bar.search-open { border-color: $input_border; }
.session-search-bar .cloud-select > button { background: transparent; border: none; padding: 0; }
.session-filter-check { background-color: #2C6A9C; color: #ffffff; border-radius: 50%; padding: 3px; min-width: 12px; min-height: 12px; }
.session-filter-chip { background-color: $panel_alt; border: 1px solid $border; border-radius: 5px; padding: 4px 8px; min-height: 18px; font-size: 11px; box-shadow: none; }
.session-filter-chip:hover { background-color: $hover; }
button.session-filter-remove { background: transparent; border: none; box-shadow: none; padding: 0 2px; min-width: 18px; min-height: 20px; border-radius: 3px; }
button.session-filter-remove:hover { background-color: $hover; }
.session-popover > contents { background-color: $card; }
.animated-search > contents { transform-origin: 50% 0%; transform: scale(1); opacity: 1; transition: transform 200ms cubic-bezier(0.2, 0.8, 0.2, 1), opacity 200ms ease-out; }
.animated-search.search-collapsed > contents { transform: scale(0.98, 0.08); opacity: 0; }
.column-setting { min-height: 38px; padding: 0 8px; border: 1px solid $border; border-radius: 8px; background: $panel_alt; }
.column-setting.column-dragging { opacity: 0.3; }
.column-setting.column-drop-before { box-shadow: 0 -3px 0 $accent; }
.column-setting.column-drop-after { box-shadow: 0 3px 0 $accent; }
.cloud-select.flat > button { background: transparent; border-color: transparent; }
.cloud-select.flat > button:hover:not(:disabled) { background-color: $hover; }
.cloud-option { background: transparent; border: none; border-radius: 4px; box-shadow: none; padding: 2px 10px; min-height: 24px; font-size: 12px; }
.local-session-group .row-title { font-size: 14px; font-weight: 600; }
.group-count { background-color: $window; border: 1px solid $border; border-radius: 5px; padding: 3px 8px; font-size: 11px; font-weight: 500; color: $secondary; }
list.sessions > row.session-group-holder { background: transparent; }
list.sessions > row.session-group-holder:first-child .local-session-group { margin-top: 4px; }
check, radio { background-image: none; background-color: $input; border: 1px solid $tertiary; color: $accent_text; box-shadow: none; }
check { border-radius: 4px; }
/* Checked marks take the brand blue with a white tick in both themes, like the Mac's session checkboxes. */
check:checked, radio:checked { background-image: none; background-color: #2d6a9c; border-color: #2d6a9c; color: #ffffff; }
check:disabled, radio:disabled { opacity: 0.5; }
scrollbar { background-color: transparent; border: 0; }
scrollbar slider { background-color: alpha($secondary, 0.5); border-radius: 8px; min-width: 6px; min-height: 6px; }
expander title arrow { color: $secondary; }

.signin-splash { background-color: #03070d; padding: 0; }
.signin-splash label { color: #ffffff; }
.signin-splash .harness { background-color: alpha(#ffffff, 0.1); border-radius: 9px; min-width: 40px; min-height: 40px; font-weight: 700; color: #ffffff; }
.signin-panel { background-color: #071016; }
.signin-panel button.primary label { font-size: 15px; }
.signin-panel label { color: #edf4fb; }
.signin-panel label.secondary { color: #7c8a9b; }
.signin-panel label.error { color: #d93025; }
.signin-title { font-size: 28px; font-weight: 600; letter-spacing: -0.5px; }
.signin-subtitle { font-size: 15px; }
.signin-panel button { background-color: #f1f2f4; color: #1d1f23; border-color: #d5d8de; min-height: 40px; }
.signin-panel button label { color: #1d1f23; }
/* The panel is light in both themes, so its hover and press never take the dark palette's. */
.signin-panel button:hover:not(:disabled) { background-color: #e7eff7; }
.signin-panel button:active:not(:disabled), .signin-panel button:checked { background-color: #d9e6f2; }
.signin-panel button.link, .signin-panel button.link:hover:not(:disabled), .signin-panel button.link:active:not(:disabled) { background: transparent; border: 0; box-shadow: none; min-height: 0; padding: 0; }
.signin-panel button.link label { color: #7c8a9b; }
/* Canonical Agent Relay brand action, independent of workspace accent colors. */
button.primary { background: #2d6a9c; color: #ffffff; border: 1.5px solid #234969; border-radius: 10px 10px 10px 3px; font-size: 12px; font-weight: 600; box-shadow: none; text-shadow: none; }
button.primary label { color: #ffffff; }
button.primary:hover:not(:disabled) { background: #26557e; border-color: #234969; }
button.primary:active:not(:disabled) { background: shade(#26557e, 0.92); }
button.primary:focus-visible { outline: 2px solid $ring; outline-offset: 3px; }
/* Authentication buttons follow the website's white provider controls. */
.signin-panel button.provider-signin { background: #ffffff; color: #0d1a27; border: 1px solid alpha(#0d1a27, 0.14); border-radius: 12px 12px 12px 4px; padding: 0 12px; box-shadow: 0 1px 2px alpha(#0d1a27, 0.12); }
.signin-panel button.provider-signin label { color: #0d1a27; font-size: 15px; font-weight: 600; }
.signin-panel button.provider-signin:hover:not(:disabled) { background: #eef4f9; }
.signin-panel button.provider-signin:active:not(:disabled) { background: #e4edf5; }
.signin-panel button.workspace-choice { background: #13202b; border-color: alpha(#edf4fb, 0.18); border-radius: 8px; padding: 14px; }
.signin-panel button.workspace-choice label { color: #edf4fb; }
.signin-panel button.workspace-choice:hover:not(:disabled) { background: #1c3040; }
.signin-panel button.workspace-choice:active:not(:disabled) { background: #243d50; }
.signin-works-caption { font-family: monospace; font-size: 11px; letter-spacing: 0.4px; }
.signin-divider { min-height: 0; background: transparent; border-top: 1px dashed alpha(#7c8a9b, 0.35); }
.signin-legal button.link label { font-size: 11px; }
.loading-bar { background-color: #2d6a9c; border-radius: 2px; }
.avatar-picture { border-radius: 50%; }
.placeholder-title { font-size: 15px; font-weight: 600; }

/* Last, so no generic button rule above outranks them. */
/* The logo is a plain button on the Mac: it returns to Sessions and never shows a hover, pressed or focus box. */
button.header-brand, button.header-brand:hover, button.header-brand:hover:not(:disabled), button.header-brand:active, button.header-brand:active:not(:disabled), button.header-brand:checked, button.header-brand:focus, button.header-brand:focus-visible { background: transparent; background-color: transparent; border: none; box-shadow: none; outline: none; padding: 0; }
/* The search field is one rounded field: the magnifier and the placeholder are plain, never a box. */
.session-search-bar menubutton.search-icon > button, .session-search-bar menubutton.search-icon > button:hover:not(:disabled), .session-search-bar menubutton.search-icon > button:active:not(:disabled), .session-search-bar menubutton.search-icon > button:checked, .session-search-bar button.search-query, .session-search-bar button.search-query:hover:not(:disabled), .session-search-bar button.search-query:active:not(:disabled) { background: transparent; border: none; box-shadow: none; outline: none; }
.session-search-bar menubutton.search-icon > button { padding: 0; min-width: 20px; min-height: 26px; color: $secondary; }
.session-search-bar button.search-query { padding: 0 4px; min-height: 26px; font-size: 13px; }
.session-search-bar .search-shortcut { font-family: monospace; font-size: 11px; color: $secondary; border: 1px solid $border; border-radius: 4px; padding: 0 5px; }
/* The header's icon controls (saved views, Table display): 32-point squares, borderless until hovered. */
menubutton.header-icon > button { background: transparent; border: none; box-shadow: none; border-radius: 7px; padding: 0; min-width: 32px; min-height: 32px; }
menubutton.header-icon > button:hover:not(:disabled), menubutton.header-icon > button:checked { background-color: $hover; }
"#;

/// Light or dark, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// The stylesheet for `theme` at 100 percent.
pub fn css(theme: Theme) -> String {
    css_at(theme, UiZoom::ACTUAL_SIZE)
}

/// The stylesheet for `theme` with every size scaled to `zoom`.
pub fn css_at(theme: Theme, zoom: UiZoom) -> String {
    scale_pixels(&colored(theme), zoom)
}

/// The template with `theme`'s colors filled in, its sizes as written.
fn colored(theme: Theme) -> String {
    let palette = match theme {
        Theme::Light => &LIGHT,
        Theme::Dark => &DARK,
    };
    // Longer names first, so `$accent` never eats `$accent_soft`.
    let mut colors: Vec<_> = palette.0.iter().collect();
    colors.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut text = TEMPLATE.to_string();
    for (name, value) in colors {
        text = text.replace(&format!("${name}"), value);
    }
    text
}

/// Every `<number>px` in `css` scaled to `zoom` by [`UiZoom::scale`]: font
/// sizes, padding, margins, corners, minimum sizes, spacing and shadows.
/// Hairlines under 2px (borders, separators, letter spacing) stay as they
/// are, so lines stay crisp; at 100 percent the text comes back unchanged.
pub fn scale_pixels(css: &str, zoom: UiZoom) -> String {
    let mut scaled = String::with_capacity(css.len() + css.len() / 16);
    let mut rest = css;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let (before, from_digit) = rest.split_at(start);
        scaled.push_str(before);
        let number_end = from_digit
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(from_digit.len());
        let (number, after) = from_digit.split_at(number_end);
        let starts_a_value =
            !before.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '#');
        let is_pixels = after.strip_prefix("px").is_some_and(|unit_end| {
            !unit_end.starts_with(|c: char| c.is_alphanumeric() || c == '-')
        });
        match number.parse::<i32>() {
            Ok(pixels) if starts_a_value && is_pixels && pixels >= 2 => {
                scaled.push_str(&zoom.scale(pixels).to_string());
            }
            _ => scaled.push_str(number),
        }
        rest = after;
    }
    scaled.push_str(rest);
    scaled
}

/// Sets `settings`' `gtk-xft-dpi` to the desktop's own value times `zoom`;
/// at 100 percent it goes back to following the desktop.
fn follow_desktop_dpi(settings: &gtk::Settings, zoom: UiZoom) {
    settings.reset_property("gtk-xft-dpi");
    if zoom.is_actual_size() {
        return;
    }
    let desktop = settings.gtk_xft_dpi();
    let base = if desktop > 0 { desktop } else { 96 * 1024 };
    settings.set_gtk_xft_dpi((f64::from(base) * zoom.factor()).round() as i32);
}

/// What `color-scheme` says: 1 dark, 2 light, anything else no preference.
pub fn scheme_prefers_dark(value: u32) -> Option<bool> {
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

type ThemeChanged = Box<dyn Fn(Theme)>;

struct Inner {
    choice: Cell<Appearance>,
    zoom: Cell<UiZoom>,
    provider: gtk::CssProvider,
    /// The portal's answer; `None` without a portal or a preference.
    system_dark: Cell<Option<bool>>,
    /// The desktop's own GTK theme, read before this app replaces it.
    desktop_theme: String,
    applied: Cell<Option<Theme>>,
    on_change: RefCell<Option<ThemeChanged>>,
    #[cfg_attr(windows, allow(dead_code))]
    portal: RefCell<Option<gio::DBusProxy>>,
}

/// Applies the chosen theme and follows the system while it is "Match
/// system". Cheap to clone; every clone is the same appearance.
#[derive(Clone)]
pub struct Styling {
    inner: Rc<Inner>,
}

impl Styling {
    /// Installs the stylesheet on the default display and starts following
    /// the portal.
    pub fn start(choice: Appearance, zoom: UiZoom) -> Styling {
        let settings = gtk::Settings::default();
        let desktop_theme = settings
            .as_ref()
            .and_then(|settings| settings.gtk_theme_name())
            .map(|name| name.to_string())
            .unwrap_or_default();
        let provider = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let styling = Styling {
            inner: Rc::new(Inner {
                choice: Cell::new(choice),
                zoom: Cell::new(zoom),
                provider,
                system_dark: Cell::new(None),
                desktop_theme,
                applied: Cell::new(None),
                on_change: RefCell::new(None),
                portal: RefCell::new(None),
            }),
        };
        #[cfg(not(windows))]
        styling.watch_portal();
        #[cfg(windows)]
        styling.watch_windows();
        super::zoom::set_current(zoom);
        styling.apply();
        styling.apply_dpi();
        styling
    }

    /// Runs `on_change` after every change of the drawn theme.
    pub fn on_change(&self, on_change: impl Fn(Theme) + 'static) {
        *self.inner.on_change.borrow_mut() = Some(Box::new(on_change));
    }

    pub fn choice(&self) -> Appearance {
        self.inner.choice.get()
    }

    pub fn theme(&self) -> Theme {
        match self.inner.choice.get() {
            Appearance::Light => Theme::Light,
            Appearance::Dark => Theme::Dark,
            Appearance::System if self.system_prefers_dark() => Theme::Dark,
            Appearance::System => Theme::Light,
        }
    }

    pub fn set(&self, choice: Appearance) {
        self.inner.choice.set(choice);
        self.apply();
    }

    pub fn zoom(&self) -> UiZoom {
        self.inner.zoom.get()
    }

    /// Draws everything at `zoom`: the stylesheet, the desktop's font
    /// resolution and the sizes widgets were given in code.
    pub fn set_zoom(&self, zoom: UiZoom) {
        if self.inner.zoom.replace(zoom) != zoom {
            super::zoom::set_current(zoom);
            load_css(&self.inner.provider, &css_at(self.theme(), zoom));
            self.apply_dpi();
        }
    }

    /// `gtk-xft-dpi`, which sizes the text of controls the stylesheet does
    /// not give a size: the desktop's value (96 dpi without one) times the
    /// zoom. The desktop's value is read afresh each time, after dropping
    /// this app's own, so a change of the desktop's text scaling made since
    /// is followed; at 100 percent the desktop's value is left in charge.
    fn apply_dpi(&self) {
        if let Some(settings) = gtk::Settings::default() {
            follow_desktop_dpi(&settings, self.zoom());
        }
    }

    fn apply(&self) {
        let theme = self.theme();
        load_css(&self.inner.provider, &css_at(theme, self.zoom()));
        if let Some(settings) = gtk::Settings::default() {
            // Adwaita, which every GTK 4 ships, under this app's palette: the
            // desktop's own theme would show through in controls it draws.
            settings.set_gtk_theme_name(Some("Adwaita"));
            prefer_scheme(&settings, theme);
        }
        if self.inner.applied.replace(Some(theme)) != Some(theme) {
            if let Some(on_change) = self.inner.on_change.borrow().as_ref() {
                on_change(theme);
            }
        }
    }

    fn system_prefers_dark(&self) -> bool {
        self.inner
            .system_dark
            .get()
            .unwrap_or_else(|| self.inner.desktop_theme.to_lowercase().contains("dark"))
    }

    /// Windows' app mode (Settings > Personalization > Colors), read now and
    /// followed.
    #[cfg(windows)]
    fn watch_windows(&self) {
        self.inner.system_dark.set(windows_prefers_dark());
        let weak = Rc::downgrade(&self.inner);
        // A registry read every two seconds costs less than a thread
        // waiting on `RegNotifyChangeKeyValue`, and follows the switch as
        // promptly as the person notices.
        glib::timeout_add_seconds_local(2, move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let dark = windows_prefers_dark();
            if inner.system_dark.replace(dark) != dark {
                let styling = Styling { inner };
                if styling.choice() == Appearance::System {
                    styling.apply();
                }
            }
            glib::ControlFlow::Continue
        });
    }

    /// `org.freedesktop.appearance color-scheme`, read now and followed.
    /// Without a portal the GTK theme's name decides.
    #[cfg(not(windows))]
    fn watch_portal(&self) {
        let Ok(portal) = gio::DBusProxy::for_bus_sync(
            gio::BusType::Session,
            gio::DBusProxyFlags::NONE,
            None,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            gio::Cancellable::NONE,
        ) else {
            return;
        };
        if let Ok(value) = portal.call_sync(
            "Read",
            Some(&("org.freedesktop.appearance", "color-scheme").to_variant()),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        ) {
            self.inner.system_dark.set(scheme_of(&value));
        }
        let weak = Rc::downgrade(&self.inner);
        portal.connect_local("g-signal", false, move |values| {
            let signal = values.get(2)?.get::<String>().ok()?;
            let parameters = values.get(3)?.get::<glib::Variant>().ok()?;
            let inner = weak.upgrade()?;
            if signal != "SettingChanged" || parameters.n_children() < 3 {
                return None;
            }
            let namespace = parameters.child_value(0).str().map(str::to_owned);
            let key = parameters.child_value(1).str().map(str::to_owned);
            if namespace.as_deref() == Some("org.freedesktop.appearance")
                && key.as_deref() == Some("color-scheme")
            {
                inner.system_dark.set(scheme_of(&parameters.child_value(2)));
                let styling = Styling { inner };
                if styling.choice() == Appearance::System {
                    styling.apply();
                }
            }
            None
        });
        *self.inner.portal.borrow_mut() = Some(portal);
    }
}

/// Tells GTK's own drawing which scheme to use: `gtk-interface-color-scheme`
/// where GTK has it (4.20 and later), else the dark-theme preference it
/// replaces.
fn prefer_scheme(settings: &gtk::Settings, theme: Theme) {
    const SCHEME: &str = "gtk-interface-color-scheme";
    let nick = match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    };
    let scheme = settings
        .find_property(SCHEME)
        .filter(|spec| spec.flags().contains(glib::ParamFlags::WRITABLE))
        .and_then(|spec| glib::EnumClass::with_type(spec.value_type()))
        .and_then(|class| class.to_value_by_nick(nick));
    match scheme {
        Some(value) => settings.set_property_from_value(SCHEME, &value),
        None => settings.set_gtk_application_prefer_dark_theme(theme == Theme::Dark),
    }
}

/// Loads `css` into `provider`.
pub fn load_css(provider: &gtk::CssProvider, css: &str) {
    provider.load_from_data(css);
}

/// Whether Windows' app mode is dark: `AppsUseLightTheme` is 0 under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`;
/// `None` where the value is missing (before Windows 10 1809).
#[cfg(windows)]
fn windows_prefers_dark() -> Option<bool> {
    use agent_relay_desktop_core::install::windows::registry;
    registry::read_dword(
        registry::Root::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "AppsUseLightTheme",
    )
    .map(|light| light == 0)
}

/// A `u32` however deeply the portal wraps it in variants and tuples.
#[cfg(not(windows))]
fn scheme_of(value: &glib::Variant) -> Option<bool> {
    let mut value = value.clone();
    loop {
        if let Some(number) = value.get::<u32>() {
            return scheme_prefers_dark(number);
        }
        if let Some(inner) = value.as_variant() {
            value = inner;
        } else if value.is_container() && value.n_children() > 0 {
            value = value.child_value(0);
        } else {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_100_percent_the_stylesheet_is_unchanged() {
        for theme in [Theme::Light, Theme::Dark] {
            assert_eq!(css(theme), colored(theme), "{theme:?}");
            assert_eq!(css_at(theme, UiZoom::ACTUAL_SIZE), colored(theme));
        }
        let sample = "a { padding: 0 6px; border: 1px solid #12px; margin: -0.5px; x: 1.5px; }";
        assert_eq!(scale_pixels(sample, UiZoom::ACTUAL_SIZE), sample);
    }

    #[test]
    fn at_150_percent_every_size_is_half_again_in_whole_pixels() {
        let zoom = UiZoom::from_percent(150);
        assert_eq!(
            scale_pixels(
                "window { font-size: 13px; min-height: 34px; padding: 0 6px; border-radius: 999px; }",
                zoom
            ),
            "window { font-size: 20px; min-height: 51px; padding: 0 9px; border-radius: 1499px; }"
        );
        // Hairlines, fractions, colors and names keep what they say.
        let kept = "a { border: 1px solid #222px; outline-offset: -1px; letter-spacing: 0.2px; b2px: 4pxa; }";
        assert_eq!(scale_pixels(kept, zoom), kept);
        let light = css_at(Theme::Light, zoom);
        assert!(light.contains("font-size: 20px"), "13px fonts become 20px");
        assert!(!light.contains("font-size: 13px"));
        assert!(light.contains("1px solid"), "borders stay one pixel");
        assert_eq!(
            light.matches("px").count(),
            css(Theme::Light).matches("px").count()
        );
    }

    #[test]
    fn every_palette_color_is_filled_in() {
        for theme in [Theme::Light, Theme::Dark] {
            let text = css(theme);
            assert!(!text.contains('$'), "{theme:?} leaves a $name");
        }
        assert!(css(Theme::Dark).contains("#14191f"));
        assert!(css(Theme::Light).contains("#ffffff"));
    }

    #[test]
    fn primary_buttons_keep_brand_color_and_tail_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let css = css(theme);
            let primary = css
                .split("button.primary {")
                .nth(1)
                .unwrap()
                .split('}')
                .next()
                .unwrap();
            assert!(primary.contains("background: #2d6a9c;"));
            assert!(primary.contains("color: #ffffff;"));
            assert!(primary.contains("border: 1.5px solid #234969;"));
            assert!(primary.contains("border-radius: 10px 10px 10px 3px;"));
            assert!(primary.contains("box-shadow: none;"));
        }
    }

    /// The rule that sets `selector`'s own declarations, wherever it sits in
    /// a selector list.
    fn rules_for<'a>(css: &'a str, selector: &str) -> Vec<&'a str> {
        css.lines()
            .filter(|line| {
                line.split('{')
                    .next()
                    .is_some_and(|selectors| selectors.split(',').any(|s| s.trim() == selector))
            })
            .collect()
    }

    #[test]
    fn the_logo_never_shows_a_hover_pressed_or_focus_box() {
        for theme in [Theme::Light, Theme::Dark] {
            let css = css(theme);
            let brand = css
                .find("button.header-brand:hover:not(:disabled)")
                .unwrap();
            // GTK takes the later of two equally specific rules, so the
            // brand's must follow every generic button hover.
            for generic in [
                "button:hover:not(:disabled)",
                "button.flat:hover:not(:disabled)",
                "button:active:not(:disabled), button:checked",
            ] {
                assert!(
                    css.find(generic).unwrap() < brand,
                    "{generic} after the brand"
                );
            }
            for state in [
                "button.header-brand:hover:not(:disabled)",
                "button.header-brand:active:not(:disabled)",
                "button.header-brand:focus-visible",
            ] {
                let rules = rules_for(&css, state);
                assert_eq!(rules.len(), 1, "{state}");
                assert!(rules[0].contains("background-color: transparent;"));
                assert!(rules[0].contains("outline: none;"));
                assert!(rules[0].contains("box-shadow: none;"));
            }
        }
    }

    #[test]
    fn the_search_icon_is_plain_inside_one_field() {
        let css = css(Theme::Light);
        let hover = rules_for(
            &css,
            ".session-search-bar menubutton.search-icon > button:hover:not(:disabled)",
        );
        assert_eq!(hover.len(), 1);
        assert!(hover[0].contains("background: transparent;"));
        assert!(hover[0].contains("border: none;"));
        assert!(
            css.find(".cloud-select.flat > button:hover:not(:disabled)")
                .unwrap()
                < css
                    .find(".session-search-bar menubutton.search-icon > button:hover")
                    .unwrap()
        );
        assert!(css.contains(
            ".global-header { min-height: 64px; background: #ffffff; padding: 0 24px; }"
        ));
    }

    #[test]
    fn selection_tints_rows_and_the_bar_is_a_quiet_card() {
        for theme in [Theme::Light, Theme::Dark] {
            let css = css(theme);
            let selected = rules_for(&css, "list.sessions > row.session-row:selected");
            assert_eq!(selected.len(), 1);
            // A translucent tint over the stripe, never a solid fill.
            assert!(selected[0].contains("background-color: transparent;"));
            assert!(selected[0].contains(", 0.14)"));
            assert!(
                !rules_for(&css, "list.sessions > row.session-row.striped:selected").is_empty()
            );
            let bar = rules_for(&css, ".selection-bar");
            assert_eq!(bar.len(), 1);
            assert!(bar[0].contains("border-radius: 8px;"));
            assert!(bar[0].contains("border: 1px solid"));
            assert!(!bar[0].contains("#e7eff7") && !bar[0].contains("#223e58"));
            let checked = rules_for(&css, "check:checked");
            assert!(checked[0].contains("background-color: #2d6a9c;"));
            let unfocused = rules_for(&css, "checkbutton:focus:not(:focus-visible) check");
            assert!(unfocused[0].contains("outline: none;"));
        }
    }

    #[test]
    fn the_portal_scheme_reads_as_dark_light_or_none() {
        assert_eq!(scheme_prefers_dark(1), Some(true));
        assert_eq!(scheme_prefers_dark(2), Some(false));
        assert_eq!(scheme_prefers_dark(0), None);
    }
}
