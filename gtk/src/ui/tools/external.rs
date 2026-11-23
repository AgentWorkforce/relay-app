//! Setup routes for hosted apps and cloud providers, without local MCP installation.
use super::*;
use crate::models::CloudProviderSetup;
use crate::ui::widgets::{self, button, hbox, icon, settings_divider, settings_section, text, vbox, LabelStyle};
use gtk::prelude::*;
use gtk4 as gtk;

pub(super) fn is_external(id: &str) -> bool {
    matches!(
        id,
        "conductor" | "paseo" | "capy" | "grok-bot" | "dot" | "muse-assistant" | "replicas" | "claude-cloud" | "amp"
    )
}

/// Amp's one action: its token lives on the Cloud workspace, never here.
pub(super) const AMP_ACTION: &str = "Open Cloud connections";
/// Amp's connection is claimed only when Agent Relay Cloud reports it
/// configured; otherwise the status says how Amp connects.
pub(super) const AMP_CONNECTED: &str = "Connected in Agent Relay Cloud";
pub(super) const AMP_CONNECTS: &str = "Connects through Agent Relay Cloud";

/// The signed-in site's Cloud connections page, or the built-in site's.
pub(super) fn amp_connections(site: Option<&str>) -> Option<String> {
    agent_relay_desktop_core::mcp_settings::amp_connections_url(
        site.unwrap_or(agent_relay_node::relay::links::built_in_site_url()),
    )
}

pub(super) fn agent_providers(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "paseo" => &[
            ("Codex settings", "codex"),
            ("Claude Code settings", "claude"),
            ("Grok settings", "grok"),
            ("Muse Code settings", "muse"),
        ],
        "claude-cloud" => &[("Claude Code settings", "claude")],
        "conductor" => &[
            ("Codex settings", "codex"),
            ("Claude Code settings", "claude"),
        ],
        _ => &[],
    }
}

impl App {
    pub(in crate::ui) fn external_provider_setup(&self, column: &gtk::Box, id: &str) {
        let providers = agent_providers(id);
        if providers.is_empty() {
            return;
        }
        let (section, group) = settings_section("Agent settings", None);
        for (index, &(label, target)) in providers.iter().enumerate() {
            if index > 0 {
                settings_divider(&group, 50);
            }
            let link = button(
                label,
                &["flat"],
                self.on(move |app| app.open_provider(target)),
            );
            let row = hbox(12, &[]);
            widgets::pad(&row, 6, Some(4), None);
            row.append(&widgets::agent_badge(target, 22));
            let title = text(label, &["settings-row-title"]);
            title.set_hexpand(true);
            title.set_halign(gtk::Align::Start);
            row.append(&title);
            row.append(&icon("go-next-symbolic", 14));
            link.set_child(Some(&row));
            group.append(&link);
        }
        column.append(&section);
    }

    pub(super) fn cloud_setup_skill_link(&self, id: &str) -> Option<gtk::Button> {
        let url = CloudProviderSetup::for_id(id)?.skill_url()?;
        Some(button("Read setup skill", &["flat"], self.on(move |app| {
            app.inner.desktop.open_uri(url);
        })))
    }

    pub(super) fn cloud_setup_snippet(&self, id: &str) -> Option<gtk::Box> {
        let setup = CloudProviderSetup::for_id(id)?;
        if !setup.copy_only() { return None; }
        let column = vbox(10, &[]);
        let block = vbox(0, &["transcript-code-block"]);
        block.append(&text(setup.prompt()?, &["mono"]).wrapped().copyable());
        column.append(&block);
        let actions = hbox(12, &[]);
        actions.append(&button("Open Muse", &["flat"], self.on(move |app| app.open_cloud_provider_app(setup))));
        if let Some(link) = self.cloud_setup_skill_link(id) { actions.append(&link); }
        column.append(&actions);
        Some(column)
    }

    fn open_cloud_provider_app(&self, setup: CloudProviderSetup) {
        if let Some(url) = setup.app_url().filter(|url| self.inner.desktop.has_uri_handler(url.scheme())) {
            self.inner.desktop.open_app_uri(url.as_str(), setup.web_url());
        } else {
            self.inner.desktop.open_uri(setup.web_url());
        }
    }

    /// Open the workspace's Cloud connections in the browser, where Amp connects.
    pub(super) fn amp_connections_button(&self, id: &str) -> Option<gtk::Button> {
        (id == "amp").then(|| {
            button(AMP_ACTION, &["primary"], self.on(|app| {
                let site = app.with_state(|state| state.account().site_url);
                if let Some(url) = amp_connections(site.as_deref()) {
                    app.open_url(&url);
                }
            }))
        })
    }

    /// Copy a manual setup snippet or open a supported handoff; the person sends it.
    pub(super) fn cloud_setup_button(&self, id: &str) -> Option<gtk::Button> {
        let setup = CloudProviderSetup::for_id(id)?;
        let id = id.to_string();
        Some(button(
            setup.action(),
            &["primary"],
            self.on(move |app| {
                if let Some(prompt) = setup.prompt() { widgets::copy_to_clipboard(prompt); }
                if !setup.copy_only() { app.open_cloud_provider_app(setup); }
                app.update_state(|state| {
                    state.tools.setup_started.insert(id.clone());
                    if setup == CloudProviderSetup::Capy { state.capy_prompt_copied = true; }
                });
                if !app.render_tools_onboarding() { app.render_settings(); }
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amp_opens_the_cloud_connections_tab_on_the_signed_in_site() {
        assert!(is_external("amp"));
        assert_eq!(AMP_ACTION, "Open Cloud connections");
        assert_eq!(
            amp_connections(Some("https://agentrelay.com")).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings#connections")
        );
        assert_eq!(
            amp_connections(None),
            amp_connections(Some(agent_relay_node::relay::links::built_in_site_url()))
        );
        assert_eq!(amp_connections(Some("file:///tmp")), None);
    }
}
