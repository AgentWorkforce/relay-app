import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

final class UpdateURLProtocol: URLProtocol {
    static let lock = NSLock()
    static var responseData = Data()
    static var statusCode = 200
    static var requests = 0
    static var responsesByURL: [URL: Data] = [:]
    static var requestedURLs: [URL] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock()
        Self.requests += 1
        Self.requestedURLs.append(request.url!)
        let data = Self.responsesByURL[request.url!] ?? Self.responseData
        let statusCode = Self.statusCode
        Self.lock.unlock()
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: statusCode,
            httpVersion: nil,
            headerFields: nil
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}

    static func reset(with data: Data, statusCode: Int = 200) {
        lock.lock()
        defer { lock.unlock() }
        responseData = data
        self.statusCode = statusCode
        requests = 0
        responsesByURL = [:]
        requestedURLs = []
    }

    static func setResponse(data: Data, statusCode: Int) {
        lock.lock()
        defer { lock.unlock() }
        responseData = data
        self.statusCode = statusCode
    }

    static var requestCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }
}

@MainActor
final class AppUpdateTests: XCTestCase {
    func testBetaSelectionAndStablePromotion() throws {
        let stable = releaseJSON(version: "2026.10.1")
        let beta9 = releaseJSON(version: "2026.10.2-beta.9", prerelease: true)
        let beta10 = releaseJSON(version: "2026.10.2-beta.10", prerelease: true)
        let draft = releaseJSON(version: "2026.10.99-beta.1", prerelease: true, draft: true)
        let rc = releaseJSON(version: "2026.10.98-rc.1", prerelease: true)
        let mislabeled = releaseJSON(version: "2026.10.97-beta.1")
        let page = try releaseList([stable, beta10, draft, beta9, rc, mislabeled])
        let selected = try AppUpdateMetadata.newestRelease(from: [page], architecture: .arm64)
        XCTAssertEqual(selected.version, "2026.10.2-beta.10")
        XCTAssertThrowsError(try AppUpdateMetadata.release(from: beta10, architecture: .arm64))
        XCTAssertThrowsError(try AppUpdateMetadata.release(from: mislabeled, architecture: .arm64))
        let promoted = try AppUpdateMetadata.newestRelease(
            from: [page, releaseList([releaseJSON(version: "2026.10.2")])], architecture: .arm64)
        XCTAssertEqual(promoted.version, "2026.10.2")
        XCTAssertTrue(AppUpdateMetadata.isNewer(promoted.version, than: selected.version))
        XCTAssertFalse(AppUpdateMetadata.isNewer("2026.10.1", than: selected.version))
        XCTAssertEqual(try AppUpdateMetadata.newestRelease(
            from: [releaseList([stable])], architecture: .x64).version, "2026.10.1")
        XCTAssertThrowsError(try AppUpdateMetadata.newestRelease(from: [Data("[]".utf8)], architecture: .arm64))
    }

    func testBetaStillRequiresVerifiedAssets() throws {
        let beta = releaseJSON(version: "2026.10.2-beta.1", omitChecksum: true, prerelease: true)
        XCTAssertThrowsError(try AppUpdateMetadata.newestRelease(from: [releaseList([beta])], architecture: .x64))
        var value = try XCTUnwrap(JSONSerialization.jsonObject(with: beta) as? [String: Any])
        var assets = try XCTUnwrap(value["assets"] as? [[String: String]])
        assets[0]["browser_download_url"] = "https://example.com/update.dmg"
        value["assets"] = assets
        XCTAssertThrowsError(try AppUpdateMetadata.newestRelease(
            from: [JSONSerialization.data(withJSONObject: [value])], architecture: .arm64))
    }

    func testChangingChannelInvalidatesCacheAndThrottleAndBetaChecksPaginate() async throws {
        let stable = releaseJSON(version: "2026.10.1")
        UpdateURLProtocol.reset(with: stable)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3")
        XCTAssertFalse(updater.includeBetaReleases)
        await updater.checkForUpdates()
        updater.includeBetaReleases = true
        XCTAssertEqual(updater.state, .idle)
        UpdateURLProtocol.responsesByURL = [
            AppUpdateMetadata.releasesURL(page: 1): try releaseList(Array(repeating: stable, count: 100)),
            AppUpdateMetadata.releasesURL(page: 2): try releaseList([
                releaseJSON(version: "2026.10.2-beta.10", prerelease: true)])
        ]
        await updater.checkAutomaticallyIfNeeded()
        guard case let .available(beta) = updater.state else { return XCTFail("Expected beta") }
        XCTAssertEqual(beta.version, "2026.10.2-beta.10")
        let restored = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3")
        XCTAssertTrue(restored.includeBetaReleases)
        XCTAssertEqual(restored.state, updater.state)
        restored.includeBetaReleases = false
        XCTAssertEqual(restored.state, .idle)
        let fresh = AppUpdater(defaults: defaults, session: session, currentVersion: beta.version)
        XCTAssertFalse(fresh.includeBetaReleases)
        XCTAssertEqual(fresh.state, .idle)
        await fresh.checkAutomaticallyIfNeeded()
        guard case .upToDate = fresh.state else { return XCTFail("Should wait for stable without downgrading") }
        XCTAssertEqual(UpdateURLProtocol.requestedURLs, [AppUpdateMetadata.latestReleaseURL(),
            AppUpdateMetadata.releasesURL(page: 1), AppUpdateMetadata.releasesURL(page: 2),
            AppUpdateMetadata.latestReleaseURL()])
    }

    func testUpdatesAreCheckedInTheRepositoryTheBuildWasMadeFor() async throws {
        let stable = releaseJSON(version: "2026.10.1")
        UpdateURLProtocol.reset(with: stable)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3",
                                 releasesRepository: "Example/relay-releases")
        await updater.checkForUpdates()
        guard case let .available(release) = updater.state else {
            return XCTFail("Expected an available update, got \(updater.state)")
        }
        XCTAssertEqual(release.version, "2026.10.1")
        XCTAssertEqual(UpdateURLProtocol.requestedURLs,
                       [URL(string: "https://api.github.com/repos/Example/relay-releases/releases/latest")!])
        updater.includeBetaReleases = true
        UpdateURLProtocol.responsesByURL = [
            AppUpdateMetadata.releasesURL(page: 1, repository: "Example/relay-releases"): try releaseList([stable])
        ]
        await updater.checkForUpdates()
        XCTAssertEqual(UpdateURLProtocol.requestedURLs.last,
                       URL(string: "https://api.github.com/repos/Example/relay-releases/releases?per_page=100&page=1")!)
        // Production's repository when a build names none.
        XCTAssertEqual(AppUpdateMetadata.latestReleaseURL().absoluteString,
                       "https://api.github.com/repos/AgentWorkforce/relay-desktop-releases/releases/latest")
        XCTAssertEqual(AppUpdateMetadata.releasesURL(page: 2).absoluteString,
                       "https://api.github.com/repos/AgentWorkforce/relay-desktop-releases/releases?per_page=100&page=2")
    }

    private func releaseList(_ releases: [Data]) throws -> Data {
        try JSONSerialization.data(withJSONObject: releases.map { try JSONSerialization.jsonObject(with: $0) })
    }

    func testParsesVersionedAssetForArchitecture() throws {
        let release = try AppUpdateMetadata.release(
            from: releaseJSON(version: "2026.09.10"),
            architecture: .arm64
        )

        XCTAssertEqual(release.version, "2026.09.10")
        XCTAssertEqual(release.downloadURL.lastPathComponent, "AgentRelay-2026.09.10-macOS-arm64.dmg")
        XCTAssertEqual(release.checksumURL?.lastPathComponent, "AgentRelay-2026.09.10-macOS-arm64.dmg.sha256")
    }

    func testAProfileBuildLooksForItsOwnAssetPrefix() throws {
        // A site profile's release carries Desktop-<id>- images only; its Mac
        // app looks for those, and the default name is never a fallback.
        let release = releaseJSON(version: "2026.11.3", assetNames: [
            "Desktop-acme-7-2026.11.3-macOS-arm64.dmg", "Desktop-acme-7-2026.11.3-macOS-arm64.dmg.sha256",
        ])
        let selected = try AppUpdateMetadata.release(from: release, architecture: .arm64, assetPrefix: "Desktop-acme-7")
        XCTAssertEqual(selected.downloadURL.lastPathComponent, "Desktop-acme-7-2026.11.3-macOS-arm64.dmg")
        XCTAssertThrowsError(try AppUpdateMetadata.release(from: release, architecture: .arm64)) { error in
            XCTAssertEqual(error as? AppUpdateMetadataError, .missingAsset("AgentRelay-2026.11.3-macOS-arm64.dmg"))
        }
        XCTAssertEqual(try AppUpdateMetadata.newestRelease(
            from: [releaseList([release])], architecture: .arm64, assetPrefix: "Desktop-acme-7").version, "2026.11.3")
        // The bundle record: a plain name, else the default.
        XCTAssertEqual(AppInfo.assetPrefix(from: "Desktop-acme-7\n"), "Desktop-acme-7")
        XCTAssertEqual(AppInfo.assetPrefix(from: nil), "AgentRelay")
        XCTAssertEqual(AppInfo.assetPrefix(from: "Desktop acme"), "AgentRelay")
        XCTAssertEqual(AppInfo.assetPrefix(from: "-Desktop"), "AgentRelay")
    }

    func testRequiresMatchingDiskImageAndChecksum() throws {
        XCTAssertThrowsError(
            try AppUpdateMetadata.release(from: releaseJSON(version: "2026.09.10", omitChecksum: true), architecture: .x64)
        ) { error in
            XCTAssertEqual(
                error as? AppUpdateMetadataError,
                .missingAsset("AgentRelay-2026.09.10-macOS-x64.dmg.sha256")
            )
        }
    }

    func testPrefersTheHighestOSFloorThisMacMeets() throws {
        let data = releaseJSON(version: "2026.11.1", assetNames: [
            "AgentRelay-2026.11.1-macOS-arm64.dmg", "AgentRelay-2026.11.1-macOS-arm64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS13-arm64.dmg", "AgentRelay-2026.11.1-macOS13-arm64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS15-arm64.dmg", "AgentRelay-2026.11.1-macOS15-arm64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS26-x64.dmg", "AgentRelay-2026.11.1-macOS26-x64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS15-arm64-probe.tar.gz"
        ])
        // Below every floor, the unfloored image.
        for (os, expected) in [(12, "macOS"), (13, "macOS13"), (14, "macOS13"), (15, "macOS15"), (26, "macOS15")] {
            let release = try AppUpdateMetadata.release(from: data, architecture: .arm64, macOSMajorVersion: os)
            XCTAssertEqual(release.downloadURL.lastPathComponent, "AgentRelay-2026.11.1-\(expected)-arm64.dmg")
            XCTAssertEqual(release.checksumURL?.lastPathComponent, "AgentRelay-2026.11.1-\(expected)-arm64.dmg.sha256")
        }
    }

    func testFlooredImagesThisMacCannotOpenAreNotInstalled() throws {
        let data = releaseJSON(version: "2026.11.1", assetNames: [
            "AgentRelay-2026.11.1-macOS15-arm64.dmg", "AgentRelay-2026.11.1-macOS15-arm64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS16-arm64.dmg", "AgentRelay-2026.11.1-macOS16-arm64.dmg.sha256",
            "AgentRelay-macOS-arm64.dmg", "AgentRelay-macOS-arm64.dmg.sha256"
        ])
        XCTAssertThrowsError(try AppUpdateMetadata.release(from: data, architecture: .arm64, macOSMajorVersion: 14)) {
            XCTAssertEqual($0 as? AppUpdateMetadataError, .requiresNewerMacOS(15))
        }
        XCTAssertThrowsError(try AppUpdateMetadata.newestRelease(
            from: [releaseList([data])], architecture: .arm64, macOSMajorVersion: 13)) {
            XCTAssertEqual($0 as? AppUpdateMetadataError, .requiresNewerMacOS(15))
        }
    }

    func testFlooredImageStillRequiresItsOwnChecksum() throws {
        let data = releaseJSON(version: "2026.11.1", assetNames: [
            "AgentRelay-2026.11.1-macOS-x64.dmg", "AgentRelay-2026.11.1-macOS-x64.dmg.sha256",
            "AgentRelay-2026.11.1-macOS13-x64.dmg"
        ])
        XCTAssertThrowsError(try AppUpdateMetadata.release(from: data, architecture: .x64, macOSMajorVersion: 15)) {
            XCTAssertEqual($0 as? AppUpdateMetadataError, .missingAsset("AgentRelay-2026.11.1-macOS13-x64.dmg.sha256"))
        }
    }

    func testUpdaterExplainsAnUpdateThisMacCannotRun() async throws {
        let floor = AppUpdateMetadata.currentMacOSMajorVersion + 1
        let arch = AppUpdateArchitecture.current.rawValue
        UpdateURLProtocol.reset(with: releaseJSON(version: "2026.11.1", assetNames: [
            "AgentRelay-2026.11.1-macOS\(floor)-\(arch).dmg", "AgentRelay-2026.11.1-macOS\(floor)-\(arch).dmg.sha256"
        ]))
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.10.1")
        await updater.checkForUpdates()
        XCTAssertEqual(updater.state, .failed(
            "This update needs macOS \(floor) or newer. This Mac stays on the current version."))
    }

    func testComparesCalVerNumerically() {
        XCTAssertTrue(AppUpdateMetadata.isNewer("2026.09.10", than: "2026.09.3"))
        XCTAssertFalse(AppUpdateMetadata.isNewer("2026.09.3", than: "2026.09.10"))
        XCTAssertFalse(AppUpdateMetadata.isNewer("2026.09.3", than: "2026.09.3"))
        XCTAssertTrue(AppUpdateMetadata.isNewer("2026.09.3", than: "2026.09.3-rc.1"))
        XCTAssertFalse(AppUpdateMetadata.isNewer("2026.09.3", than: "development build"))
    }

    func testParsesStandardSHA256File() {
        let digest = String(repeating: "a", count: 64)
        let data = Data("\(digest)  AgentRelay-2026.09.10-macOS-arm64.dmg\n".utf8)
        XCTAssertEqual(AppUpdateMetadata.checksum(from: data), digest)
        XCTAssertNil(AppUpdateMetadata.checksum(from: Data("not-a-checksum file.dmg".utf8)))
    }

    func testUpdaterFindsNewReleaseAndThrottlesAutomaticChecks() async throws {
        let data = releaseJSON(version: "2026.09.10")
        UpdateURLProtocol.reset(with: data)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var date = Date(timeIntervalSince1970: 1_000)
        let updater = AppUpdater(
            defaults: defaults,
            session: session,
            currentVersion: "2026.09.3",
            now: { date }
        )

        await updater.checkForUpdates()
        guard case let .available(release) = updater.state else {
            return XCTFail("Expected an available update, got \(updater.state)")
        }
        XCTAssertEqual(release.version, "2026.09.10")
        XCTAssertEqual(UpdateURLProtocol.requestCount, 1)

        date.addTimeInterval(60 * 60)
        let restoredUpdater = AppUpdater(
            defaults: defaults,
            session: session,
            currentVersion: "2026.09.3",
            now: { date }
        )
        guard case let .available(restoredRelease) = restoredUpdater.state else {
            return XCTFail("Expected the cached update after relaunch, got \(restoredUpdater.state)")
        }
        XCTAssertEqual(restoredRelease, release)

        await restoredUpdater.checkAutomaticallyIfNeeded()
        XCTAssertEqual(UpdateURLProtocol.requestCount, 1)
    }

    func testCachedReleaseIsRestoredOnlyForTheVersionAndMacOSThatChoseIt() async throws {
        let arch = AppUpdateArchitecture.current.rawValue
        let floored = "AgentRelay-2026.11.1-macOS15-\(arch).dmg"
        let bridge = "AgentRelay-2026.11.1-macOS-\(arch).dmg"
        UpdateURLProtocol.reset(with: releaseJSON(version: "2026.11.1", assetNames: [
            floored, "\(floored).sha256", bridge, "\(bridge).sha256"
        ]))
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let date = Date(timeIntervalSince1970: 1_000)
        func updater(_ version: String, macOS: Int = 15) -> AppUpdater {
            AppUpdater(defaults: defaults, session: session, currentVersion: version,
                       macOSMajorVersion: macOS, now: { date })
        }

        // What an updater older than 2026.10.65 leaves behind: the newest
        // release with the unfloored bridge image, checked minutes ago.
        let legacyURL = try XCTUnwrap(URL(string: "https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/latest/\(bridge)"))
        defaults.set(date, forKey: "lastAppUpdateCheck")
        defaults.set(false, forKey: "cachedAppUpdateIncludesBeta")
        defaults.set(try JSONEncoder().encode(AppRelease(
            version: "2026.11.1", downloadURL: legacyURL,
            checksumURL: legacyURL.appendingPathExtension("sha256"))), forKey: "cachedAppUpdateRelease")

        let bridged = updater("2026.10.65")
        XCTAssertEqual(bridged.state, .idle)
        await bridged.checkAutomaticallyIfNeeded()
        XCTAssertEqual(UpdateURLProtocol.requestCount, 1)
        guard case let .available(release) = bridged.state else {
            return XCTFail("Expected a fresh lookup, got \(bridged.state)")
        }
        XCTAssertEqual(release.downloadURL.lastPathComponent, floored)

        let relaunched = updater("2026.10.65")
        XCTAssertEqual(relaunched.state, .available(release))
        await relaunched.checkAutomaticallyIfNeeded()
        XCTAssertEqual(UpdateURLProtocol.requestCount, 1)

        let otherMacOS = updater("2026.10.65", macOS: 16)
        XCTAssertEqual(otherMacOS.state, .idle)
        await otherMacOS.checkAutomaticallyIfNeeded()
        XCTAssertEqual(UpdateURLProtocol.requestCount, 2)
    }

    func testAutomaticChecksCanBeDisabled() async throws {
        UpdateURLProtocol.reset(with: releaseJSON(version: "2026.09.10"))
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3")
        updater.automaticallyChecksForUpdates = false

        await updater.checkAutomaticallyIfNeeded()

        XCTAssertEqual(UpdateURLProtocol.requestCount, 0)
        XCTAssertEqual(updater.state, .idle)
    }

    func testAutomaticCheckRetriesAfterFailure() async throws {
        let data = releaseJSON(version: "2026.09.10")
        UpdateURLProtocol.reset(with: data, statusCode: 503)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3")

        await updater.checkAutomaticallyIfNeeded()
        guard case .failed = updater.state else {
            return XCTFail("Expected a failed first check, got \(updater.state)")
        }
        XCTAssertEqual(UpdateURLProtocol.requestCount, 1)

        UpdateURLProtocol.setResponse(data: data, statusCode: 200)
        await updater.checkAutomaticallyIfNeeded()
        guard case .available = updater.state else {
            return XCTFail("Expected the retry to find the update, got \(updater.state)")
        }
        XCTAssertEqual(UpdateURLProtocol.requestCount, 2)
    }

    func testFailedUpdateRecordsAttemptAndSafeFailureWithoutInstalling() async throws {
        UpdateURLProtocol.reset(with: Data("private server diagnostic".utf8), statusCode: 503)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UpdateURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let suite = "app-update-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var terminated = false
        let updater = AppUpdater(defaults: defaults, session: session, currentVersion: "2026.09.3",
                                 terminate: { terminated = true })
        var events: [TelemetryEvent] = []
        updater.recordTelemetry = { events.append($0) }
        let release = try AppUpdateMetadata.release(from: releaseJSON(version: "2026.09.10"), architecture: .arm64)
        await updater.downloadAndInstall(release)
        XCTAssertFalse(terminated)
        XCTAssertEqual(events.map(\.operation), [.updateInstall, .updateInstall])
        XCTAssertEqual(events.map(\.outcome), [.started, .failed])
        XCTAssertEqual(events.last?.failure, .updateFailed)
        XCTAssertFalse(String(data: try JSONEncoder().encode(events), encoding: .utf8)!.contains("private"))
    }

    func testUpdateTelemetryDeadlineDoesNotWaitForUncooperativeRPC() async {
        let updater = AppUpdater()
        updater.telemetryTimeoutNanoseconds = 10_000_000
        var reply: CheckedContinuation<Void, Never>?
        let lateReplyCompleted = expectation(description: "The resumed RPC finishes after the deadline")
        var completed = false
        updater.recordTelemetry = { _ in
            await withCheckedContinuation { reply = $0 }
            completed = true
            lateReplyCompleted.fulfill()
        }
        let start = Date()
        await updater.recordUpdateTelemetry(TelemetryEvent(operation: .updateInstall, outcome: .succeeded))
        XCTAssertLessThan(Date().timeIntervalSince(start), 1)
        XCTAssertFalse(completed)
        XCTAssertNotNil(reply)
        // A late, cancellation-insensitive reply must not resume twice.
        reply?.resume()
        await fulfillment(of: [lateReplyCompleted], timeout: 1)
        XCTAssertTrue(completed)
    }

    func testUpdateTelemetryWaitsForReplyBeforeContinuing() async {
        let updater = AppUpdater()
        var completed = false
        updater.recordTelemetry = { _ in
            await Task.yield()
            completed = true
        }
        await updater.recordUpdateTelemetry(TelemetryEvent(operation: .updateInstall, outcome: .succeeded))
        XCTAssertTrue(completed)
    }

    /// A development build (a loopback site) has a real version but no
    /// releases of its own; it never asks production's repository.
    func testDevelopmentBuildsDoNotCheckForUpdates() async {
        let defaults = UserDefaults(suiteName: "agent-relay-dev-updates-\(UUID().uuidString)")!
        let updater = AppUpdater(defaults: defaults, session: .shared, currentVersion: "2026.09.3", development: true)
        await updater.checkAutomaticallyIfNeeded()
        XCTAssertNil(defaults.object(forKey: "lastAppUpdateCheck"))
        await updater.checkForUpdates()
        if case .unavailable = updater.state {} else {
            XCTFail("a development build must report updates unavailable, got \(updater.state)")
        }
    }

    func testInstallerRejectsNonAppDestinationBeforeMounting() {
        let image = URL(fileURLWithPath: "/nonexistent/update.dmg")
        let destination = URL(fileURLWithPath: "/Applications/Other.app")
        XCTAssertThrowsError(try AppUpdateInstaller.prepare(diskImage: image, installedApp: destination)) { error in
            XCTAssertEqual(error as? AppUpdateInstallError, .invalidLocation)
        }
    }

    /// A build for another site checks for its own bundle name, not the
    /// production app's: with its name the installer gets past the location
    /// check (and fails only on the image, which does not exist here), with
    /// production's it does not.
    func testInstallerChecksTheBuildsOwnBundleNameBeforeMounting() throws {
        let work = FileManager.default.temporaryDirectory.appendingPathComponent("agent-relay-profile-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: work) }
        let installed = work.appendingPathComponent("Relay X.app")
        try FileManager.default.createDirectory(at: installed, withIntermediateDirectories: true)
        let image = URL(fileURLWithPath: "/nonexistent/update.dmg")
        XCTAssertThrowsError(try AppUpdateInstaller.prepare(
            diskImage: image, installedApp: installed, bundleName: "Relay X.app", bundleIdentifier: "com.example.relay"
        )) { error in
            XCTAssertEqual(error as? AppUpdateInstallError, .invalidImage)
        }
        XCTAssertThrowsError(try AppUpdateInstaller.prepare(
            diskImage: image, installedApp: installed, bundleName: "Agent Relay.app", bundleIdentifier: "com.agentrelay.desktop"
        )) { error in
            XCTAssertEqual(error as? AppUpdateInstallError, .invalidLocation)
        }
    }

    func testInstallerRefusesAppsNotSignedByOurTeam() throws {
        let work = FileManager.default.temporaryDirectory.appendingPathComponent("agent-relay-signing-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: work) }
        let app = work.appendingPathComponent("Agent Relay.app")
        let macOS = app.appendingPathComponent("Contents/MacOS")
        try FileManager.default.createDirectory(at: macOS, withIntermediateDirectories: true)
        try FileManager.default.copyItem(atPath: "/usr/bin/true", toPath: macOS.appendingPathComponent("Agent Relay").path)
        let plist: [String: Any] = ["CFBundleIdentifier": "com.agentrelay.desktop", "CFBundleExecutable": "Agent Relay"]
        try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
            .write(to: app.appendingPathComponent("Contents/Info.plist"))
        let sign = Process()
        sign.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
        sign.arguments = ["--force", "--sign", "-", app.path]
        sign.standardError = FileHandle.nullDevice
        try sign.run()
        sign.waitUntilExit()
        XCTAssertEqual(sign.terminationStatus, 0)

        // A validly signed app with our bundle id is still refused without our team's certificate.
        XCTAssertThrowsError(try AppUpdateInstaller.verifySignature(of: app)) { error in
            XCTAssertEqual(error as? AppUpdateInstallError, .invalidApp)
        }
        XCTAssertTrue(AppUpdateInstaller.signingRequirement.contains(#"certificate leaf[subject.OU] = "QUJ7SA6X8X""#))
    }

    func testInstallerAcceptsReleaseSignedByOurTeam() throws {
        let installed = URL(fileURLWithPath: "/Applications/Agent Relay.app")
        let authority = Process()
        authority.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
        authority.arguments = ["--verify", "-R=anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists", installed.path]
        authority.standardError = FileHandle.nullDevice
        guard FileManager.default.fileExists(atPath: installed.path), (try? authority.run()) != nil else {
            throw XCTSkip("No Developer ID signed Agent Relay installed")
        }
        authority.waitUntilExit()
        guard authority.terminationStatus == 0 else { throw XCTSkip("Installed Agent Relay is not a Developer ID release") }
        XCTAssertNoThrow(try AppUpdateInstaller.verifySignature(of: installed))
    }

    private func releaseJSON(version: String, omitChecksum: Bool = false, prerelease: Bool = false, draft: Bool = false) -> Data {
        var assets = [
            asset("AgentRelay-\(version)-macOS-arm64.dmg"),
            asset("AgentRelay-\(version)-macOS-arm64.dmg.sha256"),
            asset("AgentRelay-\(version)-macOS-x64.dmg")
        ]
        if !omitChecksum { assets.append(asset("AgentRelay-\(version)-macOS-x64.dmg.sha256")) }
        return try! JSONSerialization.data(withJSONObject: [
            "tag_name": "v\(version)",
            "draft": draft,
            "prerelease": prerelease,
            "assets": assets
        ])
    }

    private func releaseJSON(version: String, assetNames: [String]) -> Data {
        try! JSONSerialization.data(withJSONObject: [
            "tag_name": "v\(version)",
            "draft": false,
            "prerelease": false,
            "assets": assetNames.map(asset)
        ])
    }

    private func asset(_ name: String) -> [String: String] {
        [
            "name": name,
            "browser_download_url": "https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/latest/\(name)"
        ]
    }
}
