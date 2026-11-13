import Foundation

/// The Relaycast workspace key Cloud resolves for a workspace
/// (`GET /workspaces/{id}/resolve`), checked the same way at sign-in and when
/// a rotated key is recovered: a live key, and a Relaycast server that is a
/// bare `https` origin.
struct WorkspaceKeyResolution: Equatable {
    let key: String
    /// The Relaycast origin with no trailing slash; the default when Cloud names none.
    let baseURL: String
    /// The workspace's display name; empty when Cloud names none.
    let workspaceName: String

    init(resolution: [String: Any], defaultBaseURL: String) throws {
        let nested = resolution["data"] as? [String: Any] ?? [:]
        let data = nested.isEmpty ? resolution : nested
        let key = data["relaycastApiKey"] as? String ?? ""
        guard Self.isLiveKey(key) else {
            throw RelayAPIError(message: "This workspace has no Relaycast key")
        }
        let base = (data["urls"] as? [String: Any])?["relaycastUrl"] as? String ?? ""
        guard base.isEmpty || Self.isRelaycastOrigin(base) else {
            throw RelayAPIError(message: "Invalid Relaycast server")
        }
        self.key = key
        baseURL = (base.isEmpty ? defaultBaseURL : base)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        workspaceName = data["name"] as? String ?? ""
    }

    /// `rk_live_` followed by a base64url or hex secret.
    static func isLiveKey(_ key: String) -> Bool {
        guard key.hasPrefix("rk_live_") else { return false }
        let secret = key.dropFirst("rk_live_".count)
        return (16...128).contains(secret.count) && secret.unicodeScalars.allSatisfy {
            ($0.isASCII && CharacterSet.alphanumerics.contains($0)) || $0 == "_" || $0 == "-"
        }
    }

    /// An `https` origin with no credentials, path, query, fragment or non-default port.
    static func isRelaycastOrigin(_ base: String) -> Bool {
        let url = URLComponents(string: base)
        return url?.scheme == "https" && url?.host != nil &&
            url?.user == nil && url?.password == nil &&
            url?.query == nil && url?.fragment == nil &&
            (url?.path.isEmpty == true || url?.path == "/") &&
            (url?.port == nil || url?.port == 443)
    }
}

/// Why a refused workspace key was not replaced.
enum WorkspaceKeyRecoveryError: LocalizedError, Equatable {
    /// Cloud no longer accepts this device's sign-in.
    case signInAgain
    /// Cloud has no replacement: it returned the refused key, or this key was
    /// already recovered once and is still refused.
    case keyRefused
    /// Cloud named a different Relaycast server than the one this device uses.
    case differentServer
    /// The replacement could not be saved on this device.
    case notSaved
    /// The workspace changed (sign-out, another sign-in, another recovery)
    /// while Cloud answered; the caller keeps its original error.
    case superseded

    var errorDescription: String? {
        switch self {
        case .signInAgain:
            return "Your Agent Relay sign-in has expired. Sign in again to keep using this workspace."
        case .keyRefused:
            return "Agent Relay no longer accepts this workspace's key and Cloud has no replacement yet. Sign in again to get a new one."
        case .differentServer:
            return "Cloud returned a different Relaycast server for this workspace"
        case .notSaved:
            return "Could not save the workspace key on this device"
        case .superseded:
            return "The workspace changed while its key was being renewed"
        }
    }
}

/// Recovers a workspace key Relaycast refused (a rotation whose grace has
/// lapsed) by asking Cloud for the current one. One recovery runs at a time
/// for a refused key, and a key Cloud answered for is never recovered again,
/// so a key that stays refused cannot start a loop.
@MainActor final class WorkspaceKeyRecovery {
    /// Thrown by `resolve` when Cloud refuses this device's session.
    struct CloudSessionRejected: Error {}

    private var answered: Set<String> = []
    private var inFlight: (id: UUID, refused: String, task: Task<String, Error>)?

    /// The replacement for `refused`, saved before it is returned.
    /// - Parameters:
    ///   - refused: the key Relaycast answered 401 for.
    ///   - currentBaseURL: the Relaycast origin this device uses; a
    ///     replacement for any other origin is refused.
    ///   - defaultBaseURL: the origin a resolution without one implies.
    ///   - resolve: Cloud's resolution for the workspace; throws
    ///     `CloudSessionRejected` when Cloud refuses the session.
    ///   - save: stores the replacement, answering whether it is now saved;
    ///     throws `WorkspaceKeyRecoveryError.superseded` when `refused` is no
    ///     longer the current key.
    /// - Returns: the replacement key.
    func replacement(
        for refused: String,
        currentBaseURL: String,
        defaultBaseURL: String,
        resolve: @escaping @MainActor () async throws -> [String: Any],
        save: @escaping @MainActor (_ refused: String, _ replacement: String) throws -> Bool
    ) async throws -> String {
        if let inFlight, inFlight.refused == refused {
            return try await inFlight.task.value
        }
        guard !answered.contains(refused) else { throw WorkspaceKeyRecoveryError.keyRefused }
        let task = Task { @MainActor [weak self] () throws -> String in
            let resolution: [String: Any]
            do {
                resolution = try await resolve()
            } catch is CloudSessionRejected {
                self?.answered.insert(refused)
                throw WorkspaceKeyRecoveryError.signInAgain
            }
            // Cloud answered: whatever it said, this key is not asked about
            // again. A failure to reach Cloud leaves it for a later call.
            self?.answered.insert(refused)
            let resolved = try WorkspaceKeyResolution(resolution: resolution, defaultBaseURL: defaultBaseURL)
            guard resolved.key != refused else { throw WorkspaceKeyRecoveryError.keyRefused }
            guard resolved.baseURL == currentBaseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/")) else {
                throw WorkspaceKeyRecoveryError.differentServer
            }
            guard try save(refused, resolved.key) else { throw WorkspaceKeyRecoveryError.notSaved }
            return resolved.key
        }
        let id = UUID()
        inFlight = (id, refused, task)
        defer {
            if inFlight?.id == id { inFlight = nil }
        }
        return try await task.value
    }

    /// Forgets every refused key, for a new sign-in or sign-out.
    func reset() {
        answered = []
        inFlight = nil
    }
}
