//! What the app says about the workspace's Agent Relay plan: the plan line
//! in Settings, a cap that was reached, humans at their cap on Team, and
//! history the plan's retention left out. The Mac's
//! `WorkspacePlanPresentation` says the same, word for word.

use agent_relay_node::relay::contract::plan::{
    plan_name, HistoryRetention, PlanLimitExceeded, PlanLimitKind, WorkspacePlan, WorkspaceUsage,
};

/// Opens the plan's `upgrade_url` (Cloud Billing when Cloud sent none).
pub const PLAN_MANAGE_ACTION: &str = "Manage plan";
/// The link under a cap or retention notice.
pub const PLAN_UPGRADE_ACTION: &str = "Upgrade plan";

/// "Intro plan".
pub fn plan_title(plan: &WorkspacePlan) -> String {
    format!("{} plan", plan.name)
}

/// The plan's caps against what the workspace uses, as far as the core has
/// counted: "12 of 15 agents · 1 of 2 humans · up to 5 devices · 7-day
/// session history". A cap not counted yet reads "up to"; no cap reads
/// "unlimited".
pub fn plan_summary(plan: &WorkspacePlan, usage: Option<&WorkspaceUsage>) -> String {
    [
        counted(
            plan.limits.agents,
            usage.and_then(|usage| usage.agents),
            "agents",
        ),
        counted(
            plan.limits.humans,
            usage.and_then(|usage| usage.humans),
            "humans",
        ),
        counted(plan.limits.devices, None, "devices"),
        retention(plan.limits.session_retention_days),
    ]
    .join(" · ")
}

fn counted(limit: Option<u64>, used: Option<u64>, noun: &str) -> String {
    match (limit, used) {
        (None, _) => format!("unlimited {noun}"),
        (Some(limit), None) => format!("up to {limit} {noun}"),
        (Some(limit), Some(used)) => format!("{used} of {limit} {noun}"),
    }
}

fn retention(days: Option<u64>) -> String {
    match days {
        None => "unlimited session history".into(),
        Some(days) => format!("{days}-day session history"),
    }
}

/// Where Manage plan goes: the plan's own link, else Cloud Billing on the
/// signed-in site.
pub fn plan_manage_url(plan: Option<&WorkspacePlan>, site: Option<&str>) -> Option<String> {
    if let Some(url) = plan
        .and_then(|plan| plan.upgrade_url.as_deref())
        .filter(|url| url.starts_with("https://"))
    {
        return Some(url.to_string());
    }
    super::relaycast_upgrade_url(site, None)
}

/// "Agent limit reached".
pub fn plan_limit_title(refusal: &PlanLimitExceeded) -> &'static str {
    match refusal.limit {
        PlanLimitKind::Devices => "Device limit reached",
        PlanLimitKind::Agents => "Agent limit reached",
        PlanLimitKind::Humans => "Human limit reached",
        PlanLimitKind::Other => "Plan limit reached",
    }
}

/// Why, with the counters: the core's sentence for the same refusal.
pub fn plan_limit_message(refusal: &PlanLimitExceeded) -> String {
    refusal.message()
}

/// The one banner line: title and message.
pub fn plan_limit_notice(refusal: &PlanLimitExceeded) -> String {
    format!(
        "{}. {}",
        plan_limit_title(refusal),
        plan_limit_message(refusal)
    )
}

/// The line Team shows while every human seat the plan allows is taken,
/// `None` otherwise (no plan, no cap, or seats left).
pub fn humans_at_cap(
    plan: Option<&WorkspacePlan>,
    usage: Option<&WorkspaceUsage>,
) -> Option<String> {
    let plan = plan?;
    let limit = plan.limits.humans?;
    let humans = usage?.humans?;
    (humans >= limit).then(|| {
        format!(
            "Your workspace has all {limit} humans the {} plan allows. Upgrade the plan to invite more.",
            plan.name
        )
    })
}

/// What a truncated history says, under Load older activity.
pub fn retention_notice(retention: &HistoryRetention, plan: Option<&WorkspacePlan>) -> String {
    let days = if retention.days == 1 {
        "1 day".to_string()
    } else {
        format!("{} days", retention.days)
    };
    match plan {
        Some(plan) => format!(
            "Sessions older than {days} aren't available on the {} plan.",
            plan.name
        ),
        None => format!("Sessions older than {days} aren't available on your plan."),
    }
}

/// "Intro" for `intro`.
pub fn plan_display_name(key: &str) -> String {
    plan_name(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::contract::plan::PlanLimits;

    fn intro() -> WorkspacePlan {
        WorkspacePlan {
            key: "intro".into(),
            name: "Intro".into(),
            limits: PlanLimits {
                devices: Some(5),
                agents: Some(15),
                humans: Some(2),
                session_retention_days: Some(7),
            },
            upgrade_url: Some("https://agentrelay.com/cloud/dashboard/settings#billing".into()),
        }
    }

    fn enterprise() -> WorkspacePlan {
        WorkspacePlan {
            key: "enterprise".into(),
            name: "Enterprise".into(),
            limits: PlanLimits {
                session_retention_days: Some(365),
                ..Default::default()
            },
            upgrade_url: None,
        }
    }

    #[test]
    fn plan_wording_matches_the_mac() {
        assert_eq!(PLAN_MANAGE_ACTION, "Manage plan");
        assert_eq!(PLAN_UPGRADE_ACTION, "Upgrade plan");
        assert_eq!(plan_title(&intro()), "Intro plan");
        let usage = WorkspaceUsage {
            agents: Some(12),
            humans: Some(1),
        };
        assert_eq!(
            plan_summary(&intro(), Some(&usage)),
            "12 of 15 agents · 1 of 2 humans · up to 5 devices · 7-day session history"
        );
        assert_eq!(
            plan_summary(&intro(), None),
            "up to 15 agents · up to 2 humans · up to 5 devices · 7-day session history"
        );
        assert_eq!(
            plan_summary(&enterprise(), Some(&usage)),
            "unlimited agents · unlimited humans · unlimited devices · 365-day session history"
        );
        assert_eq!(plan_display_name("pro"), "Pro");
    }

    #[test]
    fn manage_plan_prefers_the_plans_link_and_falls_back_to_billing() {
        assert_eq!(
            plan_manage_url(Some(&intro()), Some("https://agentrelay.com")).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings#billing")
        );
        assert_eq!(
            plan_manage_url(Some(&enterprise()), Some("https://agentrelay.com")).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
        );
        assert_eq!(
            plan_manage_url(None, Some("http://local.test:3000")).as_deref(),
            Some("http://local.test:3000/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
        );
        let mut insecure = intro();
        insecure.upgrade_url = Some("http://evil.test/billing".into());
        assert_eq!(
            plan_manage_url(Some(&insecure), Some("https://agentrelay.com")).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
        );
    }

    #[test]
    fn a_reached_cap_names_the_cap_the_counters_and_the_plan() {
        let agents = PlanLimitExceeded {
            limit: PlanLimitKind::Agents,
            max: 15,
            current: 15,
            plan: "intro".into(),
        };
        assert_eq!(plan_limit_title(&agents), "Agent limit reached");
        assert_eq!(
            plan_limit_message(&agents),
            "This workspace is using 15 of 15 agents on the Intro plan. Upgrade the plan to add more."
        );
        assert_eq!(
            plan_limit_notice(&agents),
            "Agent limit reached. This workspace is using 15 of 15 agents on the Intro plan. Upgrade the plan to add more."
        );
        let devices = PlanLimitExceeded {
            limit: PlanLimitKind::Devices,
            max: 5,
            current: 5,
            plan: "intro".into(),
        };
        assert_eq!(plan_limit_title(&devices), "Device limit reached");
        let humans = PlanLimitExceeded {
            limit: PlanLimitKind::Humans,
            max: 10,
            current: 10,
            plan: "pro".into(),
        };
        assert_eq!(plan_limit_title(&humans), "Human limit reached");
        assert_eq!(
            plan_limit_message(&humans),
            "This workspace is using 10 of 10 humans on the Pro plan. Upgrade the plan to add more."
        );
        let other = PlanLimitExceeded {
            limit: PlanLimitKind::Other,
            max: 1,
            current: 1,
            plan: "pro".into(),
        };
        assert_eq!(plan_limit_title(&other), "Plan limit reached");
    }

    #[test]
    fn team_says_when_every_human_seat_is_taken() {
        let full = WorkspaceUsage {
            agents: None,
            humans: Some(2),
        };
        assert_eq!(
            humans_at_cap(Some(&intro()), Some(&full)).as_deref(),
            Some("Your workspace has all 2 humans the Intro plan allows. Upgrade the plan to invite more.")
        );
        let room = WorkspaceUsage {
            agents: None,
            humans: Some(1),
        };
        assert_eq!(humans_at_cap(Some(&intro()), Some(&room)), None);
        assert_eq!(humans_at_cap(Some(&intro()), None), None, "not counted yet");
        assert_eq!(
            humans_at_cap(Some(&enterprise()), Some(&full)),
            None,
            "no cap"
        );
        assert_eq!(humans_at_cap(None, Some(&full)), None, "no plan");
    }

    #[test]
    fn history_says_what_retention_left_out() {
        let seven = HistoryRetention {
            days: 7,
            cutoff: "2026-10-18T00:00:00Z".into(),
        };
        assert_eq!(
            retention_notice(&seven, Some(&intro())),
            "Sessions older than 7 days aren't available on the Intro plan."
        );
        assert_eq!(
            retention_notice(&seven, None),
            "Sessions older than 7 days aren't available on your plan."
        );
        let one = HistoryRetention {
            days: 1,
            cutoff: String::new(),
        };
        assert_eq!(
            retention_notice(&one, Some(&intro())),
            "Sessions older than 1 day aren't available on the Intro plan."
        );
    }
}
