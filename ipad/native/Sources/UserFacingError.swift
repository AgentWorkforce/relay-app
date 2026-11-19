import Foundation

/// The words a person sees when something they asked for failed. A server's
/// own error text, an HTTP status or a header (such as "Retry the request
/// after the interval in the Retry-After header") is never shown: it is
/// mapped to copy the app wrote. Messages the app composed itself (for
/// example "Sign in to Agent Relay") are shown as written.
enum UserFacingError {
    /// The server asked for a wait after something the person did (a send,
    /// a start, a sign-in): nothing retries it for them.
    static let busy = "Agent Relay is busy. Try again in a moment."
    /// The server asked for a wait during an automatic read, which the app
    /// retries on its own.
    static let busyRetrying = "Agent Relay is busy. Trying again shortly."
    static let signIn = "Your Agent Relay sign-in has expired. Sign in again to continue."
    static let serverTrouble = "Agent Relay is having trouble. Try again in a moment."
    static let unreachable = "Can't reach Agent Relay. Check your connection."
    static let generic = "Something went wrong talking to Agent Relay."
    /// The inline line an open conversation shows while its last automatic
    /// read failed and its messages may be out of date.
    static let staleMessages = "Couldn't refresh messages. Retrying…"
    /// The same for a channel, whose history the periodic refresh does not
    /// re-read: no retry is promised.
    static let staleChannelMessages = "Couldn't refresh messages."

    /// The copy for `error`, or nil for a cancellation, which is never shown.
    /// `retriesAutomatically` is true only for an automatic read the app
    /// tries again on its own; a failure the person caused is theirs to retry.
    static func message(for error: Error, retriesAutomatically: Bool = false) -> String? {
        if ConnectionErrorReports.isCancellation(error) { return nil }
        if let api = error as? RelayAPIError { return apiMessage(api, retriesAutomatically: retriesAutomatically) }
        if error is URLError { return unreachable }
        if let recovery = error as? WorkspaceKeyRecoveryError { return recovery.localizedDescription }
        // A file on this device could not be read or opened: the system's
        // own words name the file and the problem.
        if error is CocoaError { return error.localizedDescription }
        return generic
    }

    static func apiMessage(_ error: RelayAPIError, retriesAutomatically: Bool = false) -> String {
        if error.status == 429 || error.retryAfterSeconds != nil || error.code == "dm_unavailable" {
            return retriesAutomatically ? busyRetrying : busy
        }
        guard error.fromServer else { return shown(error.message) }
        if let summary = error.summary { return shown(summary) }
        switch error.status ?? 0 {
        case 401, 403: return signIn
        case 500...599: return serverTrouble
        default: return generic
        }
    }

    /// `text` without a scrubber's `[redacted]` marker. A redactor between a
    /// node and this app can replace an error code it mistakes for a key (as
    /// relay did with `spawn_fork_*` before relay#2218: `[redacted] @agent is
    /// not an agent on this computer`). The marker tells a person nothing, so
    /// it never reaches the screen; the rest of the sentence is kept. Text
    /// that was nothing but markers becomes `generic`.
    static func shown(_ text: String) -> String {
        guard text.range(of: "[redacted]", options: .caseInsensitive) != nil else { return text }
        let cleaned = text
            .replacingOccurrences(of: #"\[redacted\]:?[ \t]*"#, with: "", options: [.regularExpression, .caseInsensitive])
            .replacingOccurrences(of: #"[ \t]{2,}"#, with: " ", options: .regularExpression)
            .replacingOccurrences(of: #"(?m)^[ \t:]+|[ \t]+$"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"(?m)^Details:\s*$\n?"#, with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? generic : cleaned
    }

    /// The server asked the app to wait before trying again: a 429, any
    /// answer with Retry-After, or Relaycast shedding load (`dm_unavailable`
    /// for DM reads, `database_overloaded`).
    static func isBackoff(_ error: Error) -> Bool {
        guard let api = error as? RelayAPIError else { return false }
        return api.status == 429 || api.retryAfterSeconds != nil ||
            api.code == "dm_unavailable" || api.code == "database_overloaded"
    }
}

/// Holds automatic and repeated reads back while the server has asked the app
/// to wait. Relaycast sheds DM reads for a credential (HTTP 503
/// `dm_unavailable` with Retry-After) after failures, and a read before the
/// interval has passed lengthens the wait, so every DM read for that token
/// (any conversation, the conversation list included) pauses until then.
/// Other reads pause per path. A pause belongs to the token it was earned
/// with, and the next successful read of its kind clears it.
struct ReadBackoff {
    /// The wait when the server asked for one without saying how long.
    static let defaultDelay = 30
    static let delayRange = 1...300

    private var pauses: [String: (token: String, until: Date)] = [:]

    /// Which pause a request belongs to; nil for anything but a read.
    static func scope(path: String, method: String) -> String? {
        guard method == "GET" else { return nil }
        if path.hasPrefix("/dm/") { return "dm" }
        return String(path.prefix { $0 != "?" })
    }

    /// How long reads in `scope` with `token` must still wait, in whole
    /// seconds; nil when they may go ahead.
    func remaining(_ scope: String, token: String, now: Date = Date()) -> Int? {
        guard let pause = pauses[scope], pause.token == token else { return nil }
        let left = pause.until.timeIntervalSince(now)
        return left > 0 ? Int(left.rounded(.up)) : nil
    }

    mutating func failed(_ scope: String, token: String, error: Error, now: Date = Date()) {
        guard let delay = Self.delay(after: error) else { return }
        pauses[scope] = (token: token, until: now.addingTimeInterval(TimeInterval(delay)))
    }

    mutating func succeeded(_ scope: String) {
        pauses[scope] = nil
    }

    /// The wait a failure asks for: its Retry-After, bounded, or the default
    /// when it is a backoff without one; nil when it asks for none.
    static func delay(after error: Error) -> Int? {
        guard UserFacingError.isBackoff(error), let api = error as? RelayAPIError else { return nil }
        let asked = api.retryAfterSeconds ?? defaultDelay
        return min(max(asked, delayRange.lowerBound), delayRange.upperBound)
    }
}
