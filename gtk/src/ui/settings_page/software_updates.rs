//! Presentation and beta-channel preference for software updates.
use super::*;
use crate::updates::Release;

pub(super) fn headline(state: UpdateState, release: Option<&Release>) -> String {
    match state {
        UpdateState::Idle => "Agent Relay".into(),
        UpdateState::Checking => "Checking for updates…".into(),
        UpdateState::UpToDate => "Agent Relay is up to date".into(),
        UpdateState::Unavailable => "Updates unavailable".into(),
        UpdateState::Failed => "Update failed".into(),
        UpdateState::Available | UpdateState::Downloading | UpdateState::Installing => {
            match release {
                Some(release) => format!("Agent Relay {} is available", release.version),
                None => "Update available".into(),
            }
        }
    }
}

pub(super) fn detail(
    state: UpdateState,
    include_beta: bool,
    kind: InstallKind,
    message: Option<String>,
    checked: Option<String>,
) -> String {
    let version = agent_relay_node::VERSION;
    match state {
        UpdateState::Idle => format!("You’re running version {version}."),
        UpdateState::Checking => if include_beta {
            "Looking for the latest beta or stable release."
        } else {
            "Looking for the latest stable release."
        }
        .into(),
        UpdateState::UpToDate => format!(
            "You’re running version {version}. No newer release is available. Checked {}.",
            checked.as_deref().unwrap_or("just now")
        ),
        UpdateState::Available => update_detail(kind).into(),
        UpdateState::Downloading => "Downloading and verifying the update…".into(),
        UpdateState::Installing => {
            "Installing the update. Agent Relay will reopen when it finishes.".into()
        }
        UpdateState::Unavailable | UpdateState::Failed => message.unwrap_or_default(),
    }
}

impl App {
    pub(super) fn beta_update_row(&self, state: UpdateState) -> gtk::Box {
        let weak = self.downgrade();
        toggle_row(
            "Include beta releases",
            "Try upcoming features before they reach the stable channel. Beta releases may be less reliable. Turning this off keeps your current version until a newer stable release is available.",
            "applications-science-symbolic",
            self.prefs().include_beta_releases(),
            !state.busy(),
            move |enabled| {
                if let Some(app) = weak.upgrade() {
                    app.set_beta_updates(enabled);
                }
            },
        )
    }

    pub fn set_beta_updates(&self, enabled: bool) {
        if self.inner.updating.get() || self.with_state(|state| state.update_state.busy()) {
            return;
        }
        let _ = self
            .inner
            .prefs
            .borrow_mut()
            .set_include_beta_releases(enabled);
        self.update_state(|state| {
            state.update_release = None;
            state.update_message = None;
            state.update_checked = None;
            state.update_state = UpdateState::Idle;
        });
        self.check_updates(true);
    }
}
