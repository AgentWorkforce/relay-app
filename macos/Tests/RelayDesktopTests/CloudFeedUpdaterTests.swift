import CryptoKit
import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// Answers each URL from its own queue (the last answer repeats), without a
/// `Content-Length` and, with `chunk`, in pieces of that many bytes, and
/// keeps every request's method, URL and headers. An answer whose status is
/// in `unfinished` never ends its body; one in `failing` fails after it.
final class FeedURLProtocol: URLProtocol {
    struct Seen {
        var method: String
        var url: String
        var headers: [String: String]
    }

    static let lock = NSLock()
    static var answers: [String: [(Int, Data)]] = [:]
    static var requests: [Seen] = []
    static var chunk = 0
    static var unfinished: Set<Int> = []
    static var failing: Set<Int> = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let url = request.url!.absoluteString
        Self.lock.lock()
        Self.requests.append(Seen(
            method: request.httpMethod ?? "GET", url: url, headers: request.allHTTPHeaderFields ?? [:]))
        var queue = Self.answers[url] ?? []
        let (status, body) = queue.first ?? (599, Data())
        if queue.count > 1 {
            queue.removeFirst()
            Self.answers[url] = queue
        }
        let chunk = Self.chunk > 0 ? Self.chunk : max(body.count, 1)
        let unfinished = Self.unfinished.contains(status)
        let failing = Self.failing.contains(status)
        Self.lock.unlock()
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        var offset = 0
        repeat {
            let end = min(offset + chunk, body.count)
            client?.urlProtocol(self, didLoad: body.subdata(in: offset..<end))
            offset = end
        } while offset < body.count
        if failing {
            client?.urlProtocol(self, didFailWithError: URLError(.networkConnectionLost))
        } else if !unfinished {
            client?.urlProtocolDidFinishLoading(self)
        }
    }
    override func stopLoading() {}

    static func reset(_ answers: [String: [(Int, Data)]] = [:]) {
        lock.lock()
        defer { lock.unlock() }
        self.answers = answers
        requests = []
        chunk = 0
        unfinished = []
        failing = []
    }

    static func answer(_ url: String, _ answers: (Int, Data)...) {
        lock.lock()
        defer { lock.unlock() }
        self.answers[url] = answers
    }

    static func leave(unfinished: Set<Int> = [], failing: Set<Int> = []) {
        lock.lock()
        defer { lock.unlock() }
        self.unfinished = unfinished
        self.failing = failing
    }

    static var seen: [Seen] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }
}

/// The account's feed headers: a fresh token per call, or none when signed
/// out.
final class FeedHeaderSource {
    var signedIn = true
    private(set) var sites: [String] = []

    func headers(_ site: String) -> [String: String]? {
        sites.append(site)
        return signedIn ? ["Authorization": "Bearer \(sites.count)"] : nil
    }
}

/// A cloud build's updater against a site's signed feed (proposed routes),
/// as gtk/src/updates/cloud.rs reads it.
@MainActor
final class CloudFeedUpdaterTests: XCTestCase {
    private typealias F = FeedFixtures
    private let site = "https://site.example"
    private let base = "https://site.example/cloud/api/v1/desktop/feed"
    private let signedURL = "https://storage.googleapis.com/bucket/object?signature=1"
    private let payload = Data("a disk image".utf8)
    private var session: URLSession!
    private var source: FeedHeaderSource!

    private var stableURL: String { "\(base)/manifest?channel=stable" }
    private var betaURL: String { "\(base)/manifest?channel=beta" }
    private var signingURL: String { "\(base)/assets/a_01JD3Q4W01/download-url" }
    private var streamURL: String { "\(base)/assets/a_01JD3Q4W01" }
    private var key: String { F.signingKey.publicKey.rawRepresentation.base64EncodedString() }

    override func setUp() async throws {
        FeedURLProtocol.reset()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [FeedURLProtocol.self]
        session = URLSession(configuration: configuration)
        source = FeedHeaderSource()
    }

    override func tearDown() async throws {
        session.invalidateAndCancel()
        FeedURLProtocol.reset()
    }

    /// valid-stable, signed, whose first disk image (`arch`, macOS 15) is
    /// `payload`.
    private func stable(arch: AppUpdateArchitecture = .arm64) throws -> Data {
        var text = try F.text("manifest/valid-stable.json")
        let sha256 = SHA256.hash(data: payload).map { String(format: "%02x", $0) }.joined()
        for (from, to) in [
            ("\"size\": 187236352", "\"size\": \(payload.count)"),
            ("d15c4f6b877439a86800b411899322aadef40e6aa1257f158cd3df4e1333af59", sha256),
            ("macOS15-arm64.dmg", "macOS15-\(arch.rawValue).dmg"),
            ("\"arch\": \"arm64\"", "\"arch\": \"\(arch.rawValue)\""),
        ] {
            let range = try XCTUnwrap(text.range(of: from), from)
            text.replaceSubrange(range, with: to)
        }
        return Data(F.envelope(text, try F.sign(try F.json(text))).utf8)
    }

    private func beta() throws -> Data {
        Data(try F.signed("manifest/valid-beta.json").utf8)
    }

    private func feed(publicKey: String? = nil) -> CloudFeed {
        let source = source!
        return CloudFeed(
            site: site, publicKey: publicKey ?? key, profileID: "x7k2q", assetPrefix: "Desktop-x7k2q",
            installedVersion: "2026.11.8", productName: "Example", architecture: .arm64, macOSMajorVersion: 15,
            session: session, headers: { source.headers($0) })
    }

    private func updater(_ defaults: UserDefaults, publicKey: String? = nil) -> AppUpdater {
        let updater = AppUpdater(
            defaults: defaults, session: session, currentVersion: "2026.11.8",
            releasesRepository: AppInfo.releasesRepository(from: nil, feed: "cloud"),
            releaseFeed: AppInfo.releaseFeed(from: "cloud"), assetPrefix: "Desktop-x7k2q",
            feedPublicKey: publicKey ?? key, site: site, productName: "Example", macOSMajorVersion: 15,
            development: false)
        let source = source!
        updater.feedHeaders = { source.headers($0) }
        return updater
    }

    private func withDefaults(_ body: (UserDefaults) async throws -> Void) async throws {
        let suite = "cloud-feed-test-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        try await body(defaults)
    }

    private func assertFails(
        _ message: String, file: StaticString = #filePath, line: UInt = #line, _ body: () async throws -> Any?
    ) async {
        do {
            _ = try await body()
            XCTFail("no error", file: file, line: line)
        } catch {
            XCTAssertEqual(error as? ReleaseFeedFailure, ReleaseFeedFailure(message), file: file, line: line)
        }
    }

    private func json(_ object: [String: Any]) -> Data {
        try! JSONSerialization.data(withJSONObject: object)
    }

    // MARK: Checks

    func testWithoutHeadersTheUpdaterAsksToSignInAndSendsNothing() async throws {
        source.signedIn = false
        try await withDefaults { defaults in
            let updater = self.updater(defaults)
            await updater.checkForUpdates()
            XCTAssertEqual(updater.state, .failed("Sign in to Example to receive updates."))
        }
        XCTAssertEqual(source.sites, [site])
        XCTAssertTrue(FeedURLProtocol.seen.isEmpty)
    }

    func testA401GetsFreshHeadersOnceThenAsksToSignIn() async throws {
        FeedURLProtocol.answer(stableURL, (401, Data()), (401, Data()))
        await assertFails(AppUpdateMetadata.signInMessage(product: "Example")) {
            try await self.feed().latestRelease(includeBeta: false)
        }
        XCTAssertEqual(FeedURLProtocol.seen.map { $0.headers["Authorization"] }, ["Bearer 1", "Bearer 2"])
        FeedURLProtocol.reset()
        try FeedURLProtocol.answer(stableURL, (401, Data()), (200, stable()))
        let release = try await feed().latestRelease(includeBeta: false)
        XCTAssertEqual(release?.version, "2026.11.9")
        XCTAssertEqual(FeedURLProtocol.seen.map { $0.headers["Authorization"] }, ["Bearer 3", "Bearer 4"])
        XCTAssertEqual(source.sites, Array(repeating: site, count: 4))
    }

    func testANotEntitledWorkspaceIsToldSo() async throws {
        FeedURLProtocol.answer(stableURL, (403, json(["error": "not_entitled"])))
        try await withDefaults { defaults in
            let updater = self.updater(defaults)
            await updater.checkForUpdates()
            XCTAssertEqual(updater.state, .failed("This workspace is not entitled to updates for Example."))
        }
        // Any other refusal is an unreadable answer.
        FeedURLProtocol.answer(stableURL, (403, json(["error": "forbidden"])))
        await assertFails(AppUpdateMetadata.unreadableMessage) { try await self.feed().latestRelease(includeBeta: false) }
    }

    func testNoReleaseOrNoNewerReleaseIsUpToDate() async throws {
        FeedURLProtocol.answer(stableURL, (404, Data()))
        try await withDefaults { defaults in
            let updater = self.updater(defaults)
            await updater.checkForUpdates()
            guard case .upToDate = updater.state else { return XCTFail("\(updater.state)") }
            XCTAssertNil(defaults.object(forKey: "cachedAppUpdateRelease"))
        }
        // Only the site's feed was asked, with the account's headers.
        XCTAssertEqual(FeedURLProtocol.seen.map(\.url), [stableURL])
        XCTAssertEqual(FeedURLProtocol.seen.first?.method, "GET")
        XCTAssertEqual(FeedURLProtocol.seen.first?.headers["Authorization"], "Bearer 1")
        var current = feed()
        current.installedVersion = "2026.11.9"
        try FeedURLProtocol.answer(stableURL, (200, stable()))
        let release = try await current.latestRelease(includeBeta: false)
        XCTAssertNil(release)
    }

    func testAnEmptyCheckIsRestoredAsUpToDateAfterARelaunch() async throws {
        FeedURLProtocol.answer(stableURL, (404, Data()))
        try await withDefaults { defaults in
            let updater = self.updater(defaults)
            await updater.checkForUpdates()
            guard case let .upToDate(checked) = updater.state else { return XCTFail("\(updater.state)") }
            let relaunched = self.updater(defaults)
            guard case let .upToDate(restored) = relaunched.state else { return XCTFail("\(relaunched.state)") }
            XCTAssertEqual(restored.timeIntervalSince1970, checked.timeIntervalSince1970, accuracy: 0.001)
            // The day's check is not repeated.
            await relaunched.checkAutomaticallyIfNeeded()
            XCTAssertEqual(FeedURLProtocol.seen.map(\.url), [stableURL])
            // A later check that finds a release restores that release.
            try FeedURLProtocol.answer(self.stableURL, (200, self.stable(arch: .current)))
            await relaunched.checkForUpdates()
            guard case .available = self.updater(defaults).state else { return XCTFail("not restored as available") }
            // Another channel discards an empty check too.
            FeedURLProtocol.answer(self.stableURL, (404, Data()))
            await relaunched.checkForUpdates()
            guard case .upToDate = self.updater(defaults).state else { return XCTFail("not restored as up to date") }
            relaunched.includeBetaReleases = true
            XCTAssertEqual(self.updater(defaults).state, .idle)
        }
    }

    func testAnUnendingOrBroken401IsRetriedWithoutReadingItsBody() async throws {
        let modes: [(unfinished: Set<Int>, failing: Set<Int>)] = [([401], []), ([], [401])]
        for mode in modes {
            FeedURLProtocol.reset()
            FeedURLProtocol.leave(unfinished: mode.unfinished, failing: mode.failing)
            try FeedURLProtocol.answer(stableURL, (401, Data(repeating: 0x20, count: 1024)), (200, stable()))
            let started = Date()
            let release = try await feed().latestRelease(includeBeta: false)
            XCTAssertEqual(release?.version, "2026.11.9")
            XCTAssertLessThan(Date().timeIntervalSince(started), 2)
            XCTAssertEqual(FeedURLProtocol.seen.count, 2)
        }
    }

    func testAn403BodyIsReadUntilItsDeadlineAndKept() async throws {
        FeedURLProtocol.leave(unfinished: [403])
        // With no Content-Type, URLSession holds the response back until 512 bytes or the end arrive (sniffing).
        FeedURLProtocol.answer(
            stableURL, (403, json(["error": "not_entitled", "detail": String(repeating: "x", count: 4096)])))
        var slow = feed()
        slow.refusalBodyDeadline = 0.5
        let limited = slow
        let started = Date()
        await assertFails(AppUpdateMetadata.notEntitledMessage(product: "Example")) {
            try await limited.latestRelease(includeBeta: false)
        }
        let took = Date().timeIntervalSince(started)
        XCTAssertGreaterThanOrEqual(took, 0.4)
        XCTAssertLessThan(took, 2)
    }

    func testAVerifiedReleaseIsOfferedWithItsAssetAndPage() async throws {
        try FeedURLProtocol.answer(stableURL, (200, stable(arch: .current)))
        try await withDefaults { defaults in
            let updater = self.updater(defaults)
            await updater.checkForUpdates()
            guard case let .available(release) = updater.state else { return XCTFail("\(updater.state)") }
            XCTAssertEqual(release.version, "2026.11.9")
            XCTAssertEqual(release.downloadURL.absoluteString, streamURL)
            XCTAssertNil(release.checksumURL)
            XCTAssertEqual(release.feed?.name, "Desktop-x7k2q-2026.11.9-macOS15-\(AppUpdateArchitecture.current.rawValue).dmg")
            XCTAssertEqual(release.feed?.size, Int64(payload.count))
            XCTAssertEqual(release.feed?.downloadHosts, ["storage.googleapis.com"])
            XCTAssertEqual(release.feed?.page?.absoluteString, "https://site.example/cloud/dashboard/downloads")
        }
        // A tampered manifest is never offered.
        let signed = String(decoding: try stable(), as: UTF8.self)
        let tampered = signed.replacingOccurrences(of: "10:04:31Z", with: "10:04:32Z")
        FeedURLProtocol.answer(stableURL, (200, Data(tampered.utf8)))
        await assertFails(AppUpdateMetadata.feedUnverifiedMessage) { try await self.feed().latestRelease(includeBeta: false) }
    }

    func testABetaSubscriberGetsTheNewerOfStableAndBeta() async throws {
        try FeedURLProtocol.answer(stableURL, (200, stable()))
        try FeedURLProtocol.answer(betaURL, (200, beta()))
        let newer = try await feed().latestRelease(includeBeta: true)
        XCTAssertEqual(newer?.version, "2026.11.10-beta.2")
        XCTAssertEqual(newer?.feed?.name, "Desktop-x7k2q-2026.11.10-beta.2-macOS15-arm64.dmg")
        XCTAssertEqual(Set(FeedURLProtocol.seen.map(\.url)), [stableURL, betaURL])
        FeedURLProtocol.answer(betaURL, (404, Data()))
        let stableOnly = try await feed().latestRelease(includeBeta: true)
        XCTAssertEqual(stableOnly?.version, "2026.11.9")
        // A failure on either channel fails the check.
        FeedURLProtocol.answer(betaURL, (500, Data()))
        await assertFails(AppUpdateMetadata.unreadableMessage) { try await self.feed().latestRelease(includeBeta: true) }
        // Without beta, the beta channel is not asked.
        let manifest = try stable()
        FeedURLProtocol.reset([stableURL: [(200, manifest)]])
        _ = try await feed().latestRelease(includeBeta: false)
        XCTAssertEqual(FeedURLProtocol.seen.map(\.url), [stableURL])
    }

    func testWithoutAKeyNothingIsAskedAndTheFeedIsUnverified() async throws {
        for publicKey in ["", "not base64", F.base64url(F.signingKey.publicKey.rawRepresentation)] {
            await assertFails(AppUpdateMetadata.feedUnverifiedMessage) {
                try await self.feed(publicKey: publicKey).latestRelease(includeBeta: true)
            }
        }
        try await withDefaults { defaults in
            let updater = self.updater(defaults, publicKey: "")
            await updater.checkForUpdates()
            XCTAssertEqual(updater.state, .failed("The update could not be verified."))
        }
        XCTAssertTrue(source.sites.isEmpty)
        XCTAssertTrue(FeedURLProtocol.seen.isEmpty)
    }

    // MARK: Downloads

    /// The offered release, then a clean request log.
    private func offered() async throws -> AppRelease {
        try FeedURLProtocol.answer(stableURL, (200, stable()))
        let latest = try await feed().latestRelease(includeBeta: false)
        let release = try XCTUnwrap(latest)
        FeedURLProtocol.reset()
        return release
    }

    func testASignedURLIsFetchedWithoutTheSitesHeaders() async throws {
        let release = try await offered()
        FeedURLProtocol.answer(signingURL, (200, json(["url": signedURL, "expires_at": "2026-11-20T11:00:00Z"])))
        FeedURLProtocol.answer(signedURL, (200, payload))
        let file = try await feed().diskImage(of: release)
        defer { try? FileManager.default.removeItem(at: file) }
        XCTAssertEqual(try Data(contentsOf: file), payload)
        let seen = FeedURLProtocol.seen
        XCTAssertEqual(seen.map(\.method), ["POST", "GET"])
        XCTAssertEqual(seen.map(\.url), [signingURL, signedURL])
        XCTAssertEqual(seen[0].headers["Authorization"], "Bearer 2")
        XCTAssertNil(seen[1].headers["Authorization"])
    }

    func testASignedURLOffTheManifestsHostsIsRefused() async throws {
        let release = try await offered()
        for url in [
            "https://evil.example/object", "http://storage.googleapis.com/object",
            "https://storage.googleapis.com:443/object", "https://user@storage.googleapis.com/object",
        ] {
            FeedURLProtocol.answer(signingURL, (200, json(["url": url])))
            await assertFails(AppUpdateMetadata.feedUnverifiedMessage) { try await self.feed().diskImage(of: release) }
        }
        XCTAssertEqual(Set(FeedURLProtocol.seen.map(\.url)), [signingURL])
    }

    func testAChecksumOrSizeMismatchDiscardsTheDownload() async throws {
        let release = try await offered()
        FeedURLProtocol.answer(signingURL, (200, json(["url": signedURL])))
        for body in [Data("a disk imagE".utf8), payload.dropLast()] {
            FeedURLProtocol.answer(signedURL, (200, body))
            await assertFails(AppUpdateMetadata.unverifiedMessage) { try await self.feed().diskImage(of: release) }
        }
        // One byte past the signed size is refused as it arrives.
        FeedURLProtocol.answer(signedURL, (200, payload + Data([0])))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().diskImage(of: release) }
    }

    /// The partial downloads in the temporary directory.
    private func partialFiles() throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: FileManager.default.temporaryDirectory.path)
            .filter { $0.hasPrefix("agent-relay-feed-") }
    }

    func testAStreamedImageOverItsSignedSizeIsCutOffOnBothPaths() async throws {
        let release = try await offered()
        let before = try partialFiles()
        // No Content-Length and many pieces: only the bytes received count.
        let oversized = Data(repeating: 0x61, count: 256 * 1024)
        FeedURLProtocol.chunk = 4096
        FeedURLProtocol.answer(signingURL, (200, json(["url": signedURL])))
        FeedURLProtocol.answer(signedURL, (200, oversized))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().diskImage(of: release) }
        FeedURLProtocol.answer(signingURL, (501, Data()))
        FeedURLProtocol.answer(streamURL, (200, oversized))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().diskImage(of: release) }
        XCTAssertEqual(Set(FeedURLProtocol.seen.map(\.url)), [signingURL, signedURL, streamURL])
        XCTAssertEqual(try partialFiles(), before)
    }

    func testAnOversizedManifestOrDownloadURLAnswerIsRefused() async throws {
        let oversized = Data(repeating: 0x20, count: CloudFeed.maxFeedAnswerSize + 1)
        FeedURLProtocol.chunk = 65_536
        FeedURLProtocol.answer(stableURL, (200, oversized))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().latestRelease(includeBeta: false) }
        let release = try await offered()
        FeedURLProtocol.chunk = 65_536
        FeedURLProtocol.answer(signingURL, (200, oversized))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().diskImage(of: release) }
        XCTAssertEqual(FeedURLProtocol.seen.map(\.url), [signingURL])
    }

    func testASiteThatOnlyStreamsServesTheImageWithTheHeaders() async throws {
        let release = try await offered()
        FeedURLProtocol.answer(signingURL, (501, Data()))
        FeedURLProtocol.answer(streamURL, (200, payload))
        let file = try await feed().diskImage(of: release)
        defer { try? FileManager.default.removeItem(at: file) }
        XCTAssertEqual(try Data(contentsOf: file), payload)
        let seen = FeedURLProtocol.seen
        XCTAssertEqual(seen.map(\.method), ["POST", "GET"])
        XCTAssertEqual(seen.map(\.url), [signingURL, streamURL])
        XCTAssertEqual(seen.map { $0.headers["Authorization"] }, ["Bearer 2", "Bearer 3"])
        // A stream refused for entitlement says so.
        FeedURLProtocol.answer(streamURL, (403, json(["error": "not_entitled"])))
        await assertFails(AppUpdateMetadata.notEntitledMessage(product: "Example")) {
            try await self.feed().diskImage(of: release)
        }
    }

    func testAnUnending501StreamsTheImageWithoutReadingItsBody() async throws {
        let release = try await offered()
        FeedURLProtocol.leave(unfinished: [501])
        FeedURLProtocol.answer(signingURL, (501, Data(repeating: 0x20, count: 1024)))
        FeedURLProtocol.answer(streamURL, (200, payload))
        let started = Date()
        let file = try await feed().diskImage(of: release)
        defer { try? FileManager.default.removeItem(at: file) }
        XCTAssertLessThan(Date().timeIntervalSince(started), 2)
        XCTAssertEqual(try Data(contentsOf: file), payload)
        XCTAssertEqual(FeedURLProtocol.seen.map(\.url), [signingURL, streamURL])
    }

    func testAnImageOverTheCapIsNeverRequested() async throws {
        let release = try await offered()
        let feedAsset = try XCTUnwrap(release.feed)
        let large = AppRelease(
            version: release.version, downloadURL: release.downloadURL, checksumURL: nil,
            feed: AppReleaseFeedAsset(
                assetID: feedAsset.assetID, name: feedAsset.name, size: AppUpdateMetadata.maxDownloadSize + 1,
                sha256: feedAsset.sha256, downloadHosts: feedAsset.downloadHosts, page: feedAsset.page))
        await assertFails(AppUpdateMetadata.tooLargeMessage) { try await self.feed().diskImage(of: large) }
        XCTAssertTrue(FeedURLProtocol.seen.isEmpty)
    }
}
