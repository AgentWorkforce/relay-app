import Foundation

/// The Session Uploads search field.
public enum SessionFilter {
    /// Title, id, source and project, ignoring case; empty matches all.
    public static func matches(_ session: UploadSession, query: String) -> Bool {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if needle.isEmpty { return true }
        return [session.displayTitle, session.title, session.id,
                session.displayProject, session.source]
            .contains { $0.lowercased().contains(needle) }
    }

    public static func filter(_ sessions: [UploadSession], query: String) -> [UploadSession] {
        sessions.filter { matches($0, query: query) }
    }
}
