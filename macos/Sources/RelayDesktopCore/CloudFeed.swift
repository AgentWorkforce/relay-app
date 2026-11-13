import Foundation

/// Updates from the signed release feed of a `cloud` build, as on Linux and
/// Windows (gtk/src/updates/cloud.rs): the check reads the manifest from the
/// site this build was made for (fixed at build time, like its branding)
/// with the signed-in account's headers and verifies it
/// (`ReleaseFeedVerifier`); the download asks the site for a signed URL to
/// the manifest's disk image, or streams it from the site, and keeps the
/// file only when the manifest's size and SHA-256 match. Every body is
/// counted as it arrives and the request is cancelled once it passes its
/// budget: `maxFeedAnswerSize` for a manifest or `download-url` answer,
/// the manifest's size for a disk image.
///
/// **Proposed routes, not confirmed.** The paths under
/// `{site}/cloud/api/v1/desktop/feed` and the answers read here (`404` for
/// no release, `403 {"error":"not_entitled"}`, `501` from `download-url`
/// for a site that only streams, `{url, expires_at}`) follow the Cloud
/// team's draft; they may change and are not a stable contract.
///
/// Every feed request fetches its headers first (`account.feed_headers`);
/// a `401` fetches them once more, which renews a rejected sign-in, and
/// tries once more. Feed requests follow no redirect, so the headers reach
/// only the site, and a signed URL is fetched without them.
public struct CloudFeed: ReleaseFeed {
    /// The site whose feed is read: the built-in site, never the signed-in
    /// account's; a session on another site gets no headers for it.
    public var site: String
    /// The compiled feed key, standard base64 (Resources/feed-public-key.txt).
    public var publicKey: String
    /// The profile id the manifest and its key must name.
    public var profileID: String
    /// The first part of this build's asset names, `Desktop-<profile id>`.
    public var assetPrefix: String
    /// The running version, which a release must be newer than.
    public var installedVersion: String
    /// The product name the sign-in and entitlement messages say.
    public var productName: String
    public var architecture: AppUpdateArchitecture
    public var macOSMajorVersion: Int
    public var session: URLSession
    /// The headers for one feed request to the given site; nil when the
    /// account cannot supply them (signed out, signed in on another site or
    /// as another account, or a core error).
    public var headers: (String) async -> [String: String]?
    /// The longest a `403` body is read for, however slowly it arrives, in
    /// seconds (gtk/src/updates/http.rs `REFUSAL_BODY_DEADLINE`).
    public var refusalBodyDeadline: TimeInterval = 10

    public init(
        site: String, publicKey: String, profileID: String, assetPrefix: String, installedVersion: String,
        productName: String, architecture: AppUpdateArchitecture, macOSMajorVersion: Int, session: URLSession,
        headers: @escaping (String) async -> [String: String]?
    ) {
        self.site = site
        self.publicKey = publicKey
        self.profileID = profileID
        self.assetPrefix = assetPrefix
        self.installedVersion = installedVersion
        self.productName = productName
        self.architecture = architecture
        self.macOSMajorVersion = macOSMajorVersion
        self.session = session
        self.headers = headers
    }

    /// The newest release for this Mac; nil when there is no newer one. A
    /// beta subscriber reads the stable and the beta manifest and gets the
    /// newer of the two, as the GitHub reader does; either failing fails
    /// the check, so a broken manifest is never passed over silently.
    public func latestRelease(includeBeta: Bool) async throws -> AppRelease? {
        // Without the compiled key nothing could be verified: no request.
        guard let key = ReleaseFeedVerifier.decodePublicKey(publicKey) else {
            throw ReleaseFeedFailure(AppUpdateMetadata.feedUnverifiedMessage)
        }
        var newest: FeedManifest?
        for channel in includeBeta ? [FeedChannel.stable, .beta] : [.stable] {
            guard let manifest = try await self.manifest(channel, key: key) else { continue }
            if newest.map({ AppUpdateMetadata.isNewer(manifest.version, than: $0.version) }) ?? true {
                newest = manifest
            }
        }
        guard let newest else { return nil }
        let asset: FeedAsset
        do {
            asset = try newest.diskImage(
                assetPrefix: assetPrefix, arch: architecture, macOSMajorVersion: macOSMajorVersion)
        } catch let error as FeedError {
            throw ReleaseFeedFailure(error.message)
        }
        guard let stream = endpoint("/assets/\(asset.id)") else {
            throw ReleaseFeedFailure(AppUpdateMetadata.unreadableMessage)
        }
        return AppRelease(
            version: newest.version, downloadURL: stream, checksumURL: nil,
            feed: AppReleaseFeedAsset(
                assetID: asset.id, name: asset.name, size: asset.size, sha256: asset.sha256,
                downloadHosts: newest.downloadHosts, page: AppUpdateMetadata.cloudDownloadsPage(site: site)))
    }

    /// The disk image of `release`: from the signed URL the site hands out
    /// when its host is one the manifest allows, else (`501`) streamed from
    /// the site; kept only when the manifest's size and SHA-256 match.
    public func diskImage(of release: AppRelease) async throws -> URL {
        guard let asset = release.feed else { throw ReleaseFeedFailure(AppUpdateMetadata.feedUnverifiedMessage) }
        guard asset.size <= AppUpdateMetadata.maxDownloadSize else {
            throw ReleaseFeedFailure(AppUpdateMetadata.tooLargeMessage)
        }
        guard let signing = endpoint("/assets/\(asset.assetID)/download-url"),
              let stream = endpoint("/assets/\(asset.assetID)") else { throw ReleaseFeedError.downloadFailed }
        let answer = try await authorized("POST", signing, download: nil, downloading: true)
        let file: URL
        switch answer.status {
        case 200:
            guard let text = Self.signedURL(answer.body),
                  feedDownloadHostAllowed(text, downloadHosts: asset.downloadHosts, site: site),
                  let url = URL(string: text) else {
                throw ReleaseFeedFailure(AppUpdateMetadata.feedUnverifiedMessage)
            }
            // The signed URL carries its own authority: none of the site's
            // headers go with it.
            let fetched: Answer
            do {
                fetched = try await send(Self.request("GET", url), download: asset.size)
            } catch FeedRefusal.tooLarge {
                throw ReleaseFeedFailure(AppUpdateMetadata.tooLargeMessage)
            } catch {
                throw ReleaseFeedError.downloadFailed
            }
            guard fetched.status == 200, let downloaded = fetched.file else { throw refusedDownload(fetched) }
            file = downloaded
        case 501:
            let fetched = try await authorized("GET", stream, download: asset.size, downloading: true)
            guard fetched.status == 200, let downloaded = fetched.file else { throw refusedDownload(fetched) }
            file = downloaded
        default:
            throw refusedDownload(answer)
        }
        do {
            let size = try FileManager.default.attributesOfItem(atPath: file.path)[.size] as? NSNumber
            guard size?.int64Value == asset.size,
                  try await AppUpdateMetadata.sha256(of: file) == asset.sha256 else {
                throw ReleaseFeedFailure(AppUpdateMetadata.unverifiedMessage)
            }
        } catch {
            try? FileManager.default.removeItem(at: file)
            throw error as? ReleaseFeedFailure ?? ReleaseFeedFailure(AppUpdateMetadata.unverifiedMessage)
        }
        return file
    }

    /// `channel`'s verified manifest; nil for none (`404`) or one that is
    /// not newer than the running version.
    private func manifest(_ channel: FeedChannel, key: Data) async throws -> FeedManifest? {
        guard let url = endpoint("/manifest?channel=\(channel.rawValue)") else {
            throw ReleaseFeedFailure(AppUpdateMetadata.unreadableMessage)
        }
        let answer = try await authorized("GET", url, download: nil, downloading: false)
        switch answer.status {
        case 200: break
        case 404: return nil
        default:
            throw ReleaseFeedFailure(
                Self.notEntitled(answer)
                    ? AppUpdateMetadata.notEntitledMessage(product: productName)
                    : AppUpdateMetadata.unreadableMessage)
        }
        let expected = FeedExpectation(profile: profileID, channel: channel, installedVersion: installedVersion)
        do {
            return try ReleaseFeedVerifier.verify(answer.body, key: key, expected: expected)
        } catch FeedError.notNewer {
            return nil
        } catch let error as FeedError {
            throw ReleaseFeedFailure(error.message)
        }
    }

    /// Sends `method url` with fresh headers; a `401` gets fresh headers and
    /// one more try, and a second `401` means signing in again. A request
    /// that gets no answer fails the check (`unreadable`) or the download.
    /// `download` is the size of the file a `200` answer is, as in `send`.
    private func authorized(_ method: String, _ url: URL, download: Int64?, downloading: Bool) async throws -> Answer {
        for _ in 0..<2 {
            guard let headers = await self.headers(site) else {
                throw ReleaseFeedFailure(AppUpdateMetadata.signInMessage(product: productName))
            }
            var request = Self.request(method, url)
            for (name, value) in headers { request.setValue(value, forHTTPHeaderField: name) }
            let answer: Answer
            do {
                answer = try await send(request, download: download)
            } catch FeedRefusal.tooLarge {
                throw ReleaseFeedFailure(AppUpdateMetadata.tooLargeMessage)
            } catch {
                if downloading { throw ReleaseFeedError.downloadFailed }
                throw ReleaseFeedFailure(AppUpdateMetadata.unreadableMessage)
            }
            if answer.status != 401 { return answer }
        }
        throw ReleaseFeedFailure(AppUpdateMetadata.signInMessage(product: productName))
    }

    /// An answer: its status, its body (a `403`'s first `maxRefusalBodySize`
    /// bytes, for `not_entitled`; none for any other refusal) and, for a
    /// `200` download, its file.
    private struct Answer {
        var status: Int
        var body: Data
        var file: URL?
    }

    private enum FeedRefusal: Error {
        case tooLarge
    }

    /// The most a `200` manifest or `download-url` answer may be; a signed
    /// manifest is a few kilobytes.
    static let maxFeedAnswerSize = 1_048_576
    /// The most of a refused answer that is kept.
    static let maxRefusalBodySize = 65_536

    /// Sends `request` without following any redirect. With `download`, the
    /// signed size of the file a `200` answer is, that answer goes to a file
    /// of at most that many bytes; any other `200` is read up to
    /// `maxFeedAnswerSize`. A body over its budget is refused as soon as it
    /// passes it, by the bytes received, whatever its `Content-Length`; a
    /// partial file is deleted. A refused answer's body is not read, so a
    /// `401` is retried and a `501` streamed as soon as it is answered,
    /// however slow its body; only a `403`'s is, for `not_entitled`.
    ///
    /// The body comes through `FeedTransfer`, the task's own delegate, not
    /// `URLSession.AsyncBytes`: bytes that arrive are handed over as they
    /// arrive, and the reader can end the body itself at a deadline.
    private func send(_ request: URLRequest, download: Int64?) async throws -> Answer {
        let transfer = FeedTransfer()
        let task = session.dataTask(with: request)
        task.delegate = transfer
        return try await withTaskCancellationHandler {
            task.resume()
            defer {
                task.cancel()
                transfer.end()
            }
            let response = try await transfer.response()
            let status = Self.status(response)
            guard status == 200 else {
                let body = status == 403 ? await Self.refusalBody(transfer, deadline: refusalBodyDeadline) : Data()
                return Answer(status: status, body: body, file: nil)
            }
            let limit = min(download ?? Int64(Self.maxFeedAnswerSize), AppUpdateMetadata.maxDownloadSize)
            guard response.expectedContentLength <= limit else { throw FeedRefusal.tooLarge }
            guard download != nil else {
                let body = try await Self.read(transfer.body, limit: Int(limit))
                return Answer(status: status, body: body, file: nil)
            }
            return Answer(status: status, body: Data(), file: try await Self.save(transfer.body, limit: limit))
        } onCancel: {
            task.cancel()
        }
    }

    /// `body` up to `limit` bytes, refused once it passes it.
    private static func read(_ body: AsyncThrowingStream<Data, Error>, limit: Int) async throws -> Data {
        var data = Data()
        for try await chunk in body {
            guard data.count + chunk.count <= limit else { throw FeedRefusal.tooLarge }
            data.append(chunk)
        }
        // A cancelled check ends the stream early: that is no whole body.
        try Task.checkCancellation()
        return data
    }

    /// What of a refused answer's body arrives within `deadline` seconds, up
    /// to `maxRefusalBodySize` bytes. The body is only informative: one that
    /// is slow, unfinished or fails keeps what arrived, and its status
    /// stands. The read races a timer: whichever ends first ends the other,
    /// the body is ended here and what arrived by then is kept, whether or
    /// not the read has woken; `send` then cancels the request.
    private static func refusalBody(_ transfer: FeedTransfer, deadline: TimeInterval) async -> Data {
        let received = RefusalBuffer(limit: maxRefusalBodySize)
        await withTaskGroup(of: Void.self) { group in
            group.addTask {
                do {
                    for try await chunk in transfer.body {
                        if received.append(chunk) { break }
                    }
                } catch {
                    // The body failed: keep what arrived.
                }
            }
            group.addTask {
                // Cancelled when the body ends first.
                try? await Task.sleep(nanoseconds: UInt64(max(deadline, 0) * 1_000_000_000))
            }
            _ = await group.next()
            transfer.end()
            group.cancelAll()
        }
        return received.data
    }

    /// `body` in a new temporary file, refused (and the file deleted) once
    /// it passes `limit` bytes.
    private static func save(_ body: AsyncThrowingStream<Data, Error>, limit: Int64) async throws -> URL {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-relay-feed-\(UUID().uuidString).download")
        guard FileManager.default.createFile(atPath: file.path, contents: nil) else {
            throw ReleaseFeedError.downloadFailed
        }
        do {
            let handle = try FileHandle(forWritingTo: file)
            defer { try? handle.close() }
            var received: Int64 = 0
            for try await chunk in body {
                received += Int64(chunk.count)
                guard received <= limit else { throw FeedRefusal.tooLarge }
                try handle.write(contentsOf: chunk)
            }
            // A cancelled download ends the stream early: that is no whole file.
            try Task.checkCancellation()
            return file
        } catch {
            try? FileManager.default.removeItem(at: file)
            throw error
        }
    }

    /// The answer's status; 0 for one that is not HTTP. A refused redirect
    /// is its own `3xx`.
    private static func status(_ response: URLResponse) -> Int {
        (response as? HTTPURLResponse)?.statusCode ?? 0
    }

    private static func request(_ method: String, _ url: URL) -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        // No stored cookie or credential goes with a feed request.
        request.httpShouldHandleCookies = false
        request.cachePolicy = .reloadIgnoringLocalCacheData
        return request
    }

    private func endpoint(_ path: String) -> URL? {
        AppUpdateMetadata.cloudFeedEndpoint(site: site, path: path)
    }

    private static func notEntitled(_ answer: Answer) -> Bool {
        guard answer.status == 403,
              let body = try? JSONSerialization.jsonObject(with: answer.body) as? [String: Any] else { return false }
        return body["error"] as? String == "not_entitled"
    }

    /// An answer that is not the one asked for: not entitled, or a failed
    /// download.
    private func refusedDownload(_ answer: Answer) -> Error {
        Self.notEntitled(answer)
            ? ReleaseFeedFailure(AppUpdateMetadata.notEntitledMessage(product: productName))
            : ReleaseFeedError.downloadFailed
    }

    /// The `url` of a `download-url` answer `{url, expires_at}`. The expiry
    /// is the site's to enforce; this clock is not trusted for it.
    private static func signedURL(_ body: Data) -> String? {
        guard let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { return nil }
        return object["url"] as? String
    }
}

/// The part of a refused answer's body received so far, up to `limit`
/// bytes, readable while the read that fills it is still waiting.
private final class RefusalBuffer: @unchecked Sendable {
    private let limit: Int
    private let lock = NSLock()
    private var received = Data()

    init(limit: Int) {
        self.limit = limit
    }

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return received
    }

    /// Keeps what of `chunk` fits; true once `limit` bytes are kept.
    func append(_ chunk: Data) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        received.append(chunk.prefix(max(limit - received.count, 0)))
        return received.count >= limit
    }
}

/// One feed request's answer as it arrives, as the task's own delegate:
/// its response, then its body in chunks as they are received. Every
/// redirect is refused, so a feed request's headers reach only the site and
/// a signed URL is fetched from the host that was checked; a refused
/// redirect is answered as its own `3xx`. `end()` ends the body for its
/// reader, keeping the chunks already received.
private final class FeedTransfer: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    let body: AsyncThrowingStream<Data, Error>
    private let sink: AsyncThrowingStream<Data, Error>.Continuation
    private let lock = NSLock()
    private var outcome: Result<URLResponse, Error>?
    private var waiting: CheckedContinuation<URLResponse, Error>?

    override init() {
        var sink: AsyncThrowingStream<Data, Error>.Continuation?
        body = AsyncThrowingStream(bufferingPolicy: .unbounded) { sink = $0 }
        self.sink = sink!
        super.init()
    }

    /// The response, or the error that came instead of one.
    func response() async throws -> URLResponse {
        try await withCheckedThrowingContinuation { continuation in
            lock.lock()
            if let outcome {
                lock.unlock()
                continuation.resume(with: outcome)
            } else {
                waiting = continuation
                lock.unlock()
            }
        }
    }

    func end() {
        sink.finish()
    }

    private func settle(_ result: Result<URLResponse, Error>) {
        lock.lock()
        guard outcome == nil else { return lock.unlock() }
        outcome = result
        let waiting = self.waiting
        self.waiting = nil
        lock.unlock()
        waiting?.resume(with: result)
    }

    func urlSession(
        _ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }

    func urlSession(
        _ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
        completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
    ) {
        settle(.success(response))
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        sink.yield(data)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        settle(.failure(error ?? URLError(.badServerResponse)))
        if let error {
            sink.finish(throwing: error)
        } else {
            sink.finish()
        }
    }
}
