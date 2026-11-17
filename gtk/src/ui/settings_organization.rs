//! What Settings shows of the organization's workspace policy
//! (`settings.get`'s `organization_policy`): the note under each setting it
//! decides, and a locked setting that cannot be changed. The Mac's
//! `OrganizationNote` and `SettingsToggleRow` `organization`/`locked` match.

use super::app::App;
use super::settings_page::switch_accessory;
use super::widgets::{self, hbox, icon, settings_row, text, LabelStyle};
use crate::models::{organization_locked, organization_note};
use gtk::prelude::*;
use gtk4 as gtk;

/// The note under a setting the organization decides.
pub(in crate::ui) fn organization_note_widget(value: &str) -> gtk::Widget {
    let row = hbox(6, &[]);
    let glyph = icon("network-workgroup-symbolic", 12);
    glyph.add_css_class("dim-label");
    glyph.set_valign(gtk::Align::Start);
    row.append(&glyph);
    let words = text(value, &["dim-label", "caption"]).wrapped();
    words.set_hexpand(true);
    row.append(&words);
    row.set_margin_top(2);
    row.upcast()
}

/// A switch row as `pending_toggle_row`, with the organization's note under
/// it and its switch locked when the organization sets it.
pub(in crate::ui) fn organization_toggle_row(
    title: &str,
    detail: &str,
    icon_name: &str,
    active: bool,
    pending: bool,
    organization: (Option<&str>, bool),
    on: impl Fn(bool) + 'static,
) -> gtk::Box {
    let (note, locked) = organization;
    let switch = gtk::Switch::new();
    widgets::action_cursor(&switch);
    switch.set_active(active);
    switch.set_sensitive(!locked);
    widgets::name(&switch, title);
    switch.connect_active_notify(move |switch| on(switch.is_active()));
    let extra: Vec<gtk::Widget> = note.map(organization_note_widget).into_iter().collect();
    settings_row(
        title,
        detail,
        icon_name,
        Some(&switch_accessory(&switch, pending)),
        &extra,
    )
    .0
}

impl App {
    /// The organization's note on `key` and whether it locks it.
    pub(in crate::ui) fn organization(&self, key: &str) -> (Option<&'static str>, bool) {
        self.with_state(|state| {
            let policy = state
                .settings
                .as_ref()
                .and_then(|settings| settings.organization_policy.as_ref());
            (
                organization_note(policy, key),
                organization_locked(policy, key),
            )
        })
    }
}
