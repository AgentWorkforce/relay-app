import Foundation
import Darwin

@MainActor protocol SessionPendingResumeStorage {
    var locationDescription: String { get }
    func load() throws -> Set<String>
    func reserve(_ key: String) throws -> (pending: Set<String>, inserted: Bool)
    func remove(_ key: String) throws -> Set<String>
}

/// Launch reservations must reach disk before a terminal receives a command.
/// Locking the read/modify/write also protects overlapping app instances.
@MainActor final class SessionPendingResumeStore: SessionPendingResumeStorage {
    private struct Record: Codable {
        let version: Int
        let pending: [String]
    }

    let file: URL
    var locationDescription: String { file.path }

    init(file: URL = AppInfo.dataDirectory.appendingPathComponent("session-pending-resumes.json")) {
        self.file = file
    }

    func load() throws -> Set<String> { try locked { try read() } }

    func reserve(_ key: String) throws -> (pending: Set<String>, inserted: Bool) {
        try locked {
            var pending = try read()
            let inserted = pending.insert(key).inserted
            if inserted { try write(pending) }
            return (pending, inserted)
        }
    }

    func remove(_ key: String) throws -> Set<String> {
        try locked {
            var pending = try read()
            if pending.remove(key) != nil { try write(pending) }
            return pending
        }
    }

    private func read() throws -> Set<String> {
        let data: Data
        do { data = try Data(contentsOf: file) }
        catch let error as NSError where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return []
        }
        let record = try JSONDecoder().decode(Record.self, from: data)
        guard record.version == 1, record.pending.allSatisfy({ !$0.isEmpty }),
              Set(record.pending).count == record.pending.count else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return Set(record.pending)
    }

    private func locked<T>(_ action: () throws -> T) throws -> T {
        let directory = file.deletingLastPathComponent()
        try makeDirectoryDurable(directory)
        let lock = open(file.appendingPathExtension("lock").path, O_CREAT | O_RDWR | O_NOFOLLOW, 0o600)
        guard lock >= 0 else { throw systemError() }
        defer { close(lock) }
        guard flock(lock, LOCK_EX) == 0 else { throw systemError() }
        defer { flock(lock, LOCK_UN) }
        return try action()
    }

    private func makeDirectoryDurable(_ directory: URL) throws {
        var isDirectory: ObjCBool = false
        if FileManager.default.fileExists(atPath: directory.path, isDirectory: &isDirectory) {
            guard isDirectory.boolValue else { throw CocoaError(.fileWriteFileExists) }
            return
        }
        let parent = directory.deletingLastPathComponent()
        try makeDirectoryDurable(parent)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        try syncDirectory(parent)
    }

    private func write(_ pending: Set<String>) throws {
        let data = try JSONEncoder().encode(Record(version: 1, pending: pending.sorted()))
        let temporary = file.deletingLastPathComponent().appendingPathComponent(".pending-resumes-\(UUID().uuidString)")
        let descriptor = open(temporary.path, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW, 0o600)
        guard descriptor >= 0 else { throw systemError() }
        defer {
            close(descriptor)
            try? FileManager.default.removeItem(at: temporary)
        }
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.write(descriptor, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                if count < 0 && errno == EINTR { continue }
                guard count > 0 else { throw systemError() }
                offset += count
            }
        }
        guard fsync(descriptor) == 0 else { throw systemError() }
        guard rename(temporary.path, file.path) == 0 else { throw systemError() }
        try syncDirectory(file.deletingLastPathComponent())
    }

    private func syncDirectory(_ directory: URL) throws {
        let descriptor = open(directory.path, O_RDONLY | O_DIRECTORY)
        guard descriptor >= 0 else { throw systemError() }
        defer { close(descriptor) }
        guard fsync(descriptor) == 0 else { throw systemError() }
    }

    private func systemError() -> NSError { NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
}
