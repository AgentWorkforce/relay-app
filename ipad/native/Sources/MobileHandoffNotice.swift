import Foundation

/// Display-only state published by the desktop after a verified handoff
/// transition. A receipt, an open conversation, or a local send is not proof.
struct MobileHandoffNotice: Equatable, Hashable, Sendable {
    enum State: String, Sendable { case connecting, ready, failed }
    let state: State
    let detail: String

    init?(metadata: [String: Any]) {
        guard let raw = metadata["mobile_handoff_state"] as? String,
              let state = State(rawValue: raw) else { return nil }
        self.state = state
        detail = state == .failed
            ? String(((metadata["mobile_handoff_error"] as? String) ?? "").prefix(500))
            : ""
    }

    /// The core's generic text for a failed or in-doubt Claude submission.
    /// It is replaced with app guidance; any other detail is shown as sent.
    static let unconfirmedDetail = "Claude's mobile connection could not be confirmed. Check the message status before sending again."
    static let unconfirmedGuidance = "Your last message may not have reached the session. Check the status under it and send it again only if it shows Queued or Sent, not Delivered."
    static let unconfirmedHint = "If this keeps happening, keep the computer awake and the session running, and run /reload-plugins once in a session opened before mobile control was set up."

    /// The text shown under the title. Nothing here sends a message again.
    var displayDetail: String {
        detail == Self.unconfirmedDetail ? Self.unconfirmedGuidance : detail
    }
    /// A second line shown only with the guidance for the generic text.
    var displayHint: String? {
        detail == Self.unconfirmedDetail ? Self.unconfirmedHint : nil
    }

    /// The core publishes this state only for Claude terminal sessions, so
    /// the titles name Claude.
    var title: String {
        switch state {
        case .connecting: return "Connecting to Claude…"
        case .ready: return "Continuing in Claude"
        case .failed: return "Could not continue Claude"
        }
    }
}


extension RelayAgent {
    /// Capability metadata is separate from Claude's live handoff status.
    var nativeContinuationNotice: String? {
        guard mobileNativeContinuationEnabled && isDesktopRegistration else { return nil }
        let approvals = cli == "codex" && mobileNativeCommandFileApprovals
            ? "You can approve or deny Codex command and file requests for messages sent here. Other prompts stay on the computer."
            : "Tool approvals stay on the computer."
        return "The verified computer owner's messages continue this conversation. \(approvals) Keep it awake and the session running."
    }

    /// A provider name alone does not opt this session into native handoff.
    func deliveryLabel(delivered: Bool) -> String {
        if delivered { return "Delivered" }
        if mobileNativeContinuationEnabled && isDesktopRegistration { return "Queued" }
        return mobileHandoffEnabled && isDesktopRegistration && ["claude", "claudecode"].contains(cli)
            ? "Queued" : "Sent"
    }
}
