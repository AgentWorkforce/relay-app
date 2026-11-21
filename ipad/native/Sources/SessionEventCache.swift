import CryptoKit
import Foundation

/// Session events kept on disk so an agent's chat shows its history at
/// launch before the network answers. One file per workspace and session in
/// the app's own Application Support directory, excluded from backup and
/// protected until the device's first unlock. Never in UserDefaults, an app
/// group or logs. Every file is deleted at sign-out and workspace switch, and
/// a session's file when the server says the session is gone.
final class SessionEventDiskCache: SessionEventSnapshotStore, @unchecked Sendable {
    /// Bumped when the file format changes; a file in another version is deleted.
    static let formatVersion = 1
    /// The newest events kept for one session.
    static let defaultEventsPerSession = 1_000
    /// The largest file kept for one session; older events go first.
    static let defaultBytesPerSession = 4 * 1024 * 1024
    /// All sessions together; the least recently used files go first.
    static let defaultTotalBytes = 50 * 1024 * 1024

    let directory: URL
    let eventsPerSession: Int
    let bytesPerSession: Int
    let totalBytes: Int
    /// Every file operation runs here, in order, off the main thread.
    private let queue = DispatchQueue(label: "com.agentworkforce.relayipad.session-event-cache", qos: .utility)

    private struct File: Codable {
        let version: Int
        let workspaceID: String
        let sessionID: String
        let snapshot: SessionEventSnapshot
    }

    private struct Version: Decodable { let version: Int }

    init(directory: URL, eventsPerSession: Int = defaultEventsPerSession,
         bytesPerSession: Int = defaultBytesPerSession, totalBytes: Int = defaultTotalBytes) {
        self.directory = directory
        self.eventsPerSession = eventsPerSession
        self.bytesPerSession = bytesPerSession
        self.totalBytes = totalBytes
    }

    /// The app's cache: `Application Support/SessionEventCache`.
    static func standard() -> SessionEventDiskCache? {
        guard let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            return nil
        }
        return SessionEventDiskCache(directory: support.appendingPathComponent("SessionEventCache", isDirectory: true))
    }

    /// The file for one session; the name is a hash, so no id shows in it.
    func fileURL(workspaceID: String, sessionID: String) -> URL {
        let digest = SHA256.hash(data: Data("\(workspaceID)\n\(sessionID)".utf8))
        let name = digest.map { String(format: "%02x", $0) }.joined()
        return directory.appendingPathComponent(name).appendingPathExtension("json")
    }

    func load(workspaceID: String, sessionID: String) async -> SessionEventSnapshot? {
        await withCheckedContinuation { continuation in
            queue.async { continuation.resume(returning: self.read(workspaceID: workspaceID, sessionID: sessionID)) }
        }
    }

    func save(_ snapshot: SessionEventSnapshot, workspaceID: String) {
        queue.async { self.write(snapshot, workspaceID: workspaceID) }
    }

    func remove(workspaceID: String, sessionID: String) {
        let url = fileURL(workspaceID: workspaceID, sessionID: sessionID)
        queue.sync { try? FileManager.default.removeItem(at: url) }
    }

    /// Deletes every file. Waits, so nothing is left when it returns; a save
    /// queued before it is deleted with the rest.
    func removeAll() {
        queue.sync { try? FileManager.default.removeItem(at: directory) }
    }

    /// Waits for queued work; tests use it.
    func flush() { queue.sync {} }

    // MARK: On the queue

    private func read(workspaceID: String, sessionID: String) -> SessionEventSnapshot? {
        let url = fileURL(workspaceID: workspaceID, sessionID: sessionID)
        guard let data = try? Data(contentsOf: url) else { return nil }
        guard (try? JSONDecoder().decode(Version.self, from: data))?.version == Self.formatVersion,
              let file = try? JSONDecoder().decode(File.self, from: data),
              file.workspaceID == workspaceID, file.sessionID == sessionID,
              file.snapshot.sessionID == sessionID else {
            // Another version, a corrupt file or someone else's: read it from the network.
            try? FileManager.default.removeItem(at: url)
            return nil
        }
        // Marks it recently used.
        try? FileManager.default.setAttributes([.modificationDate: Date()], ofItemAtPath: url.path)
        return file.snapshot
    }

    private func write(_ snapshot: SessionEventSnapshot, workspaceID: String) {
        var kept = snapshot.keepingNewest(eventsPerSession)
        var data: Data?
        while true {
            data = try? JSONEncoder().encode(File(version: Self.formatVersion, workspaceID: workspaceID,
                                                  sessionID: kept.sessionID, snapshot: kept))
            guard let size = data?.count, size > bytesPerSession, !kept.events.isEmpty else { break }
            kept = kept.keepingNewest(kept.events.count / 2)
        }
        guard let data, data.count <= bytesPerSession, prepareDirectory() else { return }
        var url = fileURL(workspaceID: workspaceID, sessionID: kept.sessionID)
        do {
            #if os(iOS)
            try data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            #else
            try data.write(to: url, options: [.atomic])
            #endif
            var values = URLResourceValues()
            values.isExcludedFromBackup = true
            try? url.setResourceValues(values)
        } catch {
            return
        }
        evict(keeping: url)
    }

    private func prepareDirectory() -> Bool {
        var url = directory
        do {
            #if os(iOS)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true,
                                                    attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
            #else
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            #endif
            var values = URLResourceValues()
            values.isExcludedFromBackup = true
            try url.setResourceValues(values)
            return true
        } catch {
            return false
        }
    }

    /// Deletes the least recently used files until all fit in `totalBytes`.
    private func evict(keeping current: URL) {
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey]
        guard let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: keys) else {
            return
        }
        var entries = files.compactMap { url -> (url: URL, size: Int, used: Date)? in
            guard let values = try? url.resourceValues(forKeys: Set(keys)) else { return nil }
            return (url, values.fileSize ?? 0, values.contentModificationDate ?? .distantPast)
        }
        var total = entries.reduce(0) { $0 + $1.size }
        guard total > totalBytes else { return }
        entries.sort { $0.used < $1.used }
        for entry in entries where total > totalBytes && entry.url.standardizedFileURL != current.standardizedFileURL {
            try? FileManager.default.removeItem(at: entry.url)
            total -= entry.size
        }
    }
}
