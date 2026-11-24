import XCTest
import UIKit
@testable import AgentRelayiPad

@MainActor
final class AttachmentImagesTests: XCTestCase {
    private let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGNkYPjPwMDAxMDAwMDAAAALHwEDmIWXfgAAAABJRU5ErkJggg==")!
    private let url = URL(string: "https://files.example.test/shot.png")!

    private final class Calls {
        var resolve = 0
        var fetch = 0
    }

    private func picture(_ id: String = "file_1", type: String = "image/png", size: Int = 68) -> RelayAttachment {
        RelayAttachment(fileID: id, filename: "shot.png", contentType: type, sizeBytes: size)
    }

    /// A store whose fetch gives `downloads` in turn (the last one again
    /// once they run out).
    private func store(_ calls: Calls, _ downloads: AttachmentImageStore.Download...) -> AttachmentImageStore {
        AttachmentImageStore { _, _ in
            calls.fetch += 1
            return downloads[min(calls.fetch, downloads.count) - 1]
        }
    }

    private func store(_ calls: Calls, data: Data) -> AttachmentImageStore {
        store(calls, .data(data))
    }

    private func image(_ load: AttachmentImageStore.Load) -> UIImage? {
        if case .image(let image) = load { return image }
        return nil
    }

    private func isFailed(_ load: AttachmentImageStore.Load) -> Bool {
        if case .failed = load { return true }
        return false
    }

    private func isDropped(_ load: AttachmentImageStore.Load) -> Bool {
        if case .dropped = load { return true }
        return false
    }

    private func resolver(_ calls: Calls) -> AttachmentImageStore.Resolve {
        let url = url
        return { _ in
            calls.resolve += 1
            return url
        }
    }

    // MARK: Picture or button

    func testShowsPictureOnlyForImagesWithinTheCap() {
        XCTAssertTrue(AttachmentPreview.showsPicture(picture()))
        XCTAssertTrue(AttachmentPreview.showsPicture(picture(type: "IMAGE/JPEG")))
        XCTAssertTrue(AttachmentPreview.showsPicture(picture(size: AttachmentPreview.maximumBytes)))
        XCTAssertFalse(AttachmentPreview.showsPicture(picture(type: "application/pdf")))
        XCTAssertFalse(AttachmentPreview.showsPicture(picture(type: "text/plain")))
        XCTAssertFalse(AttachmentPreview.showsPicture(picture(size: AttachmentPreview.maximumBytes + 1)))
        XCTAssertFalse(AttachmentPreview.showsPicture(picture(size: -1)))
        XCTAssertFalse(AttachmentPreview.showsPicture(picture("")))
    }

    func testDecodeMakesAPictureAndRefusesOtherData() {
        XCTAssertEqual(AttachmentPreview.decode(png)?.size.width, 2)
        XCTAssertNil(AttachmentPreview.decode(Data("not an image".utf8)))
        XCTAssertNil(AttachmentPreview.decode(Data()))
    }

    // MARK: Loader

    func testOverCapFileIsRefusedBeforeDownloading() async {
        let calls = Calls()
        let store = self.store(calls, data: png)
        let attachment = picture(size: AttachmentPreview.maximumBytes + 1)
        let result = await store.load(attachment, resolve: resolver(calls))
        XCTAssertTrue(isFailed(result))
        XCTAssertEqual(calls.resolve, 0)
        XCTAssertEqual(calls.fetch, 0)
        XCTAssertTrue(store.showsButton(attachment))
    }

    func testFetchIsAskedForTheCap() async {
        var limit: Int?
        let store = AttachmentImageStore { [png] _, maximumBytes in
            limit = maximumBytes
            return .data(png)
        }
        _ = await store.load(picture(), resolve: resolver(Calls()))
        XCTAssertEqual(limit, AttachmentPreview.maximumBytes)
    }

    // What a retry cannot change sticks for the session.

    func testOverCapResponseSticks() async {
        let calls = Calls()
        let store = self.store(calls, data: Data(repeating: 0, count: AttachmentPreview.maximumBytes + 1))
        let result = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isFailed(result))
        XCTAssertTrue(store.showsButton(picture()))
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 1)
    }

    func testRefusedDownloadSticks() async {
        let calls = Calls()
        let store = self.store(calls, .refused)
        let result = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isFailed(result))
        XCTAssertTrue(store.showsButton(picture()))
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 1)
    }

    func testUndecodableFileSticks() async {
        let calls = Calls()
        let store = self.store(calls, data: Data("not an image".utf8))
        let result = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isFailed(result))
        XCTAssertTrue(store.showsButton(picture()))
        XCTAssertNil(store.cached(picture()))
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 1)
    }

    // A transport, server or resolve error does not stick: the next try loads.

    func testUnavailableDownloadDoesNotStick() async {
        let calls = Calls()
        let store = self.store(calls, .unavailable, .data(png))
        let first = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isFailed(first))
        XCTAssertFalse(store.showsButton(picture()))
        let second = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(image(second)?.size.width, 2)
        XCTAssertEqual(calls.fetch, 2)
    }

    func testThrownFetchErrorDoesNotStick() async {
        let calls = Calls()
        let store = AttachmentImageStore { [png] _, _ in
            calls.fetch += 1
            if calls.fetch == 1 { throw URLError(.notConnectedToInternet) }
            return .data(png)
        }
        let first = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isFailed(first))
        XCTAssertFalse(store.showsButton(picture()))
        let second = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNotNil(image(second))
    }

    func testFailedResolveDoesNotStick() async {
        let calls = Calls()
        let store = self.store(calls, data: png)
        for error in [URLError(.timedOut), URLError(.badServerResponse)] {
            let result = await store.load(picture()) { _ in throw error }
            XCTAssertTrue(isFailed(result))
            XCTAssertFalse(store.showsButton(picture()))
        }
        XCTAssertEqual(calls.fetch, 0)
        let loaded = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNotNil(image(loaded))
    }

    // A cancelled load is dropped and never recorded, however it surfaces.

    func testCancelledTaskIsNotRecorded() async {
        let calls = Calls()
        let store = self.store(calls, .refused)
        let attachment = picture()
        let resolve = resolver(calls)
        let task = Task { @MainActor in await store.load(attachment, resolve: resolve) }
        task.cancel()
        let result = await task.value
        XCTAssertTrue(isDropped(result))
        XCTAssertFalse(store.showsButton(attachment))
    }

    func testTaskCancelledDuringFetchIsNotRecorded() async {
        let calls = Calls()
        // The fetch sees its task cancelled and gives what would otherwise stick.
        let store = AttachmentImageStore { _, _ in
            calls.fetch += 1
            withUnsafeCurrentTask { $0?.cancel() }
            return .refused
        }
        let attachment = picture()
        let resolve = resolver(calls)
        let task = Task { @MainActor in await store.load(attachment, resolve: resolve) }
        let result = await task.value
        XCTAssertEqual(calls.fetch, 1)
        XCTAssertTrue(isDropped(result))
        XCTAssertFalse(store.showsButton(attachment))
    }

    func testCancellationErrorsAreNotRecorded() async {
        let errors: [Error] = [CancellationError(), URLError(.cancelled)]
        for error in errors {
            let calls = Calls()
            let fromResolve = self.store(calls, .refused)
            let resolved = await fromResolve.load(picture()) { _ in throw error }
            XCTAssertTrue(isDropped(resolved), "\(error)")
            XCTAssertFalse(fromResolve.showsButton(picture()))

            let fromFetch = AttachmentImageStore { _, _ in throw error }
            let fetched = await fromFetch.load(picture(), resolve: resolver(calls))
            XCTAssertTrue(isDropped(fetched), "\(error)")
            XCTAssertFalse(fromFetch.showsButton(picture()))
        }
    }

    // A load dropped by clear() is not a failure.

    func testClearDuringAFetchDropsTheLoad() async {
        let calls = Calls()
        var store: AttachmentImageStore!
        store = AttachmentImageStore { [png] _, _ in
            calls.fetch += 1
            if calls.fetch == 1 { await MainActor.run { store.clear() } }
            return .data(png)
        }
        let first = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isDropped(first))
        XCTAssertNil(store.cached(picture()))
        XCTAssertFalse(store.showsButton(picture()))
        let second = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNotNil(image(second))
    }

    func testClearDuringAFetchThatWouldStickDropsTheLoad() async {
        let calls = Calls()
        var store: AttachmentImageStore!
        store = AttachmentImageStore { _, _ in
            calls.fetch += 1
            await MainActor.run { store.clear() }
            return .refused
        }
        let result = await store.load(picture(), resolve: resolver(calls))
        XCTAssertTrue(isDropped(result))
        XCTAssertFalse(store.showsButton(picture()))
    }

    func testClearDuringResolveDropsTheLoad() async {
        let calls = Calls()
        let store = self.store(calls, data: png)
        let url = url
        let result = await store.load(picture()) { _ in
            store.clear()
            return url
        }
        XCTAssertTrue(isDropped(result))
        XCTAssertEqual(calls.fetch, 0)
        XCTAssertFalse(store.showsButton(picture()))
    }

    func testSecondLoadIsACacheHit() async {
        let calls = Calls()
        let store = self.store(calls, data: png)
        let first = image(await store.load(picture(), resolve: resolver(calls)))
        let second = image(await store.load(picture(), resolve: resolver(calls)))
        XCTAssertEqual(first?.size.width, 2)
        XCTAssertTrue(first === second)
        XCTAssertTrue(store.cached(picture()) === first)
        XCTAssertEqual(calls.resolve, 1)
        XCTAssertEqual(calls.fetch, 1)
    }

    func testClearEmptiesTheCacheAndTheFailures() async {
        let calls = Calls()
        let store = AttachmentImageStore { [png] url, _ in
            calls.fetch += 1
            return url.lastPathComponent == "bad.png" ? .refused : .data(png)
        }
        let bad = URL(string: "https://files.example.test/bad.png")!
        _ = await store.load(picture(), resolve: resolver(calls))
        _ = await store.load(picture("file_2"), resolve: { _ in bad })
        XCTAssertNotNil(store.cached(picture()))
        XCTAssertTrue(store.showsButton(picture("file_2")))

        store.clear()

        XCTAssertNil(store.cached(picture()))
        XCTAssertFalse(store.showsButton(picture("file_2")))
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 3)
    }

    func testClientClearsPicturesWhenTheSessionChanges() async {
        let client = RelayClient()
        let attachment = picture()
        // A plain-http URL is refused before any network, and that sticks.
        let plain = URL(string: "http://files.example.test/shot.png")!
        _ = await client.attachmentImages.load(attachment) { _ in plain }
        XCTAssertTrue(client.attachmentImages.showsButton(attachment))
        client.advanceSessionGeneration()
        XCTAssertFalse(client.attachmentImages.showsButton(attachment))
    }

    // MARK: Download

    private func download(_ path: String) async -> (data: Data?, exceededLimit: Bool) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [AttachmentFileURLProtocol.self]
        let request = PersonPhotoDownload(maximumBytes: 1_024, requiresImageType: false)
        let data = await request.data(from: URL(string: "https://files.example.test/" + path)!,
                                      configuration: configuration)
        return (data, request.exceededLimit)
    }

    func testDownloadRefusesABodyOverTheLimitOfAnyType() async {
        let small = await download("small")
        XCTAssertEqual(small.data?.count, 512)
        XCTAssertFalse(small.exceededLimit)
        for path in ["large", "declared"] {
            let refused = await download(path)
            XCTAssertNil(refused.data, path)
            XCTAssertTrue(refused.exceededLimit, path)
        }
    }

    func testServerErrorIsNotOverTheLimit() async {
        let failed = await download("unavailable")
        XCTAssertNil(failed.data)
        XCTAssertFalse(failed.exceededLimit)
    }

    func testDownloadRefusesPlainHTTP() async {
        let result = await AttachmentImageStore.download(URL(string: "http://files.example.test/small")!,
                                                         maximumBytes: 1_024)
        guard case .refused = result else { return XCTFail("plain http must be refused") }
    }
}

private final class AttachmentFileURLProtocol: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "files.example.test" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url else { return }
        var headers = ["Content-Type": "application/octet-stream"]
        var body = Data(repeating: 0, count: 512)
        var status = 200
        switch url.lastPathComponent {
        case "large": body = Data(repeating: 0, count: 2_048)
        case "declared": headers["Content-Length"] = "4096"
        case "unavailable":
            status = 503
            headers["Content-Length"] = "4096"
        default: break
        }
        let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
