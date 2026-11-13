import Foundation

/// A count in the menu-bar summary. Each is a session search, so choosing
/// it opens the session list showing exactly the sessions it counted.
public enum MenuSummaryFilter: CaseIterable, Sendable {
    case receiving, localOnly, uploaded, notUploaded

    public var title: String {
        switch self {
        case .receiving: return "Receiving messages"
        case .localOnly: return "Local only"
        case .uploaded: return "Uploaded"
        case .notUploaded: return "Not uploaded"
        }
    }

    public var search: String {
        switch self {
        case .receiving: return "is:active is:on-relay"
        case .localOnly: return "is:active is:off-relay"
        case .uploaded: return "upload:uploaded"
        case .notUploaded: return "-upload:uploaded"
        }
    }

    /// Receiving and Uploaded read as live; the others stay muted.
    public var isLive: Bool { self == .receiving || self == .uploaded }

    /// How many `rows` the session list shows for this filter.
    public func count(_ rows: [LocalSessionRow], nowMs: Int64 = currentTimeMs(),
                      uploadState: (LocalSessionRow) -> SessionUploadState?) -> Int {
        var query = Self.queries[self]!
        query.nowMs = nowMs
        return rows.filter { query.matches($0, uploadState: uploadState($0)) }.count
    }

    private static let queries = Dictionary(uniqueKeysWithValues: allCases.map { ($0, LocalSessionQuery($0.search)) })

    /// Every filter's count in one pass at `nowMs`: each row's upload state
    /// is read once and matched against all four searches. The parsed
    /// searches are kept; the time they judge activity at is not.
    public static func counts(_ rows: [LocalSessionRow], nowMs: Int64 = currentTimeMs(),
                              uploadState: (LocalSessionRow) -> SessionUploadState?) -> [Self: Int] {
        var counts = Dictionary(uniqueKeysWithValues: allCases.map { ($0, 0) })
        let queries = Self.queries.mapValues { query in var query = query; query.nowMs = nowMs; return query }
        for row in rows {
            let state = uploadState(row)
            for (filter, query) in queries where query.matches(row, uploadState: state) {
                counts[filter, default: 0] += 1
            }
        }
        return counts
    }
}
