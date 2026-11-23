import Foundation

/// Failed handoff notices the person dismissed, held in memory only. A
/// dismissal is keyed on the agent's identity, the state and the detail: it
/// stays hidden across refreshes of the same failure, and the notice comes
/// back when the published notice changes or is seen to disappear.
struct MobileHandoffDismissals: Equatable {
    private var dismissed: [String: MobileHandoffNotice] = [:]

    /// One registration: a re-registration under the same name has a new
    /// session id and so its own dismissal.
    static func identity(name: String, sessionID: String) -> String {
        "\(name)\n\(sessionID)"
    }

    /// Only a failed notice can be dismissed.
    mutating func dismiss(_ notice: MobileHandoffNotice, for identity: String) {
        guard notice.state == .failed else { return }
        dismissed[identity] = notice
    }

    /// Forgets one agent's dismissal. The person sending to that agent again
    /// is a new attempt, so its failure is shown even if it repeats the old
    /// detail exactly.
    mutating func forget(_ identity: String) {
        dismissed[identity] = nil
    }

    func hides(_ notice: MobileHandoffNotice, for identity: String) -> Bool {
        dismissed[identity] == notice
    }

    /// Forgets every dismissal whose agent no longer publishes exactly that
    /// notice, so the same failure published again later is shown again.
    func retaining(published: [String: MobileHandoffNotice]) -> MobileHandoffDismissals {
        var result = self
        result.dismissed = dismissed.filter { published[$0.key] == $0.value }
        return result
    }
}
