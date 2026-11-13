import Foundation

public enum StartupPresentation {
    public static let title = "Rounding up your agents"
    public static let messages = [
        "Herding cats, but the cats can code.",
        "Convincing everyone to use the same group chat.",
        "Untangling a few very ambitious to-do lists.",
        "Making room for one more agent. There’s always one more."
    ]
    public static let interval: TimeInterval = 4
    public static let timeoutMessage = "Your agents are taking longer to gather than expected. Try reconnecting the background service."
}

/// Where a person whose account has no workspace creates one.
public enum WorkspaceSetup {
    public static func dashboardURL(siteURL: String) -> URL? {
        guard var components = URLComponents(string: siteURL),
              components.scheme == "https" || components.scheme == "http",
              components.host != nil else { return nil }
        components.user = nil
        components.password = nil
        components.query = nil
        components.path = "/cloud/dashboard/settings"
        components.fragment = "workspace"
        return components.url
    }
}

/// Amp connects in Agent Relay Cloud, never on this Mac: the desktop opens
/// the Connections tab of the signed-in site's Cloud settings. The route is
/// confirmed; it matches the core's `AMP_CONNECTIONS_PATH`.
public enum AmpConnections {
    public static let path = "/dashboard/settings"
    public static let fragment = "connections"
    public static let action = "Open Cloud connections"
    public static let connected = "Connected in Agent Relay Cloud"
    public static let connects = "Connects through Agent Relay Cloud"

    /// Amp's connection is claimed only when Agent Relay Cloud reports it
    /// configured; otherwise the status says how Amp connects.
    public static func status(state: String) -> String { state == "configured" ? connected : connects }

    /// The core's `amp_connections_url`: the site is an origin, optionally
    /// followed by `/cloud`; https, or http only for a loopback host; no
    /// credentials, query or fragment; a non-default port only when local.
    public static func url(siteURL: String) -> URL? {
        var site = siteURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while site.hasSuffix("/") { site.removeLast() }
        if site.hasSuffix("/cloud") { site.removeLast("/cloud".count) }
        guard var components = URLComponents(string: site),
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased(), !host.isEmpty,
              components.user == nil, components.password == nil,
              components.query == nil, components.fragment == nil,
              components.path.isEmpty || components.path == "/" else { return nil }
        let local = isLoopback(host)
        guard scheme == "https" || (scheme == "http" && local) else { return nil }
        let port = components.port == (scheme == "https" ? 443 : 80) ? nil : components.port
        if port != nil && !local { return nil }
        components.scheme = scheme
        if !host.contains(":") { components.host = host }
        components.port = port
        components.path = "/cloud" + path
        components.fragment = fragment
        return components.url
    }

    private static func isLoopback(_ host: String) -> Bool {
        let bare = host.hasPrefix("[") && host.hasSuffix("]") ? String(host.dropFirst().dropLast()) : host
        if bare == "localhost" || bare == "::1" { return true }
        let octets = bare.split(separator: ".", omittingEmptySubsequences: false)
        return octets.count == 4 && octets[0] == "127" && octets.allSatisfy { UInt8($0) != nil }
    }
}
