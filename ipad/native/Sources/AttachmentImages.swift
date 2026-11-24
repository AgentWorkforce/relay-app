import Foundation
import ImageIO
import UIKit

/// Which posted files the chat draws as a picture, and how large.
enum AttachmentPreview {
    /// Files larger than this are never downloaded for a picture: the
    /// declared size is checked before the download and the received size
    /// during it. They keep the download button.
    static let maximumBytes = 20 * 1_024 * 1_024
    /// The longest side, in pixels, of the decoded picture. Covers the
    /// tallest drawn picture (`maximumHeight`) at 3x and a bubble's width.
    static let maximumPixelSize = 960
    /// The tallest a picture is drawn in a bubble, in points.
    static let maximumHeight: CGFloat = 240

    static func isImage(_ contentType: String) -> Bool {
        contentType.lowercased().hasPrefix("image/")
    }

    /// A picture rather than the download button: an image type, a file id
    /// and a declared size within `maximumBytes`.
    static func showsPicture(_ attachment: RelayAttachment) -> Bool {
        isImage(attachment.contentType) && !attachment.id.isEmpty
            && attachment.sizeBytes >= 0 && attachment.sizeBytes <= maximumBytes
    }

    /// A downsampled picture of `data`, or nil when the system cannot decode
    /// it. Runs off the main actor when awaited from it.
    static func thumbnail(_ data: Data) async -> UIImage? {
        decode(data)
    }

    static func decode(_ data: Data) -> UIImage? {
        guard data.count <= maximumBytes,
              let source = CGImageSourceCreateWithData(data as CFData, [
                kCGImageSourceShouldCache: false,
              ] as CFDictionary),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: maximumPixelSize,
              ] as CFDictionary) else { return nil }
        return UIImage(cgImage: image)
    }
}

/// Loads posted pictures and keeps them in memory by file id, for the
/// current account and workspace only: `clear()` (called when the
/// signed-in identity changes) empties the cache and drops every load
/// still running, so a picture of one account never shows in the next.
/// Nothing is written to disk.
final class AttachmentImageStore {
    /// Fetches `url` with no credentials, refusing a body over the byte limit.
    typealias Fetch = (URL, Int) async -> Data?
    /// The short-lived download URL of a file (`RelayClient.downloadURL`).
    typealias Resolve = @MainActor (RelayAttachment) async throws -> URL

    private let fetch: Fetch
    private let images = NSCache<NSString, UIImage>()
    /// Files whose picture could not be made this session: they show the
    /// download button straight away instead of loading again.
    private var failed: Set<String> = []
    /// Moves on every `clear()`; a load that started before it is dropped.
    private var generation = 0

    init(fetch: @escaping Fetch = AttachmentImageStore.download) {
        self.fetch = fetch
        images.countLimit = 40
        images.totalCostLimit = 48 * 1_024 * 1_024
    }

    @MainActor func cached(_ attachment: RelayAttachment) -> UIImage? {
        images.object(forKey: attachment.id as NSString)
    }

    /// Whether the file shows the download button rather than a picture:
    /// not a picture by `AttachmentPreview.showsPicture`, or its load failed.
    @MainActor func showsButton(_ attachment: RelayAttachment) -> Bool {
        !AttachmentPreview.showsPicture(attachment) || failed.contains(attachment.id)
    }

    /// The picture of `attachment`, from the cache or loaded now; nil when
    /// it shows the button, the load fails or was canceled, or `clear()`
    /// ran meanwhile.
    @MainActor func load(_ attachment: RelayAttachment, resolve: Resolve) async -> UIImage? {
        guard !showsButton(attachment) else { return nil }
        if let image = cached(attachment) { return image }
        let generation = self.generation
        let url: URL
        do {
            url = try await resolve(attachment)
        } catch {
            if !(error is CancellationError) { markFailed(attachment, generation) }
            return nil
        }
        guard generation == self.generation, !Task.isCancelled else { return nil }
        let data = await fetch(url, AttachmentPreview.maximumBytes)
        guard generation == self.generation else { return nil }
        guard let data, data.count <= AttachmentPreview.maximumBytes else {
            // A canceled fetch also returns nil; that is not a failure.
            if !Task.isCancelled { markFailed(attachment, generation) }
            return nil
        }
        let image = await AttachmentPreview.thumbnail(data)
        guard generation == self.generation else { return nil }
        guard let image else {
            markFailed(attachment, generation)
            return nil
        }
        let cost = image.cgImage.map { $0.bytesPerRow * $0.height } ?? 0
        images.setObject(image, forKey: attachment.id as NSString, cost: cost)
        return image
    }

    /// Forgets every picture and failure and drops loads still running.
    @MainActor func clear() {
        generation += 1
        images.removeAllObjects()
        failed = []
    }

    @MainActor private func markFailed(_ attachment: RelayAttachment, _ generation: Int) {
        guard generation == self.generation else { return }
        failed.insert(attachment.id)
    }

    /// The download with an ephemeral session: no cookies, credentials,
    /// URL cache or extra headers, and the byte limit enforced on the
    /// declared and the received size.
    static func download(_ url: URL, maximumBytes: Int) async -> Data? {
        guard !Task.isCancelled, url.scheme?.lowercased() == "https" else { return nil }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpAdditionalHeaders = nil
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = 20
        configuration.timeoutIntervalForResource = 60
        return await PersonPhotoDownload(maximumBytes: maximumBytes, requiresImageType: false)
            .data(from: url, configuration: configuration)
    }
}
