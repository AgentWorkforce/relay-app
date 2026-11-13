import Foundation

/// Whether the session list shows a session as active: its Status column,
/// `is:active` and the menu's counts. desktop-core keeps the same rule in
/// `history/session_model/session_activity.rs` (GTK); both are tested
/// against `shared/session-active-cases.json`.
extension LocalSession {
    /// A Devin Cloud session's host, as the core names it.
    public static let devinCloudHost = "Devin Cloud"

    /// How recently a Devin Cloud session must have been active to count as
    /// active. A Devin Cloud row stays `live` for as long as the org's
    /// catalog remembers it — a message is how a quiet cloud session
    /// resumes — so `live` alone would call weeks-old sessions active. The
    /// same window decides whether one joins the relay by itself.
    public static let devinCloudActiveWindowMs: Int64 = 24 * 60 * 60 * 1000

    /// The longest the app waits before judging activity again even when no
    /// session is due to change: a sleep or a clock set forward can carry the
    /// time past a change a timer was set for.
    public static let activityRecheckMs: Int64 = 15 * 60 * 1000

    /// Whether this is a Devin Cloud session: listed from the org's catalog,
    /// not a `devin` process on this computer.
    public var isDevinCloud: Bool { target.kind == .devin && host == Self.devinCloudHost }

    /// Whether the session list shows this session as active at `nowMs`: it
    /// runs, can take a message now, and — for a Devin Cloud session, which
    /// can always take one — was active within `devinCloudActiveWindowMs`.
    /// A time up to one window ahead (a local clock running slow) still
    /// counts; one further ahead proves nothing.
    public func isActive(nowMs: Int64) -> Bool {
        guard live, running != false else { return false }
        guard isDevinCloud else { return true }
        guard let at = updatedAtMs else { return false }
        let window = Self.devinCloudActiveWindowMs
        return (-window...window).contains(nowMs &- at)
    }

    /// The first time after `nowMs` at which this session changes between
    /// active and not by time alone, or nil when only new data can change
    /// it. A Devin Cloud session becomes active once its time is no more
    /// than a window ahead, and stops a window after it.
    public func activityChange(afterMs nowMs: Int64) -> Int64? {
        guard live, running != false, isDevinCloud, let at = updatedAtMs else { return nil }
        let window = Self.devinCloudActiveWindowMs
        return [at &- window, at &+ window &+ 1].first { $0 > nowMs }
    }
}

extension Sequence where Element == LocalSession {
    /// When to judge activity again after `nowMs`: the first change among
    /// these sessions, and never later than `activityRecheckMs`. Status and
    /// `is:active` results kept until then cannot change by time alone.
    public func nextActivityCheck(afterMs nowMs: Int64) -> Int64 {
        compactMap { $0.activityChange(afterMs: nowMs) }.reduce(nowMs &+ LocalSession.activityRecheckMs, Swift.min)
    }

    /// These sessions' activity at `nowMs` as one value: it changes exactly
    /// when some session's Status or `is:active` match does.
    public func activityRevision(atMs nowMs: Int64) -> Int {
        var hasher = Hasher()
        for session in self where session.isDevinCloud {
            hasher.combine(session.target.id)
            hasher.combine(session.isActive(nowMs: nowMs))
        }
        return hasher.finalize()
    }
}

/// Now, in the milliseconds the core reports times in.
public func currentTimeMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }
