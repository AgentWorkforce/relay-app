import Foundation

/// Decides whether a failed automatic Relaycast read (the presence refresh or
/// the open conversation's history) is shown in the "Agent Relay" alert.
///
/// iOS suspends the app a few seconds after it leaves the screen, and after a
/// background push wake as soon as the push is handled, with any request still
/// in flight. When the app comes back, URLSession ends such a request with "The
/// request timed out." (or "The network connection was lost."), while the
/// refresh the app starts on returning succeeds. That failure says nothing
/// about the connection now, so it is not shown. Each stretch in the
/// foreground or in the background is its own epoch; only a request that
/// started in the current foreground epoch, and fails while the app is still
/// in the foreground, can raise the alert. The foreground refresh reports a
/// connection that is really down or slow itself.
///
/// Within an epoch, a transport failure is also dropped when a newer request
/// of the same kind has already succeeded. A cancelled request is never shown.
struct ConnectionErrorReports {
    enum Operation: Hashable {
        case presence
        case history
    }

    struct Ticket: Equatable {
        let operation: Operation
        fileprivate let epoch: Int
        fileprivate let sequence: Int
    }

    enum Decision: Equatable {
        case show(String)
        case drop(Reason)
    }

    enum Reason: String, Equatable {
        case cancelled = "cancelled"
        case inBackground = "the app is in the background"
        case fromEarlierEpoch = "started before the app last left or returned to the screen"
        case superseded = "a newer request succeeded"
    }

    /// A cold launch starts on screen; a launch in the background is corrected
    /// by the first scene phase the view reports.
    private(set) var foreground = true
    private var epoch = 0
    private var sequence = 0
    private var newestSuccess: [Operation: Int] = [:]

    /// Inactive (Control Center, the app switcher) counts as foreground:
    /// requests keep running there. Only a move to or from background starts
    /// a new epoch.
    mutating func sceneChanged(foreground now: Bool) {
        guard now != foreground else { return }
        foreground = now
        epoch += 1
    }

    mutating func begin(_ operation: Operation) -> Ticket {
        sequence += 1
        return Ticket(operation: operation, epoch: epoch, sequence: sequence)
    }

    mutating func succeeded(_ ticket: Ticket) {
        newestSuccess[ticket.operation] = max(newestSuccess[ticket.operation] ?? 0, ticket.sequence)
    }

    func decision(for error: Error, from ticket: Ticket) -> Decision {
        if Self.isCancellation(error) { return .drop(.cancelled) }
        guard foreground else { return .drop(.inBackground) }
        guard ticket.epoch == epoch else { return .drop(.fromEarlierEpoch) }
        if Self.isTransport(error), (newestSuccess[ticket.operation] ?? 0) > ticket.sequence {
            return .drop(.superseded)
        }
        return .show(error.localizedDescription)
    }

    static func isCancellation(_ error: Error) -> Bool {
        error is CancellationError || (error as? URLError)?.code == .cancelled
    }

    static func isTransport(_ error: Error) -> Bool {
        guard let code = (error as? URLError)?.code else { return false }
        let transport: [URLError.Code] = [.timedOut, .networkConnectionLost, .notConnectedToInternet,
                                          .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed,
                                          .dataNotAllowed, .internationalRoamingOff]
        return transport.contains(code)
    }

    /// A log line naming the operation, the reason and the error's code only:
    /// never a URL, a token, a server message or a message's text.
    static func diagnostic(_ operation: Operation, _ reason: Reason, _ error: Error) -> String {
        let kind: String
        if let url = error as? URLError {
            kind = "URLError \(url.code.rawValue)"
        } else if let api = error as? RelayAPIError {
            kind = api.status.map { "HTTP \($0)" } ?? "API error"
        } else if error is CancellationError {
            kind = "CancellationError"
        } else {
            kind = "other error"
        }
        return "Agent Relay: \(operation) failure not shown (\(reason.rawValue); \(kind))"
    }
}
