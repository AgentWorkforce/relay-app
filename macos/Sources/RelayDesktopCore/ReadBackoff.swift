import Foundation

/// How long an open conversation waits before it is read again after a
/// read failed. A failed read waits the server's Retry-After when the error
/// names one (held to at least a second), otherwise a delay that starts at
/// the normal cadence, doubles with each failure in a row and stops at five
/// minutes, with up to a quarter more at random so many windows do not
/// retry together. A read that succeeds goes back to the normal cadence.
///
/// GTK's counterpart is `gtk/src/models/read_backoff.rs`; the two read the
/// same.
public struct ReadBackoff: Equatable, Sendable {
    /// How often an open conversation is read while reads succeed, and the
    /// wait after the first failure.
    public static let cadence: TimeInterval = 10
    /// The longest wait the doubling reaches.
    public static let cap: TimeInterval = 300
    /// The shortest wait a Retry-After is held to.
    public static let shortestRetryAfter: TimeInterval = 1
    /// The largest share of the wait added at random.
    public static let jitter = 0.25

    /// Failed reads in a row since the last one that succeeded.
    public private(set) var failures = 0

    public init() {}

    /// A read failed: how long to wait before the next one. `retryAfter` is
    /// the server's, when it named one; `random` (in `0..<1`) picks the
    /// jitter.
    public mutating func failed(retryAfter: TimeInterval? = nil,
                                random: Double = Double.random(in: 0..<1)) -> TimeInterval {
        failures += 1
        if let retryAfter { return max(retryAfter, Self.shortestRetryAfter) }
        let doubled = Self.cadence * pow(2, Double(min(failures - 1, 16)))
        let base = min(doubled, Self.cap)
        let share = min(max(random, 0), 1) * Self.jitter
        return min(base * (1 + share), Self.cap)
    }

    /// A read succeeded: the next waits the normal cadence.
    public mutating func succeeded() {
        failures = 0
    }

    /// What the conversation says while it waits `delay` after a read that
    /// failed with `message`.
    public static func failureText(_ message: String, retryingIn delay: TimeInterval) -> String {
        text("Could not read the conversation", message, delay)
    }

    /// What the conversation says while it waits `delay` after a look-up
    /// of its recipient failed with `message` (and its history did not).
    public static func recipientFailureText(_ message: String, retryingIn delay: TimeInterval) -> String {
        text("Could not look up the recipient", message, delay)
    }

    private static func text(_ what: String, _ message: String, _ delay: TimeInterval) -> String {
        var reason = message.trimmingCharacters(in: .whitespacesAndNewlines)
        while reason.hasSuffix(".") { reason.removeLast() }
        return "\(what): \(reason). Trying again in \(wait(delay))."
    }

    /// `delay` in whole seconds, rounded up: "20 s", "2 min 30 s", "5 min".
    public static func wait(_ delay: TimeInterval) -> String {
        let seconds = max(1, Int(delay.rounded(.up)))
        guard seconds >= 60 else { return "\(seconds) s" }
        let minutes = seconds / 60
        let rest = seconds % 60
        return rest == 0 ? "\(minutes) min" : "\(minutes) min \(rest) s"
    }
}
