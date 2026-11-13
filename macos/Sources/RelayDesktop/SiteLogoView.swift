import AppKit
import SwiftUI

/// A self-host site's logo for the sign-in screen: 22 points tall and at
/// most 160 wide, nothing while it loads or when it cannot. The GTK app's
/// `sign_in_methods::logo` matches.
struct SiteLogoView: View {
    let url: String
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fit)
                    .zoomFrame(maxWidth: 160, maxHeight: 22)
                    .accessibilityHidden(true)
            }
        }
        .task(id: url) { image = await SiteLogo.load(url) }
    }
}

enum SiteLogo {
    /// A logo is at most 1 MiB and loads within 10 seconds.
    static let limit = 1024 * 1024
    static let deadline: TimeInterval = 10

    /// Its own session, so the 10 seconds bound the whole load, body
    /// included: a request's timeout only bounds the wait between packets,
    /// and a server that drips bytes would keep that alive.
    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = deadline
        configuration.timeoutIntervalForResource = deadline
        return URLSession(configuration: configuration)
    }()

    /// The logo: an `https` answer without redirects that fits the limit
    /// and arrives within the deadline.
    nonisolated static func load(_ string: String) async -> NSImage? {
        guard let url = URL(string: string), url.scheme == "https" else { return nil }
        var request = URLRequest(url: url)
        request.setValue("AgentRelayDesktop", forHTTPHeaderField: "User-Agent")
        do {
            let (bytes, response) = try await session.bytes(for: request, delegate: RefuseRedirects())
            guard let response = response as? HTTPURLResponse,
                  accepts(status: response.statusCode, expectedLength: response.expectedContentLength),
                  let data = try await collect(bytes) else { return nil }
            return NSImage(data: data)
        } catch { return nil }
    }

    /// Whether an answer may be read: a 200 that does not announce more than
    /// the limit. A redirect is handed back unfollowed, so it is refused here.
    nonisolated static func accepts(status: Int, expectedLength: Int64) -> Bool {
        status == 200 && expectedLength <= Int64(limit)
    }

    /// The bytes, or nothing once they pass the limit.
    nonisolated static func collect<Bytes: AsyncSequence>(_ bytes: Bytes) async throws -> Data? where Bytes.Element == UInt8 {
        var data = Data()
        for try await byte in bytes {
            guard data.count < limit else { return nil }
            data.append(byte)
        }
        return data
    }
}

/// Answers a redirect with the redirect itself, which `load` then refuses.
private final class RefuseRedirects: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}
