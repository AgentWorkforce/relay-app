import AppKit
import Observation

/// Every row observes one image per URL and joins the same pending download.
@MainActor
@Observable
final class PersonAvatarStore {
    static let shared = PersonAvatarStore { url in await fetchImage(url) }
    private(set) var images: [URL: NSImage] = [:]
    @ObservationIgnored private var requests: [URL: Task<Void, Never>] = [:]
    @ObservationIgnored private let fetch: (URL) async -> NSImage?

    init(fetch: @escaping (URL) async -> NSImage?) { self.fetch = fetch }

    func image(for url: URL?) -> NSImage? { url.flatMap { images[$0] } }

    func load(_ url: URL) async {
        guard url.scheme == "https", images[url] == nil else { return }
        if let pending = requests[url] { await pending.value; return }
        let pending = Task { @MainActor in
            defer { requests[url] = nil }
            for attempt in 0..<2 {
                if let image = await fetch(url) { images[url] = image; return }
                if attempt == 0 { try? await Task.sleep(nanoseconds: 250_000_000) }
            }
        }
        requests[url] = pending
        await pending.value
    }

    nonisolated private static func fetchImage(_ url: URL) async -> NSImage? {
        var request = URLRequest(url: url, timeoutInterval: 10)
        request.setValue("AgentRelayDesktop", forHTTPHeaderField: "User-Agent")
        do {
            let (bytes, response) = try await URLSession.shared.bytes(for: request)
            let limit = 2 * 1024 * 1024
            guard let response = response as? HTTPURLResponse,
                  (200..<300).contains(response.statusCode), response.url?.scheme == "https",
                  response.expectedContentLength <= Int64(limit) else { return nil }
            var data = Data()
            for try await byte in bytes {
                guard data.count < limit else { return nil }
                data.append(byte)
            }
            return NSImage(data: data)
        } catch { return nil }
    }
}
