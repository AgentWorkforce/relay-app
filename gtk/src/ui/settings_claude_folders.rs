//! Claude Code › Sharing: the Claude Code config folders whose sessions this
//! computer finds and uploads, and the cloud-sessions opt-in.

use super::app::App;
use super::desktop::AppNotice;
use super::settings_page::SettingRow;
use super::state::{Page, Pane};
use super::widgets::{
    self, button, hbox, notice, settings_divider, settings_row, settings_section, text, vbox,
    LabelStyle, Tone,
};
use crate::models::{
    claude_cloud_health_detail, claude_cloud_health_label, claude_code_sign_in_offered,
    claude_folder_detail, claude_folder_display_path, claude_folder_removable,
    claude_folders_adding, claude_folders_removing, claude_sign_in_notice,
    CLAUDE_CLOUD_SESSIONS_DETAIL, CLAUDE_SIGN_IN_NOTICE_ACTION, CLAUDE_SIGN_IN_NOTICE_BODY,
    CLAUDE_SIGN_IN_NOTICE_TITLE, CLAUDE_SIGN_IN_NOTIFIED_KEY,
    CLAUDE_CLOUD_SESSIONS_TITLE, CLAUDE_CODE_SIGN_IN, CLAUDE_CODE_SIGN_IN_HELP, CLAUDE_FOLDERS_ADD,
    CLAUDE_FOLDERS_ADD_HELP, CLAUDE_FOLDERS_DETAIL, CLAUDE_FOLDERS_NOT_SHARED,
    CLAUDE_FOLDERS_REMOVE, CLAUDE_FOLDERS_SHARE_HELP, CLAUDE_FOLDERS_TITLE,
};
use agent_relay_desktop_core::contract::account::ClaudeCloudHealth;
use agent_relay_node::relay::contract::agents::{
    ClaudeConfigDir, ClaudeConfigDirChange, SettingsChange,
};
use agent_relay_node::relay::contract::NoParams;
use gtk::{gio, prelude::*};
use gtk4 as gtk;

const CLAUDE_SHARING_TITLE: &str = "Sharing";
/// The app actions the Claude Code sign-in notification's button and body
/// run.
const CLAUDE_SIGN_IN_ACTION: &str = "claude-code-sign-in";
const CLAUDE_SETTINGS_ACTION: &str = "claude-code-settings";
const FOLDERS_KEY: &str = "provider.claude.folders";

/// The widget name of the Claude cloud-session health line, which an
/// uploader update compares before drawing Claude's page again.
const CLAUDE_CLOUD_HEALTH_NAME: &str = "claude-cloud-health";
/// The widget name of "Sign in to Claude Code", whose coming or going an
/// uploader update also redraws for.
const CLAUDE_CODE_SIGN_IN_NAME: &str = "claude-code-sign-in";

/// What Claude's page shows under cloud sessions: the health line, and
/// whether "Sign in to Claude Code" is offered.
type ClaudeCloudShown = (String, bool);

/// The health line under Claude Code cloud sessions: its state, then why and
/// when it last worked.
fn claude_cloud_health_line(health: Option<&ClaudeCloudHealth>) -> String {
    format!(
        "{} · {}",
        claude_cloud_health_label(health),
        claude_cloud_health_detail(health, &chrono::Local)
    )
}

/// How many watched folders are shared, in a few words, as on the Mac.
fn claude_folders_summary(folders: &[ClaudeConfigDir]) -> String {
    let shared = folders.iter().filter(|folder| folder.shared).count();
    match (folders.len(), shared) {
        (0, _) => "No folders found".into(),
        (_, 0) => CLAUDE_FOLDERS_NOT_SHARED.into(),
        (1, 1) => "Shared".into(),
        (all, shared) if all == shared => format!("All {all} shared"),
        (all, shared) => format!("{shared} of {all} shared"),
    }
}

impl App {
    /// The health line Claude's page should show now, and whether it offers
    /// Claude Code's sign-in; `None` while cloud sessions are off or the
    /// folders are not reported yet, when no line is drawn.
    fn claude_cloud_health_wanted(&self) -> Option<ClaudeCloudShown> {
        self.with_state(|state| {
            let settings = state.settings.as_ref()?;
            settings.claude_config_dirs.as_ref()?;
            settings.claude_cloud_sessions.filter(|on| *on)?;
            let health = state
                .status
                .as_ref()
                .and_then(|status| status.claude_cloud.clone());
            Some((
                claude_cloud_health_line(health.as_ref()),
                claude_code_sign_in_offered(health.as_ref()),
            ))
        })
    }

    /// Draws Claude's provider page again after an uploader update only when
    /// the Claude cloud-session health line, or whether it offers Claude
    /// Code's sign-in, no longer matches, so an update that changes nothing
    /// there leaves the page, and any field being edited in it, as it is.
    pub(in crate::ui) fn refresh_claude_cloud_health(&self) {
        let claude = self.page() == Page::Settings(Pane::Tools)
            && self.with_state(|state| {
                !state.tools.provider_overview
                    && state.tools.provider_id.as_deref() == Some("claude")
            });
        if !claude {
            return;
        }
        let Some(shell) = self.shell() else {
            return;
        };
        let mut line = None;
        let mut sign_in = false;
        widgets::walk(shell.settings.column.upcast_ref(), &mut |widget| {
            if widget.widget_name() == CLAUDE_CLOUD_HEALTH_NAME {
                line = widget.tooltip_text().map(|line| line.to_string());
            } else if widget.widget_name() == CLAUDE_CODE_SIGN_IN_NAME {
                sign_in = true;
            }
        });
        let shown = line.map(|line| (line, sign_in));
        if shown != self.claude_cloud_health_wanted() {
            self.render_settings();
        }
    }

    /// Every Claude Code config folder whose sessions this computer finds
    /// and uploads, and why; folders added here can be removed here. `None`
    /// until the core reports them.
    pub(super) fn claude_folders_section(&self) -> Option<gtk::Box> {
        let (folders, cloud, health, error) = self.with_state(|state| {
            let settings = state.settings.as_ref();
            (
                settings.and_then(|settings| settings.claude_config_dirs.clone()),
                settings.and_then(|settings| settings.claude_cloud_sessions),
                state
                    .status
                    .as_ref()
                    .and_then(|status| status.claude_cloud.clone()),
                state.claude_folders_error.clone(),
            )
        });
        let folders = folders?;
        let (section, group) = settings_section(CLAUDE_SHARING_TITLE, None);
        group.append(&self.claude_folders_disclosure(&folders));
        if let Some(cloud) = cloud {
            settings_divider(&group, 62);
            group.append(&self.claude_cloud_row(cloud, health.as_ref()));
        }
        if let Some(error) = error {
            settings_divider(&group, 62);
            let footer = hbox(8, &[]);
            widgets::pad(&footer, 12, Some(16), None);
            footer.append(&notice(&error, Tone::Error));
            group.append(&footer);
        }
        Some(section)
    }

    /// The watched folders behind one summary row. The upload explanation
    /// opens with the switches; the open state survives status refreshes.
    fn claude_folders_disclosure(&self, folders: &[ClaudeConfigDir]) -> gtk::Expander {
        // The arrow sits in the icon column, so the title lines up with the
        // other rows' titles; the folders open indented beneath it.
        let heading = hbox(12, &[]);
        super::zoom::margins(&heading, 12, 16, 12, 22);
        // Both wrap, as other rows' titles do, so the heading never sets
        // the page's width at a large zoom.
        let title = text(CLAUDE_FOLDERS_TITLE, &["settings-row-title"]).wrapped();
        title.set_hexpand(true);
        heading.append(&title);
        let summary = text(&claude_folders_summary(folders), &["settings-row-detail"]).wrapped();
        summary.set_xalign(1.0);
        summary.set_justify(gtk::Justification::Right);
        heading.append(&summary);
        widgets::action_cursor(&heading);
        let disclosure = gtk::Expander::builder().label_widget(&heading).build();
        disclosure.set_widget_name("claude-folders");
        disclosure.set_margin_start(16);
        disclosure.set_expanded(
            self.with_state(|state| state.settings_details_open.contains(FOLDERS_KEY)),
        );
        let weak = self.downgrade();
        disclosure.connect_expanded_notify(move |disclosure| {
            if let Some(app) = weak.upgrade() {
                app.update_state(|state| {
                    if disclosure.is_expanded() {
                        state.settings_details_open.insert(FOLDERS_KEY.into());
                    } else {
                        state.settings_details_open.remove(FOLDERS_KEY);
                    }
                });
            }
        });
        let rows = vbox(0, &[]);
        let detail = text(CLAUDE_FOLDERS_DETAIL, &["settings-row-detail"]).wrapped();
        super::zoom::margins(&detail, 0, 16, 8, 54);
        rows.append(&detail);
        self.claude_folder_rows(&rows, folders);
        disclosure.set_child(Some(&rows));
        disclosure
    }

    fn claude_folder_rows(&self, group: &gtk::Box, folders: &[ClaudeConfigDir]) {
        let home = std::env::var("HOME").unwrap_or_default();
        for folder in folders {
            settings_divider(group, 62);
            let accessory = hbox(8, &[]);
            if claude_folder_removable(folder) {
                let without = claude_folders_removing(&folder.path, folders);
                accessory.append(&button(
                    CLAUDE_FOLDERS_REMOVE,
                    &[],
                    self.on(move |app| app.set_claude_folders(without.clone())),
                ));
            }
            let switch = gtk::Switch::new();
            switch.set_active(folder.shared);
            switch.set_valign(gtk::Align::Center);
            switch.set_tooltip_text(Some(CLAUDE_FOLDERS_SHARE_HELP));
            widgets::name(&switch, CLAUDE_FOLDERS_SHARE_HELP);
            let weak = self.downgrade();
            let path = folder.path.clone();
            switch.connect_active_notify(move |switch| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.share_claude_folder(&path, switch.is_active());
                }
            });
            accessory.append(&switch);
            let (row, _) = settings_row(
                &claude_folder_display_path(&folder.path, &home),
                &claude_folder_detail(folder),
                "folder-symbolic",
                Some(accessory.upcast_ref()),
                &[],
            );
            row.set_tooltip_text(Some(&folder.path));
            group.append(&row);
        }
        settings_divider(group, 62);
        let folders = folders.to_vec();
        let add = button(
            CLAUDE_FOLDERS_ADD,
            &[],
            self.on(move |app| app.choose_claude_folder(folders.clone())),
        );
        add.set_tooltip_text(Some(CLAUDE_FOLDERS_ADD_HELP));
        add.set_halign(gtk::Align::End);
        widgets::pad(&add, 10, Some(16), None);
        group.append(&add);
    }

    /// The opt-in for claude.ai cloud sessions, with its consent detail and,
    /// while on, how syncing is going.
    fn claude_cloud_row(&self, cloud: bool, health: Option<&ClaudeCloudHealth>) -> gtk::Box {
        let switch = gtk::Switch::new();
        switch.set_active(cloud);
        switch.set_valign(gtk::Align::Center);
        widgets::name(&switch, CLAUDE_CLOUD_SESSIONS_TITLE);
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                app.set_claude_cloud_sessions(switch.is_active());
            }
        });
        let health_label = claude_cloud_health_label(health);
        let mut extra: Vec<gtk::Widget> = Vec::new();
        if cloud {
            let line = claude_cloud_health_line(health);
            let shown = notice(
                &line,
                if health_label == "Working" {
                    Tone::Ok
                } else if health_label == "Checking" {
                    Tone::Neutral
                } else {
                    Tone::Warn
                },
            );
            shown.set_widget_name(CLAUDE_CLOUD_HEALTH_NAME);
            shown.set_tooltip_text(Some(&line));
            extra.push(shown.upcast());
            if claude_code_sign_in_offered(health) {
                let sign_in = button(
                    CLAUDE_CODE_SIGN_IN,
                    &[],
                    self.on(|app| app.sign_in_to_claude_code()),
                );
                sign_in.set_widget_name(CLAUDE_CODE_SIGN_IN_NAME);
                sign_in.set_tooltip_text(Some(CLAUDE_CODE_SIGN_IN_HELP));
                sign_in.set_halign(gtk::Align::Start);
                extra.push(sign_in.upcast());
            }
        }
        settings_row(
            CLAUDE_CLOUD_SESSIONS_TITLE,
            CLAUDE_CLOUD_SESSIONS_DETAIL,
            "weather-overcast-symbolic",
            Some(switch.upcast_ref()),
            &extra,
        )
        .0
    }

    /// Asks for a folder and adds it to the watched Claude Code folders.
    fn choose_claude_folder(&self, folders: Vec<ClaudeConfigDir>) {
        let chooser = gtk::FileChooserNative::new(
            Some(CLAUDE_FOLDERS_ADD.trim_end_matches('…')),
            Some(self.window()),
            gtk::FileChooserAction::SelectFolder,
            Some("Add"),
            Some("Cancel"),
        );
        if let Ok(home) = std::env::var("HOME") {
            let _ = chooser.set_current_folder(Some(&gio::File::for_path(home)));
        }
        // The chooser must outlive this call; its answer releases it.
        let held = std::rc::Rc::new(std::cell::RefCell::new(Some(chooser.clone())));
        let weak = self.downgrade();
        chooser.connect_response(move |chooser, response| {
            let path = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|file| file.path()))
                .flatten();
            if let (Some(path), Some(app)) = (path, weak.upgrade()) {
                app.set_claude_folders(claude_folders_adding(&path.to_string_lossy(), &folders));
            }
            held.borrow_mut().take();
        });
        chooser.show();
    }

    /// Shares a watched Claude Code folder's sessions, or stops sharing them.
    pub fn share_claude_folder(&self, path: &str, shared: bool) {
        self.set_setting(
            SettingsChange {
                claude_config_dir: Some(ClaudeConfigDirChange {
                    path: path.to_string(),
                    shared: Some(shared),
                }),
                ..SettingsChange::default()
            },
            SettingRow::ClaudeFolders,
        );
    }

    /// Opens Claude Code's own sign-in (`claude_cloud.sign_in`) in a terminal
    /// window; a refusal shows under the section, as the core worded it.
    pub fn sign_in_to_claude_code(&self) {
        self.claude_code_sign_in(false);
    }

    /// The sign-in, from Settings or, with `reveal`, from the notification,
    /// whose refusal opens Claude Code's page to show it.
    fn claude_code_sign_in(&self, reveal: bool) {
        self.call::<agent_relay_desktop_core::contract::method::ClaudeCloudSignIn>(
            NoParams {},
            move |app, result| {
                let error = result.err().map(|error| error.message);
                let refused = error.is_some();
                app.update_state(|state| state.claude_folders_error = error);
                if reveal && refused {
                    app.open_provider("claude");
                } else {
                    app.render_settings();
                }
            },
        );
    }

    /// Posts the one notification of a Claude Code sign-in episode when the
    /// core first offers the sign-in; the preference that says it was posted
    /// outlives a relaunch. Matches the Mac's `ClaudeSignInNotice`.
    pub(in crate::ui) fn note_claude_cloud_health(&self, health: Option<&ClaudeCloudHealth>) {
        let notified = self
            .prefs()
            .get(CLAUDE_SIGN_IN_NOTIFIED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let decision = claude_sign_in_notice(health, notified);
        if decision.notified != notified {
            let _ = self
                .inner
                .prefs
                .borrow_mut()
                .set(CLAUDE_SIGN_IN_NOTIFIED_KEY, decision.notified.into());
        }
        if decision.notify {
            self.inner.desktop.notify_app(&AppNotice {
                id: CLAUDE_SIGN_IN_ACTION.into(),
                title: CLAUDE_SIGN_IN_NOTICE_TITLE.into(),
                body: CLAUDE_SIGN_IN_NOTICE_BODY.into(),
                default_action: CLAUDE_SETTINGS_ACTION.into(),
                button: Some((CLAUDE_SIGN_IN_NOTICE_ACTION.into(), CLAUDE_SIGN_IN_ACTION.into())),
            });
        }
    }

    /// The app actions the sign-in notification's button and body run.
    pub fn install_claude_sign_in_actions(&self, application: &gtk::Application) {
        for (name, sign_in) in [(CLAUDE_SIGN_IN_ACTION, true), (CLAUDE_SETTINGS_ACTION, false)] {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.downgrade();
            action.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    app.claude_sign_in_notice_answered(sign_in);
                }
            });
            application.add_action(&action);
        }
    }

    /// The sign-in notification was answered: its Sign in button runs the
    /// same sign-in as Settings' button; a click on the notification itself
    /// opens Claude Code's page, where Sharing is.
    pub fn claude_sign_in_notice_answered(&self, sign_in: bool) {
        if sign_in {
            self.claude_code_sign_in(true);
        } else {
            self.open_provider("claude");
        }
    }

    pub fn set_claude_cloud_sessions(&self, on: bool) {
        self.set_setting(
            SettingsChange {
                claude_cloud_sessions: Some(on),
                ..SettingsChange::default()
            },
            SettingRow::ClaudeFolders,
        );
    }

    fn set_claude_folders(&self, added: Vec<String>) {
        self.set_setting(
            SettingsChange {
                claude_config_dirs: Some(added),
                ..SettingsChange::default()
            },
            SettingRow::ClaudeFolders,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(shared: &[bool]) -> Vec<ClaudeConfigDir> {
        shared
            .iter()
            .enumerate()
            .map(|(index, &shared)| ClaudeConfigDir {
                path: format!("/f{index}"),
                origin: "default".into(),
                shared,
            })
            .collect()
    }

    #[test]
    fn folder_disclosure_summarizes_sharing_as_on_the_mac() {
        assert_eq!(claude_folders_summary(&folders(&[])), "No folders found");
        assert_eq!(claude_folders_summary(&folders(&[true])), "Shared");
        assert_eq!(
            claude_folders_summary(&folders(&[true, true])),
            "All 2 shared"
        );
        assert_eq!(
            claude_folders_summary(&folders(&[true, false, true])),
            "2 of 3 shared"
        );
        assert_eq!(claude_folders_summary(&folders(&[false])), "Not shared");
    }
}
