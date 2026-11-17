import Foundation

/// How Settings labels the settings the organization's workspace policy
/// decides (`settings.get`'s `organization_policy`), and what it says about
/// messages the organization holds. Linux and Windows:
/// `models::organization_policy`, word for word.
public enum OrganizationPolicyCopy {
    /// A setting the organization set: the person cannot change it.
    public static let locked = "This setting is set by your organization."
    /// A setting whose value came from the organization as a starting point.
    public static let managed = "Managed by your organization."
    /// A setting the organization would turn on, which only takes effect on a
    /// computer it enrolled with consent for that setting.
    public static let mismatched = "Your organization turns this on only on computers it enrolls. This computer keeps your own choice."
    /// Why a sender cannot be allowed for good while the organization holds
    /// messages from senders who are not its admins.
    public static let held = "Your organization holds messages from senders who are not its admins. Approve each message as it arrives."
    /// The message approvals section while the organization holds messages
    /// in every harness, not only Claude Code's.
    public static let approvalsTitle = "Message approvals"
    /// The repositories whose sessions join the relay by themselves.
    public static let repositoriesTitle = "Repositories that join the relay"
    public static let repositoriesDetail = "Sessions working in these repositories join the relay by themselves."
    /// The empty list.
    public static let repositoriesNone = "None"

    /// The note under `key`'s row, when the organization decides it.
    public static func note(_ policy: OrganizationPolicyState?, _ key: String) -> String? {
        guard let policy else { return nil }
        if policy.locked.contains(key) { return locked }
        if policy.mismatched.contains(key) { return mismatched }
        if policy.defaults.contains(key) { return managed }
        return nil
    }

    /// Whether the organization locks `key`, so the person cannot change it.
    public static func isLocked(_ policy: OrganizationPolicyState?, _ key: String) -> Bool {
        policy?.locked.contains(key) ?? false
    }

    /// The first note among `keys` (a row that changes several settings).
    public static func note(_ policy: OrganizationPolicyState?, any keys: [String]) -> String? {
        keys.lazy.compactMap { note(policy, $0) }.first
    }

    /// Whether the organization locks any of `keys`.
    public static func isLocked(_ policy: OrganizationPolicyState?, any keys: [String]) -> Bool {
        keys.contains { isLocked(policy, $0) }
    }

    /// The repository an agent's session is in, as its registration names it
    /// (`agents.list` `repository`).
    public static func repository(_ name: String) -> String {
        "Repository: \(name)"
    }

    /// The repositories list as one line.
    public static func repositories(_ entries: [String]) -> String {
        entries.isEmpty ? repositoriesNone : entries.joined(separator: ", ")
    }
}
