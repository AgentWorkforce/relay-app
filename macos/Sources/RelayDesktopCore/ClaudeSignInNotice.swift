import Foundation

/// The one notification posted when Claude Code cloud sessions stop for a
/// reason Claude Code's own sign-in fixes (`claude_code_sign_in`). Linux and
/// Windows keep the same words and rule in `models::claude_sign_in_notice`.
public enum ClaudeSignInNotice {
    public static let title = "Claude Code cloud sessions stopped"
    public static let body = "Sign in to Claude Code to resume them."
    /// The notification's button; it runs the same sign-in as Settings'
    /// *Sign in to Claude Code*.
    public static let actionLabel = "Sign in"

    /// The notification category and its button's identifier.
    public static let categoryID = "claude-code-sign-in"
    public static let signInActionID = "claude-code-sign-in.sign-in"

    /// The UserDefaults key that remembers this episode was notified, so a
    /// relaunch while it lasts posts nothing.
    public static let notifiedKey = "claudeCodeSignInNotified.v1"

    /// What to do with the latest health: post or not, and the flag to keep.
    public struct Decision: Equatable, Sendable {
        public let notify: Bool
        public let notified: Bool

        public init(notify: Bool, notified: Bool) {
            self.notify = notify
            self.notified = notified
        }
    }

    /// One notification per episode: posted when the sign-in is first
    /// offered, never again while it stays offered, and allowed again once
    /// the core reports health without it. No health (not reported yet,
    /// signed out, cloud sessions off) changes nothing.
    public static func decide(_ health: ClaudeCloudHealth?, notified: Bool) -> Decision {
        guard let health else { return Decision(notify: false, notified: notified) }
        guard ClaudeCloudHealthCopy.offersSignIn(health) else { return Decision(notify: false, notified: false) }
        return Decision(notify: !notified, notified: true)
    }
}
