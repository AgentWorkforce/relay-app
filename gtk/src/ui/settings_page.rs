//! Settings grouped by task: Preferences, Sessions, and Account & app.
//! Updates, diagnostic reports, and session data each have one home.

mod account_health;
mod notices;
mod pending;
mod provider_preferences;
pub(crate) mod sharing;
mod software_updates;
pub(super) use pending::{pending_toggle_row, switch_accessory};

use super::app::App;
use super::appearance_preview::mini_window;
use super::desktop::Confirmation;
use super::state::{Page, Pane, UpdateState};
use super::widgets::{
    self, button, clear, hbox, icon, notice, settings_divider, settings_row,
    settings_section, text, vbox, LabelStyle, Tone,
};
use agent_relay_desktop_core::install::InstallKind;
use crate::models::{
    managed_inbound_detail, DiagnosticsSection,
    SharingModeText, WORKSPACE_FOUND_DETAIL, WORKSPACE_MISSING_DETAIL,
};
use crate::prefs::{Appearance, SessionOpenPreference};
use agent_relay_desktop_core::contract::account::SharingParams;
use agent_relay_node::relay::contract::agents::SettingsChange;
use agent_relay_desktop_core::contract::platform::LoginItemParams;
use agent_relay_node::relay::contract::{method, NoParams};
use agent_relay_desktop_core::telemetry::{TelemetryFailure, TelemetryOperation, TelemetryOutcome};
use agent_relay_desktop_core::contract::account::SharingMode;
use gtk::{gio, glib, prelude::*};
use gtk4 as gtk;
use std::path::PathBuf;
use std::time::Duration;

/// The uploader runs one change at a time; the Mac's `perform` refuses
/// a second with this sentence.
pub(super) const UPLOADER_BUSY: &str =
    "Another upload change is still running. Try again in a moment.";

mod layout;
pub use layout::SettingsPage;

/// Which row reports a setting's failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingRow {
    DirectDelivery,
    GrokLeader,
    OpenCodePlugin,
    OpenClaw,
    TrustedSenders,
    Hermes,
    DevinDelivery,
    CursorDelivery,
    ClaudeFolders,
    ClaudeSessionHook,
    CodexHook,
    /// The pane's own banner.
    Pane,
}

/// A switch row: `on` gets each change the person makes.
pub(super) fn toggle_row(
    title: &str,
    detail: &str,
    icon_name: &str,
    active: bool,
    sensitive: bool,
    on: impl Fn(bool) + 'static,
) -> gtk::Box {
    pending_toggle_row(title, detail, icon_name, active, sensitive, false, on)
}

/// `Windows 10.0.26100 (aarch64)`: the system as diagnostics name it.
#[cfg(windows)]
fn platform() -> String {
    use agent_relay_desktop_core::install::windows::registry::{self, Root};
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let major = registry::read_dword(Root::LocalMachine, KEY, "CurrentMajorVersionNumber");
    let minor = registry::read_dword(Root::LocalMachine, KEY, "CurrentMinorVersionNumber");
    let build = registry::read_string(Root::LocalMachine, KEY, "CurrentBuild");
    match (major, minor, build) {
        (Some(major), Some(minor), Some(build)) => format!(
            "Windows {major}.{minor}.{build} ({})",
            std::env::consts::ARCH
        ),
        _ => format!("Windows ({})", std::env::consts::ARCH),
    }
}

/// `Linux 6.8.0 (x86_64)`: the system as diagnostics name it.
#[cfg(not(windows))]
fn platform() -> String {
    // SAFETY: `uname` fills the zeroed struct it is given.
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut names) } != 0 {
        return std::env::consts::OS.to_string();
    }
    let field = |value: &[libc::c_char]| {
        let bytes: Vec<u8> = value
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    format!(
        "{} {} ({})",
        field(&names.sysname),
        field(&names.release),
        field(&names.machine)
    )
}

impl App {
    /// Draws the Settings pane shown.
    pub fn render_settings(&self) {
        let Some(shell) = self.shell() else {
            return;
        };
        let Page::Settings(pane) = self.page() else {
            return;
        };
        if !self.with_state(|state| state.connected()) {
            return;
        }
        let adjustment = shell.settings.scroller.vadjustment();
        let position = adjustment.value();
        let column = &shell.settings.column;
        shell
            .settings
            .holder
            .set_max_width(if pane == Pane::Diagnostics {
                i32::MAX
            } else {
                776
            });
        clear(column);
        self.inner.updating.set(true);
        match pane {
            Pane::General => self.general_pane(column),
            Pane::Tools => self.tools_pane(column),
            Pane::Opening => self.provider_opening_preferences(column, None),
            Pane::About => self.about_and_identity(column),
            Pane::Relay => self.relay_pane(column),
            Pane::Sharing => {
                self.sharing_pane(column);
            }
            Pane::Diagnostics => self.diagnostics_pane(column),
            Pane::Account => self.account_pane(column),
        }
        self.inner.updating.set(false);
        if let Some(error) = self.with_state(|state| state.settings_error.clone()) {
            let banner = hbox(8, &["banner", "banner-warn"]);
            banner.append(&text(&error, &["error"]).wrapped());
            column.append(&banner);
        }
        let target = self.update_state(|state| {
            let jump = state.request_scroll;
            state.request_scroll = false;
            jump.then(|| {
                state
                    .selected_request
                    .as_ref()
                    .map(|entry| entry.id.clone())
            })
            .flatten()
        });
        if let Some(target) = target {
            // An idle callback runs before GTK allocates newly expanded rows.
            // Wait for their layout, then scroll using the actual row bounds.
            let frames = std::cell::Cell::new(0);
            column.add_tick_callback(move |column, _| {
                frames.set(frames.get() + 1);
                let mut child = column.first_child();
                while let Some(widget) = child {
                    if widget.widget_name().as_str() == target && widget.height() > 0 {
                        if let Some(bounds) = widget.compute_bounds(column) {
                            adjustment.set_value(bounds.y() as f64);
                            return glib::ControlFlow::Break;
                        }
                    }
                    child = widget.next_sibling();
                }
                if frames.get() > 10 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        } else {
            glib::idle_add_local_once(move || adjustment.set_value(position));
        }
    }

    // Appearance.

    fn appearance_section(&self, column: &gtk::Box) {
        let (section, group) = settings_section(
            "Appearance",
            Some("Match system follows your desktop's light and dark setting."),
        );
        let options = hbox(12, &[]);
        widgets::pad(&options, 16, None, None);
        options.set_homogeneous(true);
        let chosen = self.prefs().appearance();
        for (choice, title, icon_name) in [
            (
                Appearance::System,
                "Match system",
                "weather-few-clouds-symbolic",
            ),
            (Appearance::Light, "Light", "weather-clear-symbolic"),
            (Appearance::Dark, "Dark", "weather-clear-night-symbolic"),
        ] {
            let selected = chosen == choice;
            let option = gtk::Button::new();
            crate::ui::widgets::action_cursor(&option);
            widgets::add_classes(
                &option,
                &["appearance-option", if selected { "selected" } else { "" }],
            );
            let content = vbox(10, &[]);
            if choice == Appearance::System {
                let preview = hbox(0, &[]);
                preview.set_homogeneous(true);
                preview.append(&mini_window(false));
                preview.append(&mini_window(true));
                content.append(&preview);
            } else {
                content.append(&mini_window(choice == Appearance::Dark));
            }
            let caption = hbox(6, &[]);
            caption.set_halign(gtk::Align::Center);
            let glyph = icon(
                if selected {
                    "emblem-ok-symbolic"
                } else {
                    icon_name
                },
                14,
            );
            glyph.add_css_class(if selected { "accent" } else { "secondary" });
            caption.append(&glyph);
            // Wrapped and centred, so three options still fit side by side
            // in a narrow window at a high zoom.
            caption.append(
                &text(title, &[if selected { "settings-row-title" } else { "" }])
                    .wrapped()
                    .centered(),
            );
            content.append(&caption);
            option.set_child(Some(&content));
            widgets::name(&option, title);
            let choose = self.on(move |app| app.set_appearance(choice));
            option.connect_clicked(move |_| choose());
            options.append(&option);
        }
        self.append_zoom_row(&group, &options);
        column.append(&section);
    }

    pub fn set_appearance(&self, choice: Appearance) {
        // A preference that cannot be saved holds for this run.
        let _ = self.inner.prefs.borrow_mut().set_appearance(choice);
        self.inner.styling.set(choice);
        self.render_settings();
    }

    // Software updates.

    pub(super) fn about_pane(&self, column: &gtk::Box) {
        let (section, group) = settings_section(
            "Software updates",
            Some("Check for new Agent Relay releases."),
        );
        let (state, release, message, checked, kind) = self.with_state(|state| {
            (
                state.update_state,
                state.update_release.clone(),
                state.update_message.clone(),
                state.update_checked.clone(),
                state.install_kind,
            )
        });
        let headline = software_updates::headline(state, release.as_ref());
        let detail = software_updates::detail(
            state,
            self.prefs().include_beta_releases(),
            kind,
            message,
            checked,
        );
        let action_title = match state {
            UpdateState::Available => "Update",
            UpdateState::Downloading => "Downloading…",
            UpdateState::Installing => "Installing…",
            UpdateState::Checking => "Checking…",
            _ => "Check now",
        };
        let accessory = hbox(8, &[]);
        if state.busy() {
            let spinner = gtk::Spinner::new();
            spinner.start();
            accessory.append(&spinner);
        }
        let action = button(
            action_title,
            if state == UpdateState::Available {
                &["primary"]
            } else {
                &[]
            },
            self.on(App::update_action),
        );
        action.set_sensitive(!state.busy());
        accessory.append(&action);
        let icon_name = match state {
            UpdateState::UpToDate => "emblem-ok-symbolic",
            UpdateState::Available | UpdateState::Downloading => {
                "software-update-available-symbolic"
            }
            UpdateState::Failed => "dialog-warning-symbolic",
            _ => "view-refresh-symbolic",
        };
        let (row, words) =
            settings_row(&headline, "", icon_name, Some(accessory.upcast_ref()), &[]);
        words.append(
            &text(
                &detail,
                &[if state == UpdateState::Failed {
                    "error"
                } else {
                    "settings-row-detail"
                }],
            )
            .wrapped(),
        );
        group.append(&row);
        settings_divider(&group, 62);
        let weak = self.downgrade();
        group.append(&toggle_row(
            "Automatically check for updates",
            "Check once a day while Agent Relay is running. You choose when to install an available update.",
            "alarm-symbolic",
            self.prefs().automatically_checks_for_updates(),
            true,
            move |enabled| {
                if let Some(app) = weak.upgrade() {
                    app.set_auto_update(enabled);
                }
            },
        ));
        settings_divider(&group, 62);
        group.append(&self.beta_update_row(state));
        column.append(&section);
    }

    pub fn set_auto_update(&self, enabled: bool) {
        if !self.inner.updating.get() {
            let _ = self
                .inner
                .prefs
                .borrow_mut()
                .set_automatically_checks_for_updates(enabled);
        }
    }

    /// About's allowlisted diagnostics.
    pub fn diagnostics(&self) -> Vec<DiagnosticsSection> {
        self.with_state(|state| state.diagnostics(&platform()))
    }

    fn diagnostics_view(&self) -> gtk::Box {
        let outer = vbox(20, &[]);
        let words = vbox(5, &[]);
        words.set_hexpand(true);
        words.append(&text("Diagnostic report", &["settings-section-title"]));
        words.append(&text("Build, connection and workspace details.", &["secondary"]).wrapped());
        let copy = gtk::Button::with_label("Copy diagnostics");
        crate::ui::widgets::action_cursor(&copy);
        let weak = self.downgrade();
        copy.connect_clicked(move |copy| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            widgets::copy_to_clipboard(&crate::models::diagnostics_report(
                &app.diagnostics(),
            ));
            copy.set_label("Copied");
            let copy = copy.downgrade();
            glib::timeout_add_local_once(Duration::from_secs(2), move || {
                if let Some(copy) = copy.upgrade() {
                    copy.set_label("Copy diagnostics");
                }
            });
        });
        copy.set_valign(gtk::Align::Center);
        // The button goes under the heading when the pane is narrow.
        let row = widgets::Reflow::new(&words, &copy, 12, 10, widgets::ROW_TEXT_WIDTH);
        outer.append(&row);
        if !self.with_state(|state| state.core_connected) {
            outer.append(&notice(
                "Service offline. Build and workspace details below are from the last received snapshot.",
                Tone::Warn,
            ));
        }
        for section in self.diagnostics() {
            let card = vbox(12, &["card"]);

            for (label, value) in section.rows {
                let line = hbox(24, &[]);
                line.append(&text(label, &["secondary", "caption"]));
                line.append(&widgets::spacer());
                let value = text(&value, &["caption"]).wrapped().copyable();
                value.set_xalign(1.0);
                line.append(&value);
                card.append(&line);
            }
            let disclosure = gtk::Expander::new(Some(section.title));
            disclosure.set_expanded(
                self.with_state(|state| state.settings_details_open.contains(section.title)),
            );
            let weak = self.downgrade();
            let title = section.title.to_owned();
            disclosure.connect_expanded_notify(move |expander| {
                if let Some(app) = weak.upgrade() {
                    app.update_state(|state| {
                        if expander.is_expanded() {
                            state.settings_details_open.insert(title.clone());
                        } else {
                            state.settings_details_open.remove(&title);
                        }
                    });
                }
            });
            disclosure.set_child(Some(&card));
            disclosure.add_css_class("diagnostic-disclosure");
            outer.append(&disclosure);
        }
        outer.append(
            &text(
                "Includes workspace and account IDs. No credentials, session content, or logs are copied.",
                &["caption"],
            )
            .wrapped(),
        );
        outer
    }

    // Messages.

    fn relay_pane(&self, column: &gtk::Box) {
        let (connected, allow) = self.with_state(|state| (state.connected(), state.settings.as_ref().is_none_or(|settings| settings.allow_self_registration)));
        if connected {
            let (section, group) = settings_section(
                "Relay",
                Some("Sessions you put on the relay get a name anyone in this workspace can message."),
            );
            group.append(&self.workspace_row());
            self.append_agent_relay_rows(&group);
            column.append(&section);
        }
        let (section, group) = settings_section("Joining the relay", None);
        let weak = self.downgrade();
        group.append(&pending_toggle_row(
            "Let sessions put themselves on the relay",
            "Allow a supported local agent session to register only itself through the private desktop socket.",
            "contact-new-symbolic",
            allow,
            true,
            self.with_state(|state| state.setting_pending("allow_self_registration")),
            move |enabled| {
                if let Some(app) = weak.upgrade().filter(|app| !app.inner.updating.get()) {
                    app.set_setting(
                        SettingsChange {
                            allow_self_registration: Some(enabled),
                            ..SettingsChange::default()
                        },
                        SettingRow::Pane,
                    );
                }
            },
        ));
        column.append(&section);
        if let Some(section) = self.spawn_section() {
            column.append(&section);
        }
    }

    fn workspace_row(&self) -> gtk::Box {
        let (found, agents, connect_error, connecting, account_workspace) =
            self.with_state(|state| {
                (
                    state.has_workspace(),
                    state
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.agents.clone()),
                    state.workspace_connect_error.clone(),
                    state.connecting_workspace(),
                    state.account().workspace_id.is_some(),
                )
            });
        let mut extra = notices::workspace_notices(agents.unwrap_or_default(), connect_error);
        if connecting || !found {
            let actions = hbox(8, &[]);
            actions.set_margin_top(4);
            if connecting {
                let spinner = gtk::Spinner::new();
                spinner.start();
                actions.append(&spinner);
                actions.append(&button(
                    "Cancel",
                    &[],
                    self.on(App::cancel_connect_workspace),
                ));
            } else {
                let connect = button("Connect Workspace", &[], self.on(App::connect_workspace));
                connect.set_tooltip_text(Some(
                    "Fetch this workspace's relay key with your Agent Relay sign-in",
                ));
                connect.set_sensitive(account_workspace);
                actions.append(&connect);
            }
            extra.push(actions.upcast());
        }
        settings_row(
            if found {
                "Workspace from your Agent Relay account"
            } else {
                "No relay workspace"
            },
            if found {
                WORKSPACE_FOUND_DETAIL
            } else {
                WORKSPACE_MISSING_DETAIL
            },
            if found {
                "network-wireless-symbolic"
            } else {
                "network-wireless-offline-symbolic"
            },
            None,
            &extra,
        )
        .0
    }

    pub(super) fn claude_delivery_row(&self) -> gtk::Box {
        let (direct, managed, error) = self.with_state(|state| {
            let settings = state.settings.as_ref();
            (
                settings.and_then(|settings| settings.direct_delivery) == Some(true),
                settings.and_then(|settings| settings.direct_delivery_managed.clone()),
                state.inbound_error.clone(),
            )
        });
        let is_managed = managed
            .as_deref()
            .is_some_and(|managed| managed != "accept");
        let detail = if direct {
            "Relayed messages arrive directly: crossSessionInbound is accept in your Claude settings.".to_string()
        } else if let Some(managed) = managed.as_deref().filter(|_| is_managed) {
            managed_inbound_detail(managed)
        } else {
            "Sessions that skip permission prompts hold relayed messages for your approval. Deliver directly to let them arrive the way Codex messages do.".to_string()
        };
        let accessory: Option<gtk::Widget> = if direct && managed.is_none() {
            let ask = button(
                "Ask Again",
                &[],
                self.on(|app| {
                    app.set_setting(
                        SettingsChange {
                            direct_delivery: Some(false),
                            ..SettingsChange::default()
                        },
                        SettingRow::DirectDelivery,
                    )
                }),
            );
            ask.set_tooltip_text(Some(
                "Set crossSessionInbound to hold in your Claude settings",
            ));
            Some(ask.upcast())
        } else if !direct && !is_managed {
            Some(
                button(
                    "Deliver Directly…",
                    &[],
                    self.on(App::confirm_direct_delivery),
                )
                .upcast(),
            )
        } else {
            None
        };
        let extra: Vec<gtk::Widget> = error
            .iter()
            .map(|error| notice(error, Tone::Error))
            .collect();
        settings_row(
            "Claude Code delivery",
            &detail,
            if direct {
                "emblem-ok-symbolic"
            } else {
                "dialog-question-symbolic"
            },
            accessory.as_ref(),
            &extra,
        )
        .0
    }

    pub fn confirm_direct_delivery(&self) {
        let answer = self.confirm(Confirmation::new(
            "Deliver relayed messages to Claude Code without asking?",
            "This adds \"crossSessionInbound\": \"accept\" to ~/.claude/settings.json. Every local Claude Code session will then show messages from other sessions and programs, including Agent Relay, without holding them for approval. Claude Code still labels them and never lets them change permissions. Your current file is kept as settings.json.relay-desktop-backup.",
            "Deliver directly",
        ));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            if let (true, Some(app)) = (answer.await, weak.upgrade()) {
                app.set_setting(
                    SettingsChange {
                        direct_delivery: Some(true),
                        ..SettingsChange::default()
                    },
                    SettingRow::DirectDelivery,
                );
            }
        });
    }

    /// Grok's `[cli] use_leader`. A plain Grok session opens no route in;
    /// one on the shared leader can be listed and messaged like a Claude
    /// Code or Codex session.
    pub(in crate::ui) fn grok_leader_row(&self) -> gtk::Box {
        let (enabled, error) = self.with_state(|state| {
            (
                state
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.grok_leader)
                    == Some(true),
                state.grok_leader_error.clone(),
            )
        });
        let (detail, accessory) = if enabled {
            let off = button(
                "Turn Off",
                &[],
                self.on(|app| {
                    app.set_setting(
                        SettingsChange {
                            grok_leader: Some(false),
                            ..SettingsChange::default()
                        },
                        SettingRow::GrokLeader,
                    )
                }),
            );
            off.set_tooltip_text(Some("Set use_leader = false in your Grok config"));
            ("Grok runs its sessions on a shared leader: use_leader is true in your Grok config. Sessions you open in Grok appear here and can be messaged.", off)
        } else {
            ("A plain Grok session has no way in. Turn on leader mode so sessions you open in Grok appear here and can be messaged, the way Claude Code and Codex sessions are.",
             button("Turn On Leader Mode…", &[], self.on(App::confirm_grok_leader)))
        };
        let extra: Vec<gtk::Widget> = error
            .iter()
            .map(|error| notice(error, Tone::Error))
            .collect();
        settings_row(
            "Grok sessions",
            detail,
            if enabled {
                "emblem-ok-symbolic"
            } else {
                "network-transmit-receive-symbolic"
            },
            Some(accessory.upcast_ref()),
            &extra,
        )
        .0
    }

    /// Agent Relay's OpenCode plugin. A plain OpenCode session opens no
    /// port; with the plugin, each OpenCode window can be listed and
    /// messaged like a Claude Code or Codex session.
    pub(in crate::ui) fn opencode_plugin_row(&self) -> gtk::Box {
        let (enabled, error) = self.with_state(|state| {
            (
                state
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.opencode_plugin)
                    == Some(true),
                state.opencode_plugin_error.clone(),
            )
        });
        let set = |enabled: bool| {
            self.on(move |app| {
                app.set_setting(
                    SettingsChange {
                        opencode_plugin: Some(enabled),
                        ..SettingsChange::default()
                    },
                    SettingRow::OpenCodePlugin,
                )
            })
        };
        let (detail, accessory) = if enabled {
            let off = button("Turn Off", &[], set(false));
            off.set_tooltip_text(Some("Remove Agent Relay's plugin from OpenCode"));
            ("Agent Relay's plugin is in ~/.config/opencode/plugins/agent-relay.js. OpenCode sessions you open appear here and can be messaged. Restart OpenCode windows that were open before it was on.", off)
        } else {
            let on = button("Turn On", &[], set(true));
            on.set_tooltip_text(Some("Add Agent Relay's plugin to OpenCode"));
            ("A plain OpenCode session has no way in. Turn this on to add Agent Relay's plugin, so sessions you open in OpenCode appear here and can be messaged, the way Claude Code and Codex sessions are.", on)
        };
        let extra: Vec<gtk::Widget> = error
            .iter()
            .map(|error| notice(error, Tone::Error))
            .collect();
        settings_row(
            "OpenCode sessions",
            detail,
            if enabled {
                "emblem-ok-symbolic"
            } else {
                "network-transmit-receive-symbolic"
            },
            Some(accessory.upcast_ref()),
            &extra,
        )
        .0
    }

    pub fn confirm_grok_leader(&self) {
        let answer = self.confirm(Confirmation::new(
            "Run Grok sessions on a shared leader?",
            "This sets use_leader = true under [cli] in ~/.grok/config.toml. Grok sessions you open afterwards run on a shared grok agent leader process, which stays running after you close Grok and is what Agent Relay delivers through. Sessions already open are not affected until you restart them. Grok refuses leader mode when a sandbox profile is on. Your current file is kept as config.toml.relay-desktop-backup.",
            "Turn on leader mode",
        ));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            if let (true, Some(app)) = (answer.await, weak.upgrade()) {
                app.set_setting(
                    SettingsChange {
                        grok_leader: Some(true),
                        ..SettingsChange::default()
                    },
                    SettingRow::GrokLeader,
                );
            }
        });
    }

    fn request_log_view(&self, column: &gtk::Box) {
        let problems = self.status_problems();
        if !problems.is_empty() {
            column.append(&text("Current problems", &["heading", "warn"]));
            let details = problems.join("\n\n");
            column.append(&text(&details, &[]).wrapped().copyable());
            column.append(&self.diagnostics_copy_button(details));
        }
        let heading = hbox(12, &[]);
        let title = text("Requests", &["heading"]);
        title.set_hexpand(true);
        heading.append(&title);
        let filter = gtk::CheckButton::with_label("Failures only");
        filter.set_active(self.with_state(|state| state.request_failures_only));
        let app = self.clone();
        filter.connect_toggled(move |button| {
            app.update_state(|state| state.request_failures_only = button.is_active());
            app.render_settings();
        });
        heading.append(&filter);
        column.append(&heading);
        let (mut entries, selected, failures_only, error) = self.with_state(|state| {
            (
                state.request_entries.clone(),
                state.selected_request.clone(),
                state.request_failures_only,
                state.request_log_error.clone(),
            )
        });
        if let Some(error) = error {
            column.append(&text(&error, &["warn"]).wrapped());
        }
        if let Some(entry) = &selected {
            if !entries.iter().any(|item| item.id == entry.id) {
                entries.insert(0, entry.clone());
            }
        }
        entries.retain(|entry| !failures_only || entry.failed);
        if entries.is_empty() {
            column.append(&text(
                if failures_only {
                    "No failed requests."
                } else {
                    "No requests recorded yet."
                },
                &["secondary"],
            ));
        }
        for entry in entries {
            let status = entry
                .status
                .map(|status| format!("HTTP {status}"))
                .unwrap_or_else(|| {
                    if entry.duration_ms.is_none() {
                        "Pending"
                    } else if entry.failed {
                        "Failed"
                    } else {
                        "Completed"
                    }
                    .into()
                });
            let time = chrono::DateTime::from_timestamp_millis(entry.started_at_ms as i64)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default();
            let title = format!(
                "{} · {} {}\n{} · {}{}",
                entry.service_name(),
                entry.method,
                entry.endpoint,
                time,
                status,
                entry
                    .duration_ms
                    .map(|ms| format!(" · {ms} ms"))
                    .unwrap_or_default()
            );
            let row = gtk::Expander::new(None);
            row.set_widget_name(&entry.id);
            let label = text(&title, if entry.failed { &["warn"] } else { &[] }).wrapped();
            label.set_hexpand(true);
            label.set_halign(gtk::Align::Fill);
            row.set_hexpand(true);
            row.set_label_widget(Some(&label));
            row.set_expanded(
                selected.as_ref().is_some_and(|item| item.id == entry.id)
                    || self.with_state(|state| state.settings_details_open.contains(&entry.id)),
            );
            if selected.as_ref().is_some_and(|item| item.id == entry.id) {
                row.add_css_class("banner-warn");
            }
            let detail = vbox(8, &[]);
            let report = format!(
                "{title}\nRequest\n{}\nResponse\n{}",
                entry.request, entry.response
            );
            detail.append(&text(&report, &["monospace"]).wrapped().copyable());
            let copy = button(
                "Copy request details",
                &[],
                self.on(move |_| widgets::copy_to_clipboard(&report)),
            );
            detail.append(&copy);
            row.set_child(Some(&detail));
            let app = self.clone();
            row.connect_expanded_notify(move |row| {
                app.update_state(|state| {
                    if row.is_expanded() {
                        state.settings_details_open.insert(entry.id.clone());
                    } else {
                        state.settings_details_open.remove(&entry.id);
                        if state
                            .selected_request
                            .as_ref()
                            .is_some_and(|item| item.id == entry.id)
                        {
                            state.selected_request = None;
                        }
                    }
                });
            });
            column.append(&row);
        }
    }

    fn diagnostics_copy_button(&self, details: String) -> gtk::Button {
        let copied = self.with_state(|state| {
            state
                .diagnostics_copied_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        });
        let copy = gtk::Button::with_label(if copied { "Copied" } else { "Copy details" });
        widgets::action_cursor(&copy);
        let weak = self.downgrade();
        copy.connect_clicked(move |copy| {
            copy.clipboard().set_text(&details);
            let Some(app) = weak.upgrade() else {
                return;
            };
            let copied_at = std::time::Instant::now();
            app.update_state(|state| state.diagnostics_copied_at = Some(copied_at));
            copy.set_label("Copied");
            let weak = weak.clone();
            glib::timeout_add_local_once(Duration::from_secs(2), move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                let expired = app.update_state(|state| {
                    if state.diagnostics_copied_at == Some(copied_at) {
                        state.diagnostics_copied_at = None;
                        true
                    } else {
                        false
                    }
                });
                if expired && app.page() == Page::Settings(Pane::Diagnostics) {
                    app.render_settings();
                }
            });
        });
        copy
    }

    fn diagnostics_pane(&self, column: &gtk::Box) {
        self.request_log_view(column);
        let (section, group) = settings_section("Support", None);
        let logs = button("Open log file", &[], self.on(App::open_logs));
        logs.set_sensitive(self.with_state(|state| {
            state
                .status
                .as_ref()
                .and_then(|s| s.directory.as_ref())
                .is_some()
        }));
        group.append(
            &settings_row(
                "Log file",
                "What the background service recorded, for a bug report.",
                "text-x-generic-symbolic",
                Some(logs.upcast_ref()),
                &[],
            )
            .0,
        );
        column.append(&section);
        column.append(&self.diagnostics_view());
    }

    fn open_logs(&self) {
        let Some(directory) = self.with_state(|state| {
            state
                .status
                .as_ref()
                .and_then(|status| status.directory.clone())
        }) else {
            return;
        };
        self.record(
            TelemetryOperation::OpenLogs,
            TelemetryOutcome::Started,
            None,
        );
        let log = PathBuf::from(directory).join("collector.log");
        let shown = if log.exists() {
            log
        } else {
            log.parent().map(PathBuf::from).unwrap_or(log)
        };
        self.inner
            .desktop
            .open_uri(&gio::File::for_path(shown).uri());
    }

    // The relay workspace.

    /// Fetches the account workspace's relay key with the sign-in.
    pub fn connect_workspace(&self) {
        let Some(workspace) = self.with_state(|state| {
            state
                .account()
                .workspace_id
                .filter(|_| !state.connecting_workspace())
        }) else {
            return;
        };
        self.update_state(|state| {
            state.connect_requested = true;
            state.workspace_connect_error = None;
        });
        self.relay_changed();
        self.view_call::<agent_relay_desktop_core::contract::method::AccountConnectWorkspace>(
            agent_relay_desktop_core::contract::account::ConnectWorkspaceParams {
                workspace_id: workspace,
            },
            |app, result| {
                app.update_state(|state| {
                    state.connect_requested = false;
                    state.workspace_connect_error = result.err().map(|error| error.message);
                });
                app.relay_changed();
                app.refresh_local();
            },
        );
    }

    pub(super) fn cancel_connect_workspace(&self) {
        self.call::<agent_relay_desktop_core::contract::method::AccountSignInCancel>(NoParams {}, |_, _| {});
    }
}

fn update_detail(kind: InstallKind) -> &'static str {
    match kind {
        InstallKind::Tarball => "Install this update and reopen Agent Relay.",
        InstallKind::Deb => {
            "Download the verified package, then install it with your package manager."
        }
        InstallKind::Source => "This copy runs from source. Pull the latest changes to update it.",
    }
}

mod account;
mod background_service;
mod gateway_rows;
mod general;
mod hook_rows;
