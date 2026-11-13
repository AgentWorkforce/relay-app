//! Settings › About: the build identity, the Mac's `BuildIdentitySection`
//! (`AboutView.swift`), after Software updates: how a profile
//! build is told apart from Agent Relay and other builds on this computer.
//! It names the profile, the app id, the built-in site's host (never its
//! path or query) and the core's socket; the default profile shows nothing,
//! so Agent Relay's About is unchanged.

use super::app::App;
use super::widgets::{self, hbox, settings_section, text, vbox, LabelStyle};
use crate::build_profile::Profile;
use gtk::prelude::*;
use gtk4 as gtk;
use std::path::Path;

impl App {
    /// Settings › About: Software updates, then the build identity.
    pub(super) fn about_and_identity(&self, column: &gtk::Box) {
        self.about_pane(column);
        append_build_identity(column);
    }
}

/// The section's rows, label then value; empty for the default profile. A
/// value that cannot be told reads "Unavailable", as on the Mac.
fn identity_rows(
    profile: Profile,
    app_id: &str,
    site: &str,
    socket: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let Some(identity) = profile.identity() else {
        return Vec::new();
    };
    let known = |value: Option<String>| {
        value
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Unavailable".to_owned())
    };
    vec![
        ("Profile", identity.to_owned()),
        ("App ID", app_id.to_owned()),
        ("Site", known(site_host(site))),
        ("Core socket", known(socket.and_then(socket_name))),
    ]
}

/// The site's host alone: no scheme, port, path, query or fragment.
fn site_host(site: &str) -> Option<String> {
    url::Url::parse(site).ok()?.host_str().map(str::to_owned)
}

/// The core socket as the person can find it: the socket file's name.
#[cfg(not(windows))]
fn socket_name(socket: &Path) -> Option<String> {
    Some(socket.file_name()?.to_string_lossy().into_owned())
}

/// On Windows the socket path only names the core's pipe; the pipe is
/// what the person can find.
#[cfg(windows)]
fn socket_name(socket: &Path) -> Option<String> {
    Some(agent_relay_node::relay::transport::pipe_name(socket))
}

/// The socket the app's core listens on, resolved as the bridge does.
fn core_socket() -> Option<std::path::PathBuf> {
    let home = agent_relay_node::relay::paths::home().ok()?;
    agent_relay_node::relay::paths::CorePaths::resolve(None, |name| std::env::var(name).ok(), &home)
        .ok()
        .map(|paths| paths.socket)
}

/// The section, appended to `column` for a profile build only.
fn append_build_identity(column: &gtk::Box) {
    let rows = identity_rows(
        Profile::BUILT,
        super::app::APP_ID,
        agent_relay_node::relay::links::built_in_site_url(),
        core_socket().as_deref(),
    );
    if rows.is_empty() {
        return;
    }
    let (section, group) = settings_section(
        "Build identity",
        Some("How this build is told apart from Agent Relay and other builds on this computer."),
    );
    let lines = vbox(10, &[]);
    super::zoom::margins(&lines, 16, 16, 16, 16);
    for (label, value) in rows {
        let line = hbox(12, &[]);
        line.append(&text(label, &["secondary", "caption"]));
        line.append(&widgets::spacer());
        let value = text(&value, &["caption"]).wrapped().copyable();
        value.set_xalign(1.0);
        line.append(&value);
        lines.append(&line);
    }
    group.append(&lines);
    column.append(&section);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_profile::tests::OTHER;

    #[test]
    fn about_shows_a_profile_builds_identity_but_nothing_for_the_default() {
        let socket = Path::new("/home/me/.agentworkforce/desktop/core.sock");
        assert!(identity_rows(
            Profile::DEFAULT,
            "com.agentrelay.desktop",
            "https://agentrelay.com",
            Some(socket)
        )
        .is_empty());
        let rows = identity_rows(
            OTHER,
            "com.agentrelay.desktop",
            "https://dev.example.com:8443/app?token=secret#x",
            Some(socket),
        );
        let labels: Vec<_> = rows.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["Profile", "App ID", "Site", "Core socket"]);
        assert_eq!(rows[0].1, "example-7");
        assert_eq!(rows[1].1, "com.agentrelay.desktop");
        // The host alone: no port, path, query or token.
        assert_eq!(rows[2].1, "dev.example.com");
        assert!(rows.iter().all(|(_, value)| !value.contains("secret")));
        #[cfg(not(windows))]
        assert_eq!(rows[3].1, "core.sock");
        #[cfg(windows)]
        assert!(
            rows[3].1.starts_with(r"\\.\pipe\agent-relay-"),
            "{}",
            rows[3].1
        );
        let unknown = identity_rows(OTHER, "com.agentrelay.desktop", "not a url", None);
        assert_eq!(unknown[2].1, "Unavailable");
        assert_eq!(unknown[3].1, "Unavailable");
    }
}
