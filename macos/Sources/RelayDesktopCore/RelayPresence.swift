import Foundation

/// The session’s registration and relay connection, as its row shows it.
public enum RelayPresence: Equatable, Sendable {
    /// Not on the relay: no name, nothing delivered.
    case off
    /// On the relay with a connected Relaycast push channel. This does not
    /// verify the local provider inbox.
    case online
    /// On the relay while the push channel (re)connects; messages still
    /// arrive through the queue check.
    case connecting
    /// On the relay but the session is not running; messages wait.
    case offline
    /// Asked to go on the relay; registering.
    case joining
    /// Asked to leave the relay; releasing its name.
    case leaving

    /// `wanted` is the state last asked for while it differs from the
    /// relay's. `pushLive` is nil before the push channel reports a state.
    public static func of(onRelay: Bool, wanted: Bool? = nil, isLive: Bool, pushLive: Bool?) -> RelayPresence {
        if let wanted, wanted != onRelay { return wanted ? .joining : .leaving }
        guard onRelay else { return .off }
        guard isLive else { return .offline }
        return pushLive == true ? .online : .connecting
    }

    public var messageStatus: String {
        switch self {
        case .online: return "Relay connected"
        case .joining, .connecting: return "Connecting…"
        case .off: return "Off"
        case .offline: return "Offline"
        case .leaving: return "Turning off…"
        }
    }

    public var messageSymbol: String {
        switch self {
        case .online: return "checkmark.circle.fill"
        case .joining, .connecting, .leaving: return "arrow.triangle.2.circlepath"
        case .offline: return "pause.circle"
        case .off: return "stop.circle"
        }
    }

    public var messageHelp: String {
        switch self {
        case .online: return "Connected to the relay. Check each message’s delivery status. Click to go offline."
        case .joining, .connecting: return "Connecting to the relay. Click to go offline."
        case .offline: return "Offline — this session is not running. Messages wait until it starts. Click to turn off."
        case .off: return "Off. Click to receive messages."
        case .leaving: return "Turning off message delivery. Click to reconnect."
        }
    }

    public var label: String {
        switch self {
        case .off: return "Not on the relay"
        case .online: return "Relay connected"
        case .connecting: return "Connecting"
        case .offline: return "Offline"
        case .joining: return "Joining the relay…"
        case .leaving: return "Leaving the relay…"
        }
    }

    /// On, as the switch shows it: the state asked for wins.
    public var isOn: Bool {
        switch self {
        case .off, .leaving: return false
        case .online, .connecting, .offline, .joining: return true
        }
    }

    public var isChanging: Bool { self == .joining || self == .leaving }

    /// The badge on the session header's second line; nil while the session
    /// is not on the relay.
    public var headerBadge: String? {
        switch self {
        case .online: return "Online"
        case .joining, .connecting: return "Connecting…"
        case .offline: return "Offline"
        case .off, .leaving: return nil
        }
    }

    public var detail: String {
        switch self {
        case .off: return "Turn on to make this session reachable on the relay."
        case .online: return "Connected to the relay. Check each message’s delivery status."
        case .connecting: return "Connecting to the relay. Messages wait for delivery."
        case .offline: return "The session isn't running. Messages wait until it is."
        case .joining: return "Registering a name on the relay."
        case .leaving: return "Releasing its name on the relay."
        }
    }
}

public enum SessionHeader {
    /// The session header's second line: the provider, then `@name` on the
    /// relay when the session has one. The presence badge follows it.
    public static func identity(provider: String, agentName: String?) -> String {
        [provider, agentName.flatMap { $0.isEmpty ? nil : "@\($0)" } ?? ""]
            .filter { !$0.isEmpty }.joined(separator: " · ")
    }
}
