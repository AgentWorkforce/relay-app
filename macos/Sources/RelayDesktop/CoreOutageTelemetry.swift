import CoreFoundation
import Darwin
import Foundation
import RelayDesktopCore

/// Reports failures the normal telemetry RPC cannot reach. The app and core
/// share the installation ID. The core manages rotation.
@MainActor
final class CoreOutageTelemetry {
    enum Reason: String, Encodable, Hashable {
        case startupWatchdog = "startup_watchdog"
        case repeatedRefusal = "repeated_refusal"
        case registrationFailed = "registration_failed"
        case repairFailed = "repair_failed"
        case installationRequired = "installation_required"
        case serviceNotFound = "service_not_found"
        case knownSpawnFailure = "known_spawn_failure"
    }

    enum Architecture: String, Encodable {
        case arm64, x86_64, unknown

        static var current: Self {
            #if arch(arm64)
            .arm64
            #elseif arch(x86_64)
            .x86_64
            #else
            .unknown
            #endif
        }
    }

    struct Failure: Encodable {
        enum Domain: String, Encodable {
            case posix, cocoa, bridge, other
            case osStatus = "os_status"
            case serviceManagement = "service_management"
            case launchAgent = "launch_agent"
        }
        let domain: Domain
        let code: Int?

        init(_ error: Error) {
            if let error = error as? CoreError {
                if let socketCode = error.socketErrorCode {
                    domain = .posix
                    code = (1...255).contains(socketCode) ? Int(socketCode) : nil
                } else {
                    domain = .bridge
                    code = nil
                }
                return
            }
            let error = error as NSError
            switch error.domain {
            case NSPOSIXErrorDomain: domain = .posix
            case NSCocoaErrorDomain: domain = .cocoa
            case NSOSStatusErrorDomain: domain = .osStatus
            case "SMAppServiceErrorDomain", "SMErrorDomain", "CFErrorDomainLaunchd": domain = .serviceManagement
            case LaunchAgentService.errorDomain: domain = .launchAgent
            default: domain = .other
            }
            code = domain != .other && (-65535...65535).contains(error.code) ? error.code : nil
        }
    }

    private struct Properties: Encodable {
        let schemaVersion = 1
        let app = "relay-desktop"
        let platform = "macos"
        let emitter = "relay-desktop.macos"
        let geoIPDisabled = true
        let processPersonProfile = false
        let reason: Reason?
        let appVersion: [Int]?
        let bundledCoreVersion: [Int]?
        let runningCoreVersion: [Int]?
        let osVersion: [Int]?
        let architecture: Architecture
        let service: CoreServiceDiagnostics
        let attempts: Int
        let repairs: Int
        let lastError: Failure?

        enum CodingKeys: String, CodingKey {
            case schemaVersion, app, platform, emitter, reason, appVersion, bundledCoreVersion,
                 runningCoreVersion, osVersion, architecture, service, attempts, repairs, lastError
            case geoIPDisabled = "$geoip_disable"
            case processPersonProfile = "$process_person_profile"
        }
    }

    private enum Event: String, Encodable {
        case unavailable = "desktop_core_unavailable"
        case recovered = "desktop_core_recovered"
    }

    private struct Capture: Encodable {
        let apiKey: String
        let event: Event
        let distinctId: String
        let properties: Properties
    }

    struct Project {
        fileprivate let key: String
        fileprivate static let endpoint = URL(string: "https://i.agentrelay.com/i/v0/e/")!

        init?(key: String?, site: String?, bundleIdentifier: String?, isRelease: Bool) {
            guard isRelease, site == AppInfo.productionSiteURL,
                  bundleIdentifier == AppInfo.productionBundleIdentifier,
                  let key, key.hasPrefix("phc_"), (5...255).contains(key.utf8.count),
                  key.utf8.allSatisfy({
                      (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 95
                  }) else { return nil }
            self.key = key
        }
    }

    private let project: Project
    private let dataDirectory: URL
    private let defaults: UserDefaults
    private let transport: @MainActor (URLRequest) -> Bool
    private let appVersion: [Int]?
    private let bundledCoreVersion: [Int]?
    private let osVersion: [Int]?
    private let architecture: Architecture
    private var outageIdentity: String?
    private var reported: Set<Reason> = []
    private var endedConsentOrIdentity = false
    private var recoveryRetry: Task<Void, Never>?

    init(project: Project, dataDirectory: URL, defaults: UserDefaults,
         appVersion: String, bundledCoreVersion: String?, osVersion: OperatingSystemVersion,
         architecture: Architecture? = nil, transport: @escaping @MainActor (URLRequest) -> Bool) {
        self.project = project
        self.dataDirectory = dataDirectory
        self.defaults = defaults
        self.transport = transport
        self.appVersion = Self.version(appVersion)
        self.bundledCoreVersion = Self.version(bundledCoreVersion)
        let os = [osVersion.majorVersion, osVersion.minorVersion, osVersion.patchVersion]
        self.osVersion = os.allSatisfy { (0...9999).contains($0) } ? os : nil
        self.architecture = architecture ?? .current
    }

    static func bundled(defaults: UserDefaults) -> CoreOutageTelemetry? {
        #if DEBUG
        return nil
        #else
        guard let project = Project(
            key: Bundle.main.object(forInfoDictionaryKey: "AgentRelayOutageProjectKey") as? String,
            site: AppInfo.resource("site-url")?.trimmingCharacters(in: .whitespacesAndNewlines),
            bundleIdentifier: Bundle.main.bundleIdentifier,
            isRelease: true
        ) else { return nil }
        let sender = HTTPS()
        return CoreOutageTelemetry(project: project, dataDirectory: AppInfo.dataDirectory, defaults: defaults,
                                   appVersion: AppInfo.version, bundledCoreVersion: AppInfo.bundledCoreVersion,
                                   osVersion: ProcessInfo.processInfo.operatingSystemVersion, transport: sender.send)
        #endif
    }

    /// One report for each observed reason until the next healthy subscription.
    /// A consent or ID change ends reporting for the current outage.
    func failure(_ reason: Reason, service: @autoclosure () -> CoreServiceDiagnostics,
                 attempts: Int, repairs: Int, lastError: Failure?, runningVersion: String?, allowed: Bool) {
        if recoveryRetry != nil { finishRecovery() }
        guard !reported.contains(reason),
              let id = try? identityForOutage(allowed: allowed) else { return }
        guard (try? send(.unavailable, id: id, reason: reason, service: service(), attempts: attempts,
                         repairs: repairs, lastError: lastError, runningVersion: runningVersion)) == true else { return }
        outageIdentity = id
        reported.insert(reason)
    }

    func recovered(service: @autoclosure () -> CoreServiceDiagnostics, attempts: Int, repairs: Int,
                   lastError: Failure?, runningVersion: String, allowed: Bool) {
        guard recoveryRetry == nil else { return }
        guard outageIdentity != nil, !endedConsentOrIdentity, allowed else {
            finishRecovery()
            return
        }
        let diagnostics = service()
        let attempt: @MainActor (CoreOutageTelemetry) throws -> Void = { telemetry in
            guard let id = try telemetry.identityForOutage(allowed: allowed) else { return }
            _ = try telemetry.send(.recovered, id: id, reason: nil, service: diagnostics, attempts: attempts,
                                   repairs: repairs, lastError: lastError, runningVersion: runningVersion)
        }
        do {
            try attempt(self)
            finishRecovery()
        } catch {
            // A healthy subscription may only notify us once. Give the core
            // time to release the lock without making the main actor wait.
            recoveryRetry = Task { [weak self] in
                for _ in 0..<40 {
                    try? await Task.sleep(for: .milliseconds(50))
                    guard !Task.isCancelled, let self else { return }
                    do {
                        try attempt(self)
                        self.finishRecovery()
                        return
                    } catch {
                        continue
                    }
                }
                self?.finishRecovery()
            }
        }
    }

    private func finishRecovery() {
        recoveryRetry?.cancel()
        recoveryRetry = nil
        reported.removeAll()
        outageIdentity = nil
        endedConsentOrIdentity = false
    }

    func optOutRequested() {
        if recoveryRetry != nil { finishRecovery() }
        if outageIdentity != nil { endedConsentOrIdentity = true }
    }

    private func identityForOutage(allowed: Bool) throws -> String? {
        guard !endedConsentOrIdentity else { return nil }
        let id: String? = allowed && legacyConsent() ? try withIdentityLock { directory in
            guard diskConsent() else { return nil }
            return try installationID(create: outageIdentity == nil, directory: directory)
        } : nil
        if let outageIdentity, id != outageIdentity {
            endedConsentOrIdentity = true
            return nil
        }
        return id
    }

    private struct Preferences: Decodable {
        enum CodingKeys: String, CodingKey { case enabled = "analytics_enabled" }
        let enabled: Bool

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            enabled = values.contains(.enabled) ? try values.decode(Bool.self, forKey: .enabled) : true
        }
    }

    private func legacyConsent() -> Bool {
        guard let value = defaults.object(forKey: LegacyMigration.analyticsEnabledKey) else { return true }
        guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else { return false }
        return number.boolValue
    }

    private func diskConsent() -> Bool {
        guard legacyConsent() else { return false }
        do {
            let bytes = try read("preferences.json", limit: 16 * 1024)
            guard bytes.first(where: { ![9, 10, 13, 32].contains($0) }) == 0x7b else { return false }
            return try JSONDecoder().decode(Preferences.self, from: bytes).enabled
                && Self.hasUnambiguousConsent(bytes)
        } catch let error as POSIXError where error.code == .ENOENT {
            // The core's default when there is no saved preference.
            return true
        } catch {
            return false
        }
    }

    // JSONDecoder can silently pick one of two values for a repeated key.
    // The document has already been decoded, so only scan for top-level keys here.
    private static func hasUnambiguousConsent(_ data: Data) -> Bool {
        var depth = 0
        var start: Data.Index?
        var escaped = false
        var expectingKey = false
        var found = false
        for i in data.indices {
            let byte = data[i]
            if let beginning = start {
                if escaped {
                    escaped = false
                } else if byte == 0x5c { // backslash
                    escaped = true
                } else if byte == 0x22 { // quote
                    if depth == 1 && expectingKey {
                        let key = try? JSONDecoder().decode(String.self, from: data[beginning...i])
                        if key == "analytics_enabled" {
                            if found { return false }
                            found = true
                        }
                        expectingKey = false
                    }
                    start = nil
                }
                continue
            }
            switch byte {
            case 0x22: start = i
            case 0x7b, 0x5b: // object or array start
                depth += 1
                if depth == 1 { expectingKey = true }
            case 0x7d, 0x5d: depth -= 1 // object or array end
            case 0x2c where depth == 1: expectingKey = true // comma
            default: break
            }
        }
        return true
    }

    // Keep this disk protocol in sync with desktop-core's telemetry/macos_identity.rs.
    private enum IdentityLockError: Error { case busy }

    private func withIdentityLock<T>(_ body: (Int32) throws -> T?) throws -> T? {
        do {
            try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            let directory = open(dataDirectory.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard directory >= 0 else { return nil }
            defer { close(directory) }
            var info = stat()
            guard fstat(directory, &info) == 0, info.st_uid == geteuid(), info.st_mode & 0o022 == 0 else {
                return nil
            }
            let lock = open(dataDirectory.appendingPathComponent("installation-id.lock").path,
                              O_RDWR | O_CREAT | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
            guard lock >= 0 else { return nil }
            defer { close(lock) }
            // The core can hold this lock while the main actor handles a restart.
            guard safeFileSize(lock, limit: 0) != nil else { return nil }
            if flock(lock, LOCK_EX | LOCK_NB) != 0 {
                if errno == EWOULDBLOCK || errno == EINTR { throw IdentityLockError.busy }
                return nil
            }
            return try body(directory)
        } catch let error as IdentityLockError {
            throw error
        } catch {
            return nil
        }
    }

    private func installationID(create: Bool, directory: Int32) throws -> String? {
        do {
            let bytes = try read("installation-id", limit: 64)
            let whitespace = CharacterSet(charactersIn: " \t\n\r\u{000C}")
            guard let text = String(data: bytes, encoding: .utf8)?.trimmingCharacters(in: whitespace),
                  text.utf8.count == 36, let id = UUID(uuidString: text) else { return nil }
            return "desktop:\(id.uuidString.lowercased())"
        } catch let error as POSIXError where error.code == .ENOENT && create {
            try publishIdentity(directory: directory)
            return try installationID(create: false, directory: directory)
        }
    }

    private func publishIdentity(directory: Int32) throws {
        let destination = dataDirectory.appendingPathComponent("installation-id").path
        let temporary = dataDirectory.appendingPathComponent(".installation-id-\(UUID())").path
        let fd = open(temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard fd >= 0 else { throw posixError() }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        defer {
            try? file.close()
            temporary.withUnsafeBufferPointer { _ = unlink($0.baseAddress!) }
        }
        try file.write(contentsOf: Data((UUID().uuidString.lowercased() + "\n").utf8))
        try file.synchronize()
        let result = temporary.withUnsafeBufferPointer { renamex_np($0.baseAddress!, destination, UInt32(RENAME_EXCL)) }
        if result == 0 {
            guard fsync(directory) == 0 else { throw posixError() }
        } else if errno != EEXIST {
            throw posixError()
        }
    }

    private func posixError() -> POSIXError {
        POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }

    private func safeFileSize(_ fd: Int32, limit: Int) -> Int? {
        var info = stat()
        guard fstat(fd, &info) == 0, (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              info.st_uid == geteuid(), info.st_mode & 0o022 == 0, info.st_nlink == 1,
              (0...Int64(limit)).contains(info.st_size) else { return nil }
        return Int(info.st_size)
    }

    private func read(_ name: String, limit: Int) throws -> Data {
        let fd = open(dataDirectory.appendingPathComponent(name).path, O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw posixError() }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        defer { try? file.close() }
        guard let size = safeFileSize(fd, limit: limit),
              let bytes = try file.read(upToCount: limit + 1), bytes.count == size else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return bytes
    }

    private static func version(_ raw: String?) -> [Int]? {
        guard let raw, raw.utf8.count <= 15 else { return nil }
        let parts = raw.split(separator: ".", omittingEmptySubsequences: false)
        guard (1...3).contains(parts.count) else { return nil }
        let numbers = parts.compactMap { part -> Int? in
            guard (1...4).contains(part.utf8.count), part.utf8.allSatisfy({ (48...57).contains($0) }) else { return nil }
            return Int(part)
        }
        return numbers.count == parts.count ? numbers : nil
    }

    private func send(_ event: Event, id: String, reason: Reason?, service: CoreServiceDiagnostics,
                      attempts: Int, repairs: Int, lastError: Failure?, runningVersion: String?) throws -> Bool {
        let properties = Properties(reason: reason, appVersion: appVersion, bundledCoreVersion: bundledCoreVersion,
                                    runningCoreVersion: Self.version(runningVersion), osVersion: osVersion,
                                    architecture: architecture, service: service, attempts: min(max(attempts, 0), 255),
                                    repairs: min(max(repairs, 0), 255), lastError: lastError)
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let body = try? encoder.encode(Capture(apiKey: project.key, event: event, distinctId: id, properties: properties)) else {
            return false
        }
        var request = URLRequest(url: Project.endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("agentrelay-desktop", forHTTPHeaderField: "User-Agent")
        request.httpBody = body
        let sent: Bool? = try withIdentityLock { directory in
            guard diskConsent(), try installationID(create: false, directory: directory) == id else { return nil }
            return transport(request)
        }
        if sent == nil, outageIdentity != nil { endedConsentOrIdentity = true }
        return sent ?? false
    }

    private final class HTTPS: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
        @MainActor private var inFlight = 0
        @MainActor private lazy var session: URLSession = {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.timeoutIntervalForRequest = 10
            configuration.timeoutIntervalForResource = 10
            configuration.httpShouldSetCookies = false
            configuration.httpCookieStorage = nil
            configuration.urlCredentialStorage = nil
            configuration.urlCache = nil
            return URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
        }()

        @MainActor func send(_ request: URLRequest) -> Bool {
            guard inFlight < 8 else { return false }
            inFlight += 1
            session.dataTask(with: request) { [weak self] _, _, _ in
                Task { @MainActor in self?.inFlight -= 1 }
            }.resume()
            return true
        }

        func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                        newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
            completionHandler(nil)
        }
    }
}
