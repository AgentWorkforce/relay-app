//! Session sharing: the upload policy rows and the upload details. The
//! Mac's `GlobalSessionSharingSettings` matches it.

use crate::models::{SharingModeText, StatusView, SHARING_DISPLAY_ORDER};
use crate::ui::app::App;
use crate::ui::widgets::{
    self, hbox, icon, settings_divider, settings_row, settings_section, text, vbox, LabelStyle,
};
use agent_relay_desktop_core::contract::account::SharingMode;
use gtk::prelude::*;
use gtk4 as gtk;

/// Shown where a control the organization's policy sets was removed; the
/// Mac's `OrganizationPolicy.managed`.
pub(crate) const MANAGED_BY_ORGANIZATION: &str =
    agent_relay_desktop_core::history::operator_policy::MANAGED;

/// Under the upload policy rows while uploads wait for the operator's policy
/// to be read; the Mac's `GlobalSessionSharingSettings.withheldExplanation`.
const UPLOAD_POLICY_WITHHELD: &str =
    "Uploads wait until your organization's upload policy can be checked.";

impl App {
    pub(super) fn sharing_pane(&self, column: &gtk::Box) {
        let details = gtk::Expander::new(Some("Upload details"));
        details.set_expanded(self.with_state(|state| state.upload_details_open));
        let weak = self.downgrade();
        details.connect_expanded_notify(move |details| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| state.upload_details_open = details.is_expanded());
            }
        });
        details.set_child(Some(&self.stages_view()));
        let (status, preparing, shown_mode, pending_mode) = self.with_state(|state| {
            (
                state.status.clone().filter(|_| state.connected()),
                state.account().phase.as_deref() == Some("preparing_local_sessions"),
                state.sharing_mode(),
                state.sharing_mode_wanted.wanted().copied(),
            )
        });
        let Some(status) = status else {
            let (section, group) = settings_section("Upload policy", None);
            group.append(
                &settings_row(
                    if preparing {
                        "Finding your sessions"
                    } else {
                        "Checking uploads"
                    },
                    if preparing {
                        "Upload settings will be available when session preparation finishes."
                    } else {
                        "Upload settings appear once the uploader answers."
                    },
                    "send-to-symbolic",
                    None,
                    &[],
                )
                .0,
            );
            column.append(&section);
            column.append(&details);
            return;
        };
        let (section, group) = settings_section(
            "Upload policy",
            Some("Applies to all providers. Shared sessions are visible to your workspace. Sessions you put on the relay are always shared."),
        );
        let lock = match upload_policy_shown(&status) {
            UploadPolicyShown::Managed(mode) => {
                group.append(&enforced_policy_row(mode));
                settings_divider(&group, 0);
                group.append(&managed_line(16));
                column.append(&section);
                column.append(&details);
                return;
            }
            UploadPolicyShown::Picker(lock) => lock,
        };
        for (index, mode) in SHARING_DISPLAY_ORDER.into_iter().enumerate() {
            if index > 0 {
                settings_divider(&group, 52);
            }
            // The policy asked for is selected at once; its row spins until
            // the uploader answers, and every row stays clickable so a later
            // choice wins.
            let selected = shown_mode.unwrap_or(status.sharing_mode) == mode;
            let row = gtk::Button::new();
            row.set_sensitive(lock.is_none());
            crate::ui::widgets::action_cursor(&row);
            widgets::add_classes(
                &row,
                &["flat", "policy-row", if selected { "selected" } else { "" }],
            );
            let content = hbox(14, &[]);
            widgets::pad(&content, 10, None, None);
            let glyph = icon(
                if selected {
                    "emblem-ok-symbolic"
                } else {
                    "radio-symbolic"
                },
                18,
            );
            glyph.add_css_class(if selected { "accent" } else { "secondary" });
            glyph.set_valign(gtk::Align::Start);
            content.append(&glyph);
            let words = vbox(4, &[]);
            words.set_hexpand(true);
            words.append(&text(mode.title(), &["settings-row-title"]).wrapped());
            words.append(&text(mode.explanation(), &["settings-row-detail"]).wrapped());
            content.append(&words);
            if pending_mode == Some(mode) {
                let spinner = gtk::Spinner::new();
                spinner.set_valign(gtk::Align::Start);
                widgets::name(&spinner, "Changing the upload policy…");
                spinner.start();
                content.append(&spinner);
            }
            row.set_child(Some(&content));
            let change = self.on(move |app| app.change_sharing(mode));
            row.connect_clicked(move |_| change());
            group.append(&row);
        }
        if let Some(reason) = lock {
            settings_divider(&group, 0);
            let explanation = hbox(8, &[]);
            widgets::pad(&explanation, 16, None, None);
            let lock = icon("changes-prevent-symbolic", 14);
            lock.add_css_class("secondary");
            lock.set_valign(gtk::Align::Start);
            explanation.append(&lock);
            let words = text(reason, &["settings-row-detail"]).wrapped();
            words.set_hexpand(true);
            explanation.append(&words);
            group.append(&explanation);
        }
        column.append(&section);
        column.append(&details);
    }
}

/// What the upload policy section shows: the rows, locked while uploads wait
/// for the operator's policy, or, when the policy sets the mode, that mode
/// alone with no rows to choose. The Mac's
/// `GlobalSessionSharingSettings.Presentation`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum UploadPolicyShown {
    Picker(Option<&'static str>),
    Managed(SharingMode),
}

fn upload_policy_shown(status: &StatusView) -> UploadPolicyShown {
    if status.uploads_withheld {
        UploadPolicyShown::Picker(Some(UPLOAD_POLICY_WITHHELD))
    } else if let Some(mode) = status.enforced_sharing {
        UploadPolicyShown::Managed(mode)
    } else {
        UploadPolicyShown::Picker(None)
    }
}

/// The enforced upload policy, read-only: its title and explanation.
fn enforced_policy_row(mode: SharingMode) -> gtk::Box {
    let content = hbox(14, &[]);
    content.add_css_class("enforced-policy-row");
    widgets::pad(&content, 16, None, None);
    let glyph = icon("emblem-ok-symbolic", 18);
    glyph.add_css_class("accent");
    glyph.set_valign(gtk::Align::Start);
    content.append(&glyph);
    let words = vbox(4, &[]);
    words.set_hexpand(true);
    words.append(&text(mode.title(), &["settings-row-title"]).wrapped());
    words.append(&text(mode.explanation(), &["settings-row-detail"]).wrapped());
    content.append(&words);
    content
}

/// "Managed by your organization.", where the policy removed a control.
pub(crate) fn managed_line(padding: i32) -> gtk::Label {
    let line = text(MANAGED_BY_ORGANIZATION, &["settings-row-detail"]).wrapped();
    line.set_xalign(0.0);
    if padding > 0 {
        widgets::pad(&line, padding, None, None);
    }
    line
}
