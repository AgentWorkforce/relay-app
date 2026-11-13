import CryptoKit
import Foundation

public enum AppUpdateArchitecture: String, Sendable {
    case arm64
    case x64

    public static var current: AppUpdateArchitecture {
        #if arch(arm64)
        return .arm64
        #else
        return .x64
        #endif
    }
}

public struct AppRelease: Codable, Equatable, Sendable {
    public let version: String
    /// The disk image: GitHub's asset, or a cloud feed's streamed download
    /// (`{site}/cloud/api/v1/desktop/feed/assets/{id}`), for display.
    public let downloadURL: URL
    /// GitHub's published checksum; nil for a cloud feed's release, whose
    /// signed manifest carries the digest (`feed`).
    public let checksumURL: URL?
    /// What the verified manifest says of the disk image, for a cloud
    /// feed's release; nil for GitHub's.
    public let feed: AppReleaseFeedAsset?

    public init(version: String, downloadURL: URL, checksumURL: URL?, feed: AppReleaseFeedAsset? = nil) {
        self.version = version
        self.downloadURL = downloadURL
        self.checksumURL = checksumURL
        self.feed = feed
    }
}

/// The disk image a verified cloud manifest names, kept with the release
/// so a download after a relaunch is held to the same size, digest and
/// download hosts.
public struct AppReleaseFeedAsset: Codable, Equatable, Sendable {
    /// The id the download endpoints take.
    public let assetID: String
    public let name: String
    /// The exact size in bytes.
    public let size: Int64
    /// Lower-case hex SHA-256 of the bytes.
    public let sha256: String
    /// The only hosts a signed download URL may name.
    public let downloadHosts: [String]
    /// The site's downloads page (`{site}/cloud/dashboard/downloads`).
    public let page: URL?

    public init(assetID: String, name: String, size: Int64, sha256: String, downloadHosts: [String], page: URL?) {
        self.assetID = assetID
        self.name = name
        self.size = size
        self.sha256 = sha256
        self.downloadHosts = downloadHosts
        self.page = page
    }
}

public enum AppUpdateMetadataError: Error, Equatable {
    case invalidResponse
    case invalidVersion
    case missingAsset(String)
    case insecureAssetURL
    /// The release ships only for macOS versions newer than this Mac's.
    case requiresNewerMacOS(Int)
}

/// Parses the public GitHub release used by both the website download links and
/// the desktop updater. Keeping this logic in Core makes release selection and
/// version comparison testable without launching AppKit.
///
/// A release names each disk image for the oldest macOS it runs on,
/// `AgentRelay-<version>-macOS<floor>-<arch>.dmg`. Releases from before floors
/// existed carry only `AgentRelay-<version>-macOS-<arch>.dmg`, which runs on
/// every macOS the updater does.
public enum AppUpdateMetadata {
    public static var currentMacOSMajorVersion: Int {
        ProcessInfo.processInfo.operatingSystemVersion.majorVersion
    }

    /// The GitHub repository (`OWNER/NAME`) whose releases a build checks
    /// unless `build-app.sh --releases-repository` named another when the
    /// build was made.
    public static let defaultReleasesRepository = "AgentWorkforce/relay-desktop-releases"
    /// The first part of every release asset's name: `AgentRelay`, or a
    /// site profile's `Desktop-<id>` (docs/workspace-layout.md, "Site
    /// profiles"); the profile's Mac app reads its own from the bundle.
    public static let defaultAssetPrefix = "AgentRelay"
    /// The release feed of a build whose releases come from its site
    /// (`build-app.sh --release-feed cloud`) rather than a GitHub repository.
    public static let cloudReleaseFeed = "cloud"
    /// No disk image is larger: a release or manifest that says otherwise
    /// is refused before any download, as on Linux and Windows.
    public static let maxDownloadSize: Int64 = 512 * 1024 * 1024

    // What a check or download says. Linux and Windows say the same, word
    // for word (gtk/src/updates.rs and gtk/src/updates/cloud.rs).

    /// The release could not be read.
    public static let unreadableMessage =
        "Agent Relay could not read the latest release information. Try again later."
    /// The download's size or checksum did not match.
    public static let unverifiedMessage = "The downloaded update could not be verified and was discarded."
    /// A release or download over `maxDownloadSize`.
    public static let tooLargeMessage = "The download is larger than any Agent Relay release."
    /// The release has no disk image for this Mac.
    public static let noCompatibleUpdateMessage = "The latest release does not include a compatible update."
    /// The download failed.
    public static let downloadFailedMessage =
        "Agent Relay could not download the update. Check your connection and try again."
    /// A cloud manifest failed verification, or a download URL named a host
    /// the manifest does not allow; nothing is downloaded.
    public static let feedUnverifiedMessage = "The update could not be verified."

    /// The account is signed out, or its sign-in could not be renewed.
    public static func signInMessage(product: String) -> String {
        "Sign in to \(product) to receive updates."
    }

    /// The site answered `403 {"error":"not_entitled"}`.
    public static func notEntitledMessage(product: String) -> String {
        "This workspace is not entitled to updates for \(product)."
    }

    /// Every floored disk image needs a newer macOS than this Mac's.
    public static func requiresNewerMacOSMessage(floor: Int) -> String {
        "This update needs macOS \(floor) or newer. This Mac stays on the current version."
    }

    /// A cloud build's release page: its site's downloads.
    public static func cloudDownloadsPage(site: String) -> URL? {
        URL(string: trimmedSite(site) + "/cloud/dashboard/downloads")
    }

    /// The proposed cloud feed route `path` on `site`.
    public static func cloudFeedEndpoint(site: String, path: String) -> URL? {
        URL(string: trimmedSite(site) + "/cloud/api/v1/desktop/feed" + path)
    }

    private static func trimmedSite(_ site: String) -> String {
        var site = Substring(site)
        while site.hasSuffix("/") { site = site.dropLast() }
        return String(site)
    }

    public static func releasesURL(page: Int, repository: String = defaultReleasesRepository) -> URL {
        URL(string: "https://api.github.com/repos/\(repository)/releases?per_page=100&page=\(page)")!
    }

    public static func latestReleaseURL(repository: String = defaultReleasesRepository) -> URL {
        URL(string: "https://api.github.com/repos/\(repository)/releases/latest")!
    }

    public static func release(
        from data: Data,
        architecture: AppUpdateArchitecture,
        macOSMajorVersion: Int = currentMacOSMajorVersion,
        includeBeta: Bool = false,
        assetPrefix: String = defaultAssetPrefix
    ) throws -> AppRelease {
        let response: GitHubRelease
        do {
            response = try JSONDecoder().decode(GitHubRelease.self, from: data)
        } catch {
            throw AppUpdateMetadataError.invalidResponse
        }

        return try release(
            response, architecture: architecture, macOSMajorVersion: macOSMajorVersion, includeBeta: includeBeta,
            assetPrefix: assetPrefix)
    }

    /// Select by version, not publication order: a hotfix can be published after a beta.
    public static func newestRelease(
        from pages: [Data],
        architecture: AppUpdateArchitecture,
        macOSMajorVersion: Int = currentMacOSMajorVersion,
        assetPrefix: String = defaultAssetPrefix
    ) throws -> AppRelease {
        let releases: [GitHubRelease]
        do {
            releases = try pages.flatMap { try JSONDecoder().decode([GitHubRelease].self, from: $0) }
        } catch {
            throw AppUpdateMetadataError.invalidResponse
        }
        guard let newest = releases.filter({ $0.isEligible(includeBeta: true) }).max(by: {
            isNewer($1.version, than: $0.version)
        }) else { throw AppUpdateMetadataError.invalidResponse }
        return try release(
            newest, architecture: architecture, macOSMajorVersion: macOSMajorVersion, includeBeta: true,
            assetPrefix: assetPrefix)
    }

    public static func releaseCount(in data: Data) throws -> Int {
        try JSONDecoder().decode([GitHubRelease].self, from: data).count
    }

    private static func release(
        _ response: GitHubRelease,
        architecture: AppUpdateArchitecture,
        macOSMajorVersion: Int,
        includeBeta: Bool,
        assetPrefix: String
    ) throws -> AppRelease {
        guard response.isEligible(includeBeta: includeBeta) else {
            throw AppUpdateMetadataError.invalidResponse
        }

        let version = response.version
        guard AppVersion(version) != nil else {
            throw AppUpdateMetadataError.invalidVersion
        }

        let filename = try diskImageName(
            in: response.assets.map(\.name),
            version: version,
            architecture: architecture,
            macOSMajorVersion: macOSMajorVersion,
            assetPrefix: assetPrefix
        )
        guard let downloadURL = response.assets.first(where: { $0.name == filename })?.browserDownloadURL else {
            throw AppUpdateMetadataError.missingAsset(filename)
        }
        guard let checksumURL = response.assets.first(where: { $0.name == "\(filename).sha256" })?.browserDownloadURL else {
            throw AppUpdateMetadataError.missingAsset("\(filename).sha256")
        }
        guard downloadURL.scheme == "https", downloadURL.host == "github.com",
              checksumURL.scheme == "https", checksumURL.host == "github.com" else {
            throw AppUpdateMetadataError.insecureAssetURL
        }

        return AppRelease(version: version, downloadURL: downloadURL, checksumURL: checksumURL)
    }

    /// The highest-floored image this Mac can open, else the unfloored name;
    /// `requiresNewerMacOS` (the lowest floor) when the release has floored
    /// images this Mac cannot open and no unfloored one.
    private static func diskImageName(
        in names: [String], version: String, architecture: AppUpdateArchitecture, macOSMajorVersion: Int,
        assetPrefix: String
    ) throws -> String {
        let prefix = "\(assetPrefix)-\(version)-macOS"
        let suffix = "-\(architecture.rawValue).dmg"
        let floored: [(floor: Int, name: String)] = names.compactMap { name in
            guard name.hasPrefix(prefix), name.hasSuffix(suffix),
                  name.count > prefix.count + suffix.count else { return nil }
            let digits = name.dropFirst(prefix.count).dropLast(suffix.count)
            guard digits.allSatisfy({ $0.isASCII && $0.isNumber }), let floor = Int(digits) else { return nil }
            return (floor, name)
        }
        guard let lowest = floored.map(\.floor).min() else {
            return "\(prefix)\(suffix)"
        }
        if let chosen = floored.filter({ $0.floor <= macOSMajorVersion }).max(by: { $0.floor < $1.floor }) {
            return chosen.name
        }
        guard names.contains("\(prefix)\(suffix)") else {
            throw AppUpdateMetadataError.requiresNewerMacOS(lowest)
        }
        return "\(prefix)\(suffix)"
    }

    public static func isNewer(_ candidate: String, than installed: String) -> Bool {
        guard let candidateVersion = AppVersion(candidate),
              let installedVersion = AppVersion(installed) else { return false }
        return candidateVersion > installedVersion
    }

    /// GitHub checksum assets use the standard `<digest>  <filename>` format.
    public static func checksum(from data: Data) -> String? {
        guard let text = String(data: data, encoding: .utf8),
              let token = text.split(whereSeparator: \Character.isWhitespace).first else { return nil }
        let value = token.lowercased()
        guard value.count == 64, value.allSatisfy({ $0.isHexDigit }) else { return nil }
        return value
    }
}

/// A check or download refused with a sentence safe to show: a cloud
/// feed's sign-in, entitlement and verification answers.
public struct ReleaseFeedFailure: Error, Equatable, Sendable {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }
}

/// Why a GitHub release lookup or download failed.
public enum ReleaseFeedError: Error, Equatable, Sendable {
    case releaseLookupFailed
    case downloadFailed
    case invalidChecksum
    case checksumMismatch
}

/// Where a build's releases come from (Resources/release-feed.txt): a
/// GitHub repository's releases (`GitHubReleasesFeed`) or its site's
/// signed feed (`CloudFeed`).
public protocol ReleaseFeed {
    /// The newest release this Mac can install, on the stable channel and,
    /// with `includeBeta`, the beta one; nil when the feed has none newer.
    func latestRelease(includeBeta: Bool) async throws -> AppRelease?

    /// Downloads and verifies `release`'s disk image; the caller owns the
    /// file. Nothing is left behind when it throws.
    func diskImage(of release: AppRelease) async throws -> URL
}

/// A GitHub repository's releases, checked against each image's published
/// `.sha256`. Its assets must be on github.com.
public struct GitHubReleasesFeed: ReleaseFeed {
    public var repository: String
    public var assetPrefix: String
    public var currentVersion: String
    public var architecture: AppUpdateArchitecture
    public var macOSMajorVersion: Int
    public var session: URLSession

    public init(
        repository: String, assetPrefix: String, currentVersion: String, architecture: AppUpdateArchitecture,
        macOSMajorVersion: Int, session: URLSession
    ) {
        self.repository = repository
        self.assetPrefix = assetPrefix
        self.currentVersion = currentVersion
        self.architecture = architecture
        self.macOSMajorVersion = macOSMajorVersion
        self.session = session
    }

    /// The latest release; with betas, every page of releases, since the
    /// newest version need not be the latest published.
    public func latestRelease(includeBeta: Bool) async throws -> AppRelease? {
        guard includeBeta else {
            let data = try await releaseData(at: AppUpdateMetadata.latestReleaseURL(repository: repository))
            return try AppUpdateMetadata.release(
                from: data, architecture: architecture, macOSMajorVersion: macOSMajorVersion,
                assetPrefix: assetPrefix)
        }
        var pages: [Data] = []
        var page = 1
        while true {
            let data = try await releaseData(at: AppUpdateMetadata.releasesURL(page: page, repository: repository))
            pages.append(data)
            if try AppUpdateMetadata.releaseCount(in: data) < 100 { break }
            page += 1
        }
        return try AppUpdateMetadata.newestRelease(
            from: pages, architecture: architecture, macOSMajorVersion: macOSMajorVersion, assetPrefix: assetPrefix)
    }

    public func diskImage(of release: AppRelease) async throws -> URL {
        guard let checksumURL = release.checksumURL else { throw ReleaseFeedError.invalidChecksum }
        var downloaded: URL?
        do {
            async let checksumRequest = session.data(from: checksumURL)
            let (file, response) = try await session.download(from: release.downloadURL)
            downloaded = file
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                throw ReleaseFeedError.downloadFailed
            }
            let (checksumData, checksumResponse) = try await checksumRequest
            guard let http = checksumResponse as? HTTPURLResponse, http.statusCode == 200,
                  let expected = AppUpdateMetadata.checksum(from: checksumData) else {
                throw ReleaseFeedError.invalidChecksum
            }
            guard try await AppUpdateMetadata.sha256(of: file) == expected else {
                throw ReleaseFeedError.checksumMismatch
            }
            return file
        } catch {
            if let downloaded { try? FileManager.default.removeItem(at: downloaded) }
            throw error
        }
    }

    private func releaseData(at url: URL) async throws -> Data {
        var request = URLRequest(url: url)
        request.timeoutInterval = 15
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("AgentRelay/\(currentVersion)", forHTTPHeaderField: "User-Agent")
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw ReleaseFeedError.releaseLookupFailed
        }
        return data
    }
}

extension AppUpdateMetadata {
    /// Lower-case hex SHA-256 of the file at `url`, read a megabyte at a time.
    public static func sha256(of url: URL) async throws -> String {
        try await Task.detached(priority: .userInitiated) {
            let handle = try FileHandle(forReadingFrom: url)
            defer { try? handle.close() }
            var hasher = SHA256()
            while let data = try handle.read(upToCount: 1_048_576), !data.isEmpty {
                hasher.update(data: data)
            }
            return hasher.finalize().map { String(format: "%02x", $0) }.joined()
        }.value
    }
}

private struct GitHubRelease: Decodable {
    let tagName: String
    let draft: Bool
    let prerelease: Bool
    let assets: [GitHubAsset]

    var version: String { tagName.hasPrefix("v") ? String(tagName.dropFirst()) : tagName }

    func isEligible(includeBeta: Bool) -> Bool {
        guard !draft, let parsed = AppVersion(version) else { return false }
        if !prerelease { return parsed.prerelease == nil }
        guard includeBeta, let label = parsed.prerelease,
              label.hasPrefix("beta.") else { return false }
        let number = label.dropFirst(5)
        return !number.isEmpty && number.allSatisfy({ $0.isASCII && $0.isNumber })
    }

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case draft
        case prerelease
        case assets
    }
}

private struct GitHubAsset: Decodable {
    let name: String
    let browserDownloadURL: URL

    enum CodingKeys: String, CodingKey {
        case name
        case browserDownloadURL = "browser_download_url"
    }
}

private struct AppVersion: Comparable {
    let components: [Int]
    let prerelease: String?

    init?(_ value: String) {
        let versionAndBuild = value.split(separator: "+", maxSplits: 1, omittingEmptySubsequences: false)
        let versionAndPrerelease = versionAndBuild[0].split(
            separator: "-",
            maxSplits: 1,
            omittingEmptySubsequences: false
        )
        let pieces = versionAndPrerelease[0].split(separator: ".", omittingEmptySubsequences: false)
        guard pieces.count == 3 else { return nil }
        let components = pieces.compactMap { Int($0) }
        guard components.count == 3, components.allSatisfy({ $0 >= 0 }) else { return nil }
        self.components = components
        if versionAndPrerelease.count == 2, !versionAndPrerelease[1].isEmpty {
            prerelease = String(versionAndPrerelease[1])
        } else {
            prerelease = nil
        }
    }

    static func < (lhs: AppVersion, rhs: AppVersion) -> Bool {
        if lhs.components != rhs.components {
            return lhs.components.lexicographicallyPrecedes(rhs.components)
        }
        switch (lhs.prerelease, rhs.prerelease) {
        case (nil, nil): return false
        case (nil, _): return false
        case (_, nil): return true
        case let (left?, right?): return left.localizedStandardCompare(right) == .orderedAscending
        }
    }
}
