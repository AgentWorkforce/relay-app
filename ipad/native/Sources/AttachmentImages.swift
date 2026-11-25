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
    /// What a download of a posted file gave.
    enum Download {
        case data(Data)
        /// It can never give a picture: the file is over the byte limit
        /// (declared or received) or the URL is refused.
        case refused
        /// Nothing this time (network, server, timeout or cancellation);
        /// a later try may work.
        case unavailable
    }

    /// What `load` gave.
    enum Load {
        case image(UIImage)
        /// No picture this time: the row shows the download button. Only
        /// a failure that a retry cannot change is remembered (`failed`).
        case failed
        /// Canceled, or `clear()` ran while it loaded: not a failure, and
        /// the row may load again.
        case dropped
    }

    /// Fetches `url` with no credentials, refusing a body over the byte
    /// limit. A thrown error counts as `.unavailable`.
    typealias Fetch = (URL, Int) async throws -> Download
    /// The short-lived download URL of a file (`RelayClient.downloadURL`).
    typealias Resolve = @MainActor (RelayAttachment) async throws -> URL

    private let fetch: Fetch
    private let images = NSCache<NSString, UIImage>()
    /// Files whose picture can never be made, whatever a retry does: the
    /// data is not an image, the file is over the byte limit, or its URL
    /// is refused. They show the download button without loading again
    /// until `clear()`. Resolve, network, server and timeout errors and
    /// cancellations are not recorded: the row shows the button for that
    /// try and loads again the next time it appears.
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
    /// not a picture by `AttachmentPreview.showsPicture`, or its picture
    /// can never be made (`failed`).
    @MainActor func showsButton(_ attachment: RelayAttachment) -> Bool {
        !AttachmentPreview.showsPicture(attachment) || failed.contains(attachment.id)
    }

    /// The picture of `attachment`, from the cache or loaded now.
    @MainActor func load(_ attachment: RelayAttachment, resolve: Resolve) async -> Load {
        guard !showsButton(attachment) else { return .failed }
        if let image = cached(attachment) { return .image(image) }
        let generation = self.generation
        let url: URL
        do {
            url = try await resolve(attachment)
        } catch {
            return interrupted(generation, error) ? .dropped : .failed
        }
        if interrupted(generation) { return .dropped }
        let download: Download
        do {
            download = try await fetch(url, AttachmentPreview.maximumBytes)
        } catch {
            return interrupted(generation, error) ? .dropped : .failed
        }
        if interrupted(generation) { return .dropped }
        let data: Data
        switch download {
        case .data(let received) where received.count <= AttachmentPreview.maximumBytes:
            data = received
        case .data, .refused:
            markFailed(attachment)
            return .failed
        case .unavailable:
            return .failed
        }
        let image = await AttachmentPreview.thumbnail(data)
        if interrupted(generation) { return .dropped }
        guard let image else {
            markFailed(attachment)
            return .failed
        }
        let cost = image.cgImage.map { $0.bytesPerRow * $0.height } ?? 0
        images.setObject(image, forKey: attachment.id as NSString, cost: cost)
        return .image(image)
    }

    /// Forgets every picture and failure and drops loads still running.
    @MainActor func clear() {
        generation += 1
        images.removeAllObjects()
        failed = []
    }

    /// Whether a load that started at `generation` must stop without a
    /// result: `clear()` ran, the task was canceled, or `error` is a
    /// cancellation (Swift's or URLSession's).
    @MainActor private func interrupted(_ generation: Int, _ error: Error? = nil) -> Bool {
        if generation != self.generation || Task.isCancelled { return true }
        guard let error else { return false }
        return error is CancellationError || (error as? URLError)?.code == .cancelled
    }

    /// Called only after `interrupted` said no, so a canceled or dropped
    /// load is never recorded.
    @MainActor private func markFailed(_ attachment: RelayAttachment) {
        failed.insert(attachment.id)
    }

    /// The download with an ephemeral session: no cookies, credentials,
    /// URL cache or extra headers, and the byte limit enforced on the
    /// declared and the received size.
    static func download(_ url: URL, maximumBytes: Int) async -> Download {
        guard url.scheme?.lowercased() == "https" else { return .refused }
        guard !Task.isCancelled else { return .unavailable }
        let configuration = URLSessionConfiguration.ephemeral
        #if DEBUG
        if RelayUITestScreenshot.enabled {
            configuration.protocolClasses = [RelayUITestScreenshot.self]
        }
        #endif
        configuration.httpAdditionalHeaders = nil
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = 20
        configuration.timeoutIntervalForResource = 60
        let request = PersonPhotoDownload(maximumBytes: maximumBytes, requiresImageType: false)
        if let data = await request.data(from: url, configuration: configuration) { return .data(data) }
        return request.exceededLimit ? .refused : .unavailable
    }
}
