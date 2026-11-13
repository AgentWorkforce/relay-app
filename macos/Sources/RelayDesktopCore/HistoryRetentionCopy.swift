import Foundation

/// The session history setting's wording (`history_retention_days`), shared
/// with Linux (`agent_relay_app::models::HISTORY_RETENTION_*`): how long this
/// computer keeps local session history, and the note that history not yet
/// uploaded is kept longer.
public enum HistoryRetentionCopy {
    public static let section = "Session history"
    public static let title = "Keep session history"
    public static let detail = "How long this computer keeps the sessions Agent Relay finds. Older sessions are removed from this computer; copies already uploaded to your workspace are not affected."
    /// Days kept unless changed.
    public static let defaultDays = 30
    /// The setting's value for keeping history forever.
    public static let forever = 0
    /// The choices Settings offers, in order (the core's `CHOICES`).
    public static let choices = [7, 30, 90, forever]

    /// `7 days`, `30 days (default)`, `Forever`.
    public static func label(_ days: Int) -> String {
        switch days {
        case forever: return "Forever"
        case defaultDays: return "\(days) days (default)"
        case 1: return "1 day"
        default: return "\(days) days"
        }
    }

    /// Said under a limit when history can outlast it: sessions are never
    /// evicted before they upload, which takes longest while signed out or
    /// paused, and a short limit is the likeliest to be reached first.
    public static let note = "Unuploaded sessions are kept until delivery succeeds, so history may be retained longer than your selected limit."

    /// `note` for a choice shorter than the default, or any limit while
    /// uploads are held (signed out or paused).
    public static func warning(_ days: Int, uploadsHeld: Bool) -> String? {
        guard days != forever, uploadsHeld || days < defaultDays else { return nil }
        return note
    }
}
