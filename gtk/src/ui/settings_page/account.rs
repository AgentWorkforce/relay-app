//! Settings › Account: who is signed in, the workspace and its plan, and
//! logging out.

mod caller_requests;

use crate::models::{plan_manage_url, plan_summary, plan_title, PLAN_MANAGE_ACTION};
use crate::ui::app::App;
use crate::ui::widgets::{
    self, avatar, button, hbox, settings_row, settings_section, text, vbox, LabelStyle,
};
use gtk::prelude::*;
use gtk4 as gtk;

impl App {
    pub(super) fn account_pane(&self, column: &gtk::Box) {
        let (person, workspace, signed_in, signing_out) = self.with_state(|state| {
            (
                state.account().person.unwrap_or_default(),
                state.workspace_name(),
                state.connected(),
                state.signing_out,
            )
        });
        let email = person.email.clone().filter(|email| !email.is_empty());
        let given = person.name.clone().filter(|name| !name.is_empty());
        let name = given
            .clone()
            .or(email.clone())
            .unwrap_or_else(|| "Not signed in".into());
        let (section, group) = settings_section("Profile", None);
        let profile = vbox(16, &[]);
        widgets::pad(&profile, 20, None, None);
        let who = hbox(14, &[]);
        who.append(&avatar(&name, person.avatar_url.as_deref(), 44));
        let words = vbox(5, &[]);
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Center);
        words.append(&text(&name, &["settings-profile-name"]).wrapped());
        if let (Some(_), Some(email)) = (&given, &email) {
            words.append(&text(email, &["secondary"]).wrapped().copyable());
        }
        who.append(&words);
        let log_out = button("Log out…", &[], self.on(App::confirm_logout));
        log_out.set_sensitive(signed_in && !signing_out);
        log_out.set_valign(gtk::Align::Center);
        who.append(&log_out);
        profile.append(&who);
        profile.append(&text("Logging out stops uploads and disconnects this app. Local sessions stay where they are.", &["settings-row-detail"]).wrapped());
        group.append(&profile);
        column.append(&section);
        self.account_health_section(column);
        self.caller_requests_section(column);
        let (section, group) = settings_section("Workspace", None);
        group.append(&settings_row(&workspace,
            "Shared sessions belong to this workspace. Log out and sign in again to switch workspaces.",
            "network-workgroup-symbolic", None, &[]).0);
        let (plan, usage) = self.with_state(|state| {
            let workspace = state.workspace();
            (workspace.plan, workspace.usage)
        });
        if let Some(plan) = plan {
            let manage = button(PLAN_MANAGE_ACTION, &["primary"], self.on(App::open_plan));
            manage.set_tooltip_text(Some("Open your plan in Agent Relay"));
            manage.set_valign(gtk::Align::Center);
            let (row, _) = settings_row(
                &plan_title(&plan),
                &plan_summary(&plan, usage.as_ref()),
                "emblem-ok-symbolic",
                Some(manage.upcast_ref()),
                &[],
            );
            widgets::name(&row, "workspace-plan");
            group.append(&row);
        }
        column.append(&section);
    }

    /// Manage plan / Upgrade plan: the plan's own link, else Cloud Billing.
    pub fn open_plan(&self) {
        let url = self.with_state(|state| {
            let site = state.account().site_url;
            plan_manage_url(state.workspace().plan.as_ref(), site.as_deref())
        });
        if let Some(url) = url {
            self.open_url(&url);
        }
    }
}
