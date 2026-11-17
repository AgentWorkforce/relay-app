import Foundation

/// A session's hosted summary: `GET /sessions/:id/summary` on the Cloud API.
/// The whole contract is kept here, so it changes in one place.
///
/// - Query: `workspace` (the signed-in workspace), `source` (the session's
///   canonical stored source as the summary list returns it, such as
///   `claude`, `codex` or `gemini`; never the agent's name or CLI) and `kind`
///   (`short` or `long`). Cloud answers any other parameter with 501
///   `param_<name>`. A source of "—" (U+2014) means Cloud has none: the
///   summary is not fetched and the section is hidden.
/// - 200: `{"summary": {...}, "provenance": {...} | null}`. The inner
///   `summary` object never gains keys, so it is parsed strictly;
///   `provenance` is a sibling that is present only while Cloud's fallback
///   flag is on.
/// - 501 `summary_kind` or `summary_fallback_not_configured` for `kind=long`:
///   no long summary, ask for `kind=short`. 501 `summary_unqualified`: the
///   session can't have a summary, so the section is hidden. 503
///   `backend_unavailable`: transient, retried with backoff.
struct SessionSummary: Equatable {
    enum Kind: String, Equatable { case short, long }

    struct Provenance: Equatable {
        /// `session_brief` (the native short brief) or `session_summary`
        /// (the summariser's output for the session's current revision).
        let source: String
        /// The summariser's text was cut at its length limit.
        let truncated: Bool
    }

    struct Malformed: Error {}

    let kind: Kind
    /// Nil when Cloud has no summary of this kind yet (`status: empty`).
    let text: String?
    let model: String?
    let builtAt: Date?
    let provenance: Provenance?

    private static let readyKeys: Set<String> = ["status", "kind", "summary", "model", "builtAt", "state"]
    private static let emptyKeys: Set<String> = ["status", "kind", "state"]

    init(_ response: JSONObject) throws {
        guard let summary = response["summary"] as? JSONObject,
              let kind = (summary["kind"] as? String).flatMap(Kind.init(rawValue:)) else { throw Malformed() }
        let keys = Set(summary.keys)
        switch summary["status"] as? String {
        case "ready":
            let text = (summary["summary"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard keys.isSubset(of: Self.readyKeys), !text.isEmpty else { throw Malformed() }
            self.text = text
            model = summary["model"] as? String
            builtAt = Self.date(summary["builtAt"])
        case "empty":
            guard keys.isSubset(of: Self.emptyKeys) else { throw Malformed() }
            text = nil
            model = nil
            builtAt = nil
        default:
            throw Malformed()
        }
        self.kind = kind
        let origin = response["provenance"] as? JSONObject
        provenance = (origin?["source"] as? String).map {
            Provenance(source: $0, truncated: origin?["truncated"] as? Bool ?? false)
        }
    }

    private static func date(_ value: Any?) -> Date? {
        guard let timestamp = value as? String else { return nil }
        return (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(timestamp))
            ?? (try? Date.ISO8601FormatStyle().parse(timestamp))
    }
}

enum SessionSummaryRequest {
    /// Relative to the Cloud API base; `{id}` is the percent-encoded session ID.
    /// The Cloud proxy route is pending: until it exists Cloud answers 404 and the section stays hidden.
    static let pathTemplate = "/sessions/{id}/summary"
    /// The source Cloud reports when it has none for the session.
    static let missingSourceSentinel = "\u{2014}"

    /// The source to send, or nil when the session has none.
    static func usableSource(_ source: String?) -> String? {
        guard let source = source?.trimmingCharacters(in: .whitespacesAndNewlines),
              !source.isEmpty, source != missingSourceSentinel else { return nil }
        return source
    }

    static func path(sessionID: String, workspaceID: String, source: String, kind: SessionSummary.Kind) -> String? {
        guard UUID(uuidString: workspaceID) != nil, let source = usableSource(source),
              !sessionID.isEmpty, sessionID.count <= 512 else { return nil }
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        guard let encoded = sessionID.addingPercentEncoding(withAllowedCharacters: allowed) else { return nil }
        var components = URLComponents()
        components.queryItems = [
            URLQueryItem(name: "workspace", value: workspaceID),
            URLQueryItem(name: "source", value: source),
            URLQueryItem(name: "kind", value: kind.rawValue),
        ]
        guard let query = components.percentEncodedQuery else { return nil }
        let path = pathTemplate.replacingOccurrences(of: "{id}", with: encoded)
        return "\(path)?\(query.replacingOccurrences(of: "+", with: "%2B"))"
    }
}

/// What the session detail shows for the hosted summary.
enum SessionSummaryState: Equatable {
    case ready(text: String, isShort: Bool)
    /// "No summary yet": the normal case for most sessions.
    case empty
    /// "Summary unavailable", with a retry.
    case unavailable
    /// The session can't have a summary; the section is not shown.
    case hidden

    init(_ summary: SessionSummary) {
        if let text = summary.text {
            self = .ready(text: text, isShort: summary.kind == .short)
        } else {
            self = .empty
        }
    }

    /// What a failed request for `requested` means. Nil: ask for the short
    /// summary instead.
    static func afterFailure(status: Int?, code: String, requested: SessionSummary.Kind) -> SessionSummaryState? {
        if status == 501, requested == .long,
           code == "summary_kind" || code == "summary_fallback_not_configured" { return nil }
        if status == 501 || status == 404 { return .hidden }
        return .unavailable
    }

    static func retriesAutomatically(status: Int?) -> Bool { status == 503 }
}

/// A summary belongs to a revision of the session's content. It is fetched
/// again when the list reports a different revision, or when the detail opens
/// and the copy is older than `maximumAge`. There is no polling.
struct SessionSummaryCacheEntry: Equatable {
    static let maximumAge: TimeInterval = 5 * 60

    let state: SessionSummaryState
    let revision: String?
    let fetchedAt: Date

    func isFresh(revision current: String?, now: Date = Date()) -> Bool {
        guard state != .unavailable else { return false }
        if let current, current != revision { return false }
        return now.timeIntervalSince(fetchedAt) < Self.maximumAge
    }
}
