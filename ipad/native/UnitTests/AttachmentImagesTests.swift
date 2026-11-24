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

    private func store(_ calls: Calls, data: Data?) -> AttachmentImageStore {
        AttachmentImageStore { _, _ in
            calls.fetch += 1
            return data
        }
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
        let store = store(calls, data: png)
        let attachment = picture(size: AttachmentPreview.maximumBytes + 1)
        let image = await store.load(attachment, resolve: resolver(calls))
        XCTAssertNil(image)
        XCTAssertEqual(calls.resolve, 0)
        XCTAssertEqual(calls.fetch, 0)
        XCTAssertTrue(store.showsButton(attachment))
    }

    func testFetchIsAskedForTheCap() async {
        var limit: Int?
        let store = AttachmentImageStore { [png] _, maximumBytes in
            limit = maximumBytes
            return png
        }
        _ = await store.load(picture(), resolve: resolver(Calls()))
        XCTAssertEqual(limit, AttachmentPreview.maximumBytes)
    }

    func testOverCapResponseFallsBackToTheButton() async {
        let calls = Calls()
        let store = store(calls, data: Data(repeating: 0, count: AttachmentPreview.maximumBytes + 1))
        let image = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNil(image)
        XCTAssertTrue(store.showsButton(picture()))
    }

    func testFailedDownloadFallsBackToTheButton() async {
        let calls = Calls()
        let store = store(calls, data: nil)
        XCTAssertFalse(store.showsButton(picture()))
        let image = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNil(image)
        XCTAssertTrue(store.showsButton(picture()))
        // It is not loaded again this session.
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 1)
    }

    func testFailedResolveFallsBackToTheButton() async {
        let calls = Calls()
        let store = store(calls, data: png)
        let image = await store.load(picture()) { _ in throw URLError(.badServerResponse) }
        XCTAssertNil(image)
        XCTAssertEqual(calls.fetch, 0)
        XCTAssertTrue(store.showsButton(picture()))
    }

    func testUndecodableFileFallsBackToTheButton() async {
        let calls = Calls()
        let store = store(calls, data: Data("not an image".utf8))
        let image = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNil(image)
        XCTAssertTrue(store.showsButton(picture()))
        XCTAssertNil(store.cached(picture()))
    }

    func testSecondLoadIsACacheHit() async {
        let calls = Calls()
        let store = store(calls, data: png)
        let first = await store.load(picture(), resolve: resolver(calls))
        let second = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(first?.size.width, 2)
        XCTAssertTrue(first === second)
        XCTAssertTrue(store.cached(picture()) === first)
        XCTAssertEqual(calls.resolve, 1)
        XCTAssertEqual(calls.fetch, 1)
    }

    func testClearEmptiesTheCacheAndTheFailures() async {
        let calls = Calls()
        let store = store(calls, data: png)
        _ = await store.load(picture(), resolve: resolver(calls))
        _ = await store.load(picture("file_2"), resolve: { _ in throw URLError(.timedOut) })
        XCTAssertNotNil(store.cached(picture()))
        XCTAssertTrue(store.showsButton(picture("file_2")))

        store.clear()

        XCTAssertNil(store.cached(picture()))
        XCTAssertFalse(store.showsButton(picture("file_2")))
        _ = await store.load(picture(), resolve: resolver(calls))
        XCTAssertEqual(calls.fetch, 2)
    }

    func testClearDuringALoadDropsIt() async {
        let calls = Calls()
        var store: AttachmentImageStore!
        store = AttachmentImageStore { [png] _, _ in
            calls.fetch += 1
            await MainActor.run { store.clear() }
            return png
        }
        let image = await store.load(picture(), resolve: resolver(calls))
        XCTAssertNil(image)
        XCTAssertNil(store.cached(picture()))
        // The dropped load is not counted as a failure of the new session.
        XCTAssertFalse(store.showsButton(picture()))
    }

    func testClearDuringResolveDropsTheLoad() async {
        let calls = Calls()
        let store = store(calls, data: png)
        let url = url
        let image = await store.load(picture()) { _ in
            store.clear()
            return url
        }
        XCTAssertNil(image)
        XCTAssertEqual(calls.fetch, 0)
        XCTAssertFalse(store.showsButton(picture()))
    }

    func testCancellationIsNotAFailure() async {
        let calls = Calls()
        let store = store(calls, data: nil)
        let attachment = picture()
        let resolve = resolver(calls)
        let task = Task { @MainActor in await store.load(attachment, resolve: resolve) }
        task.cancel()
        let image = await task.value
        XCTAssertNil(image)
        XCTAssertFalse(store.showsButton(attachment))

        let refused = await store.load(attachment) { _ in throw CancellationError() }
        XCTAssertNil(refused)
        XCTAssertFalse(store.showsButton(attachment))
    }

    func testClientClearsPicturesWhenTheSessionChanges() async {
        let client = RelayClient()
        let attachment = picture()
        _ = await client.attachmentImages.load(attachment) { _ in throw URLError(.timedOut) }
        XCTAssertTrue(client.attachmentImages.showsButton(attachment))
        client.advanceSessionGeneration()
        XCTAssertFalse(client.attachmentImages.showsButton(attachment))
    }

    // MARK: Download

    func testDownloadRefusesABodyOverTheLimitOfAnyType() async {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [AttachmentFileURLProtocol.self]
        let small = await PersonPhotoDownload(maximumBytes: 1_024, requiresImageType: false)
            .data(from: URL(string: "https://files.example.test/small")!, configuration: configuration)
        XCTAssertEqual(small?.count, 512)
        let large = await PersonPhotoDownload(maximumBytes: 1_024, requiresImageType: false)
            .data(from: URL(string: "https://files.example.test/large")!, configuration: configuration)
        XCTAssertNil(large)
        let declared = await PersonPhotoDownload(maximumBytes: 1_024, requiresImageType: false)
            .data(from: URL(string: "https://files.example.test/declared")!, configuration: configuration)
        XCTAssertNil(declared)
    }

    func testDownloadRefusesPlainHTTP() async {
        let data = await AttachmentImageStore.download(URL(string: "http://files.example.test/small")!,
                                                       maximumBytes: 1_024)
        XCTAssertNil(data)
    }
}

private final class AttachmentFileURLProtocol: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "files.example.test" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url else { return }
        var headers = ["Content-Type": "application/octet-stream"]
        var body = Data(repeating: 0, count: 512)
        switch url.lastPathComponent {
        case "large": body = Data(repeating: 0, count: 2_048)
        case "declared": headers["Content-Length"] = "4096"
        default: break
        }
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
