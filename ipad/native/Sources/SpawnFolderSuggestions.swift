import Foundation

/// A folder the New agent sheet suggests for a machine's project.
struct SpawnFolderSuggestion: Identifiable, Hashable {
    /// The folder's last path component.
    let name: String
    let path: String
    var id: String { path }
}

/// Folders Relay already knows for a machine, for the New agent sheet.
///
/// The only list a node publishes is `recent_folders`, in the `configured`
/// metadata of each `spawn:<harness>` capability: newest-first absolute
/// working folders of agents started there. A node publishes it only when its
/// person lets teammates start agents on it. Agent registrations and the
/// `relay:live-agents:v1` list carry no folder.
enum SpawnFolderSuggestions {
    static let limit = 8

    /// The `recent_folders` a node's capabilities list, newest first. Every
    /// spawn capability carries the same list; they are merged in order.
    static func advertisedFolders(in capabilities: [JSONObject]) -> [String] {
        var folders: [String] = []
        for capability in capabilities {
            guard let name = capability["name"] as? String, name.hasPrefix("spawn:") else { continue }
            let metadata = capability["metadata"] as? JSONObject ?? [:]
            let configured = metadata["configured"] as? JSONObject ?? [:]
            for folder in configured["recent_folders"] as? [String] ?? [] where !folders.contains(folder) {
                folders.append(folder)
            }
        }
        return folders
    }

    /// Suggestions for `node`, newest first, without duplicates, folders
    /// already saved as projects there, or paths the node could not use.
    static func suggestions(for node: String, in nodes: [RelaySpawnNode],
                            saved savedPaths: [String] = []) -> [SpawnFolderSuggestion] {
        guard let folders = nodes.first(where: { $0.name == node })?.recentFolders else { return [] }
        var seen = Set(savedPaths.compactMap(normalized))
        var suggestions: [SpawnFolderSuggestion] = []
        for folder in folders {
            guard suggestions.count < limit else { break }
            guard let path = normalized(folder), seen.insert(path).inserted else { continue }
            suggestions.append(SpawnFolderSuggestion(name: projectName(for: path), path: path))
        }
        return suggestions
    }

    /// The folder's last path component.
    static func projectName(for path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? ""
    }

    /// An absolute path on one line without a trailing slash; nil for
    /// anything else, including `/` itself.
    private static func normalized(_ path: String) -> String? {
        let trimmed = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, SpawnProjectPath.problem(trimmed, node: "") == nil else { return nil }
        var path = trimmed
        while path.count > 1 && path.hasSuffix("/") { path.removeLast() }
        return path == "/" ? nil : path
    }
}
