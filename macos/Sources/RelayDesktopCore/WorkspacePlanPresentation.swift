import Foundation

/// What the apps say about the workspace's Agent Relay plan: the plan line
/// in Settings, a cap that was reached, humans at their cap on Team, and
/// history the plan's retention left out. The Linux app's
/// `models/plan.rs` says the same, word for word.
public enum WorkspacePlanPresentation {
    /// Opens the plan's `upgrade_url` (Cloud Billing when Cloud sent none).
    public static let manageAction = "Manage plan"
    /// The link under a cap or retention notice.
    public static let upgradeAction = "Upgrade plan"

    /// "Intro plan".
    public static func title(_ plan: WorkspacePlan) -> String {
        "\(plan.name) plan"
    }

    /// The plan's caps against what the workspace uses, as far as the core
    /// has counted: "12 of 15 agents · 1 of 2 humans · up to 5 devices ·
    /// 7-day session history". A cap not counted yet reads "up to"; no cap
    /// reads "unlimited".
    public static func summary(_ plan: WorkspacePlan, usage: WorkspaceUsage?) -> String {
        [
            counted(plan.limits.agents, used: usage?.agents, noun: "agents"),
            counted(plan.limits.humans, used: usage?.humans, noun: "humans"),
            counted(plan.limits.devices, used: nil, noun: "devices"),
            retention(plan.limits.sessionRetentionDays),
        ].joined(separator: " · ")
    }

    static func counted(_ limit: Int?, used: Int?, noun: String) -> String {
        guard let limit else { return "unlimited \(noun)" }
        guard let used else { return "up to \(limit) \(noun)" }
        return "\(used) of \(limit) \(noun)"
    }

    static func retention(_ days: Int?) -> String {
        guard let days else { return "unlimited session history" }
        return "\(days)-day session history"
    }

    /// Where Manage plan goes: the plan's own link, else Cloud Billing on
    /// the signed-in site.
    public static func manageURL(_ plan: WorkspacePlan?, siteURL: String) -> URL? {
        if let upgrade = plan?.upgradeUrl, let url = URL(string: upgrade), url.scheme == "https" { return url }
        return RelaycastUpgrade.billingURL(siteURL: siteURL)
    }

    // MARK: A cap reached

    /// "Agent limit reached".
    public static func limitTitle(_ refusal: PlanLimitExceeded) -> String {
        switch refusal.limit {
        case .devices: "Device limit reached"
        case .agents: "Agent limit reached"
        case .humans: "Human limit reached"
        case .other: "Plan limit reached"
        }
    }

    /// Why, with the counters: the core's sentence for the same refusal.
    public static func limitMessage(_ refusal: PlanLimitExceeded) -> String {
        "This workspace is using \(refusal.current) of \(refusal.max) \(noun(refusal.limit)) on the \(planName(refusal.plan)) plan. Upgrade the plan to add more."
    }

    static func noun(_ limit: PlanLimitKind) -> String {
        switch limit {
        case .devices: "devices"
        case .agents: "agents"
        case .humans: "humans"
        case .other: "items"
        }
    }

    /// "Intro" for `intro`.
    public static func planName(_ key: String) -> String {
        let key = key.trimmingCharacters(in: .whitespaces)
        guard let first = key.first else { return "" }
        return first.uppercased() + key.dropFirst()
    }

    // MARK: Team

    /// The line Team shows while every human seat the plan allows is taken,
    /// nil otherwise (no plan, no cap, or seats left).
    public static func humansAtCap(_ plan: WorkspacePlan?, usage: WorkspaceUsage?) -> String? {
        guard let plan, let limit = plan.limits.humans, let humans = usage?.humans, humans >= limit else { return nil }
        return "Your workspace has all \(limit) humans the \(plan.name) plan allows. Upgrade the plan to invite more."
    }

    // MARK: History

    /// What a truncated history says, under Load older activity.
    public static func retentionNotice(_ retention: HistoryRetention, plan: WorkspacePlan?) -> String {
        let days = retention.days == 1 ? "1 day" : "\(retention.days) days"
        guard let plan else { return "Sessions older than \(days) aren't available on your plan." }
        return "Sessions older than \(days) aren't available on the \(plan.name) plan."
    }
}
