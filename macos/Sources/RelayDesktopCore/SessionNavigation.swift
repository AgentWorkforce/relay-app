import Foundation

public enum SessionNavigation {
    /// The Cloud detail URL for an uploaded session. Session identifiers are
    /// encoded as one path component because providers may use `/` in an id.
    /// The session's history `source` and the Cloud `workspaceID` it was
    /// uploaded to, when known, go in the query: without them Cloud reads the
    /// session in the browser's current workspace and has to guess its source.
    /// desktop-core's `cloud_session_url` matches it.
    public static func cloudURL(siteURL: String, sessionId: String,
                                source: String? = nil, workspaceID: String? = nil) -> URL? {
        guard !sessionId.isEmpty, var components = URLComponents(string: siteURL),
              components.scheme == "https" || components.scheme == "http",
              components.host != nil else { return nil }
        guard let pathSegment = component(sessionId) else { return nil }
        components.user = nil
        components.password = nil
        components.fragment = nil
        components.percentEncodedPath = "/cloud/dashboard/sessions/\(pathSegment)"
        var query: [String] = []
        for (name, value) in [("source", source), ("workspace", workspaceID)] {
            guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty,
                  let encoded = component(value) else { continue }
            query.append("\(name)=\(encoded)")
        }
        components.percentEncodedQuery = query.isEmpty ? nil : query.joined(separator: "&")
        return components.url
    }

    /// The Cloud detail URL of a session that has finished uploading to
    /// `workspaceID`; Cloud has no page for one that hasn't.
    public static func cloudURL(siteURL: String, session: UploadSession, workspaceID: String?) -> URL? {
        guard session.status == .uploaded else { return nil }
        return cloudURL(siteURL: siteURL, sessionId: session.sessionId, source: session.source, workspaceID: workspaceID)
    }

    /// `value` percent-encoded but for RFC 3986's unreserved characters.
    private static func component(_ value: String) -> String? {
        value.addingPercentEncoding(
            withAllowedCharacters: CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        )
    }
}
