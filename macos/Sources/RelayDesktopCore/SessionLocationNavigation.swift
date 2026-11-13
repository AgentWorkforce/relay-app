import Foundation

/// Presentation of the shared core's decision. Platform clients execute the
/// plan; they do not reconstruct ownership or choose a different destination.
public enum SessionLocationNavigation {
    public static func offersDefaultResume(_ plan: SessionOpenPlan, afterFailure: Bool) -> Bool {
        plan.canResumeDefault && (plan.action == .unavailable || afterFailure)
    }

    public static func title(_ plan: SessionOpenPlan) -> String {
        switch plan.action {
        case .focus: return "Open in \(plan.location?.application ?? "original terminal")"
        case .showApplication: return "Show \(plan.location?.application ?? "application")"
        case .openLink: return plan.url?.hasPrefix("codex:") == true ? "Open in Codex" : "Open in Claude"
        case .resume:
            if plan.command?.args.contains("--desktop") == true { return "Open in Claude" }
            return "Resume in \(plan.location?.application ?? "Terminal")"
        case .choose: return "Open running session"
        case .unavailable: return "Session opening unavailable"
        case .unknown: return "Session opening unavailable"
        }
    }
}
