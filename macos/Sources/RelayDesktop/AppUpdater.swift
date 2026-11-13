import AppKit
import Foundation
import Observation
import RelayDesktopCore

enum AppUpdateState: Equatable {
    case idle
    case checking
    case upToDate(Date)
    case available(AppRelease)
    case downloading(AppRelease)
    case installing(AppRelease)
    case unavailable(String)
    case failed(String)
}

@MainActor
@Observable
final class AppUpdater {
    static let shared = AppUpdater()
    @ObservationIgnored var telemetryTimeoutNanoseconds: UInt64 = 3_000_000_000
    @ObservationIgnored var recordTelemetry: (TelemetryEvent) async -> Void = { _ in }
    /// The signed-in account's headers for one request to a cloud feed on
    /// the given site (`account.feed_headers`); nil when signed out or the
    /// core cannot supply them, which asks the user to sign in.
    @ObservationIgnored var feedHeaders: (String) async -> [String: String]? = { _ in nil }

    private(set) var state: AppUpdateState = .idle
    var automaticallyChecksForUpdates: Bool {
        didSet {
            defaults.set(automaticallyChecksForUpdates, forKey: Self.automaticChecksKey)
            // Turned on: a check that is due runs now, not at the next wake.
            if automaticallyChecksForUpdates && !oldValue { Task { await checkAutomaticallyIfNeeded() } }
        }
    }
    var includeBetaReleases: Bool {
        didSet {
            guard includeBetaReleases != oldValue else { return }
            defaults.set(includeBetaReleases, forKey: Self.betaReleasesKey)
            defaults.removeObject(forKey: Self.lastCheckKey)
            defaults.removeObject(forKey: Self.cachedReleaseKey)
            defaults.removeObject(forKey: Self.cachedNoReleaseKey)
            defaults.removeObject(forKey: Self.cachedWriterKey)
            state = .idle
        }
    }

    private static let betaReleasesKey = "includeBetaAppReleases"
    private static let cachedChannelKey = "cachedAppUpdateIncludesBeta"
    private static let automaticChecksKey = "automaticallyChecksForAppUpdates"
    private static let lastCheckKey = "lastAppUpdateCheck"
    private static let cachedReleaseKey = "cachedAppUpdateRelease"
    /// Set when the last check found no newer release in a cloud feed, which
    /// names none to cache: a relaunch restores it as up to date.
    private static let cachedNoReleaseKey = "cachedAppUpdateFoundNoRelease"
    /// The app version and macOS major that chose the cached release's disk
    /// image. A cache from any other pair is discarded at launch: an older
    /// app may have cached an image this version's floor logic would not
    /// pick, such as the unfloored name that holds the updater bridge.
    private static let cachedWriterKey = "cachedAppUpdateWriter"
    private static let automaticCheckInterval: TimeInterval = 24 * 60 * 60
    /// A check that failed, or could not run, is tried again this soon.
    private static let automaticCheckRetryInterval: TimeInterval = 15 * 60

    private let defaults: UserDefaults
    private let session: URLSession
    private let now: () -> Date
    private let currentVersion: String
    /// The GitHub repository whose releases are checked: the one this
    /// build was made for (Resources/releases-repository.txt).
    private let releasesRepository: String
    /// Where this build's releases come from (Resources/release-feed.txt):
    /// `releasesRepository`'s releases, or the site's signed feed (`cloud`).
    private let releaseFeed: String
    /// The first part of this build's asset names (Resources/asset-prefix.txt).
    private let assetPrefix: String
    /// The cloud feed's key (Resources/feed-public-key.txt).
    private let feedPublicKey: String
    /// The site whose feed a cloud build reads: the one it was made for.
    private let site: String
    /// The product name the sign-in and entitlement messages say.
    private let productName: String
    private let macOSMajorVersion: Int
    private let terminate: @MainActor () -> Void
    /// A build for a loopback site (Agent Relay Dev) never checks for or
    /// installs updates: it has no releases of its own, and production's
    /// could not install over it.
    private let development: Bool

    init(
        defaults: UserDefaults = .standard,
        session: URLSession = .shared,
        currentVersion: String = AppInfo.version,
        releasesRepository: String = AppInfo.releasesRepository,
        releaseFeed: String = AppInfo.releaseFeed,
        assetPrefix: String = AppInfo.assetPrefix,
        feedPublicKey: String = AppInfo.feedPublicKey,
        site: String = AppInfo.defaultSiteURL,
        productName: String = AppInfo.displayName,
        macOSMajorVersion: Int = AppUpdateMetadata.currentMacOSMajorVersion,
        development: Bool = AppInfo.isDevelopment,
        now: @escaping () -> Date = Date.init,
        terminate: @escaping @MainActor () -> Void = { NSApp.terminate(nil) }
    ) {
        self.defaults = defaults
        self.session = session
        self.development = development
        self.currentVersion = currentVersion
        self.releasesRepository = releasesRepository
        self.releaseFeed = releaseFeed
        self.assetPrefix = assetPrefix
        self.feedPublicKey = feedPublicKey
        self.site = site
        self.productName = productName
        self.macOSMajorVersion = macOSMajorVersion
        self.now = now
        self.terminate = terminate
        includeBetaReleases = defaults.bool(forKey: Self.betaReleasesKey)
        if defaults.object(forKey: Self.automaticChecksKey) == nil {
            automaticallyChecksForUpdates = true
        } else {
            automaticallyChecksForUpdates = defaults.bool(forKey: Self.automaticChecksKey)
        }
        restoreCachedState()
    }

    var isBusy: Bool {
        switch state {
        case .checking, .downloading, .installing: return true
        default: return false
        }
    }

    /// Checks once a day while the app is alive, sleeping until the next
    /// check is due. A check that failed is tried again after 15 minutes, so
    /// a login-item launch does not wait for a relaunch when the network was
    /// not up yet.
    func runAutomaticChecks() async {
        while !Task.isCancelled {
            await checkAutomaticallyIfNeeded()
            do {
                try await Task.sleep(nanoseconds: UInt64(nextAutomaticCheck() * 1_000_000_000))
            } catch {
                return
            }
        }
    }

    /// Seconds until the daily check is due: a day after the last successful
    /// check, or the retry interval when none succeeded or it is overdue.
    /// With automatic checks off, it looks again a day later.
    func nextAutomaticCheck() -> TimeInterval {
        guard automaticallyChecksForUpdates else { return Self.automaticCheckInterval }
        guard let lastCheck = defaults.object(forKey: Self.lastCheckKey) as? Date else { return Self.automaticCheckRetryInterval }
        let due = Self.automaticCheckInterval - now().timeIntervalSince(lastCheck)
        guard due > 0, due <= Self.automaticCheckInterval else { return Self.automaticCheckRetryInterval }
        return due
    }

    func checkAutomaticallyIfNeeded() async {
        guard automaticallyChecksForUpdates, !development,
              currentVersion != "development build" else { return }
        if let lastCheck = defaults.object(forKey: Self.lastCheckKey) as? Date {
            let elapsed = now().timeIntervalSince(lastCheck)
            if elapsed >= 0, elapsed < Self.automaticCheckInterval { return }
        }
        await checkForUpdates()
    }

    func checkForUpdates() async {
        guard !isBusy else { return }
        guard !development, currentVersion != "development build" else {
            state = .unavailable("Update checks are available in installed release builds.")
            return
        }

        state = .checking
        let includeBeta = includeBetaReleases
        do {
            let release = try await releaseReader.latestRelease(includeBeta: includeBeta)
            guard includeBeta == includeBetaReleases else { return }
            let checkedAt = now()
            defaults.set(checkedAt, forKey: Self.lastCheckKey)
            defaults.set(includeBeta, forKey: Self.cachedChannelKey)
            defaults.set(cacheWriter, forKey: Self.cachedWriterKey)
            // A cloud feed with no newer release names none: nothing to cache
            // but that the check found none.
            defaults.set(release == nil, forKey: Self.cachedNoReleaseKey)
            guard let release else {
                defaults.removeObject(forKey: Self.cachedReleaseKey)
                state = .upToDate(checkedAt)
                return
            }
            if let encodedRelease = try? JSONEncoder().encode(release) {
                defaults.set(encodedRelease, forKey: Self.cachedReleaseKey)
            }
            state = AppUpdateMetadata.isNewer(release.version, than: currentVersion)
                ? .available(release)
                : .upToDate(checkedAt)
        } catch {
            guard includeBeta == includeBetaReleases else { return }
            state = .failed(Self.checkMessage(for: error))
        }
    }

    /// This build's release feed: its site's signed feed for a `cloud`
    /// build, else its GitHub repository's releases.
    private var releaseReader: any ReleaseFeed {
        if releaseFeed == AppUpdateMetadata.cloudReleaseFeed {
            return CloudFeed(
                site: site, publicKey: feedPublicKey, profileID: AppInfo.profileID(fromAssetPrefix: assetPrefix),
                assetPrefix: assetPrefix, installedVersion: currentVersion, productName: productName,
                architecture: .current, macOSMajorVersion: macOSMajorVersion, session: session,
                headers: feedHeaders)
        }
        return GitHubReleasesFeed(
            repository: releasesRepository, assetPrefix: assetPrefix, currentVersion: currentVersion,
            architecture: .current, macOSMajorVersion: macOSMajorVersion, session: session)
    }

    func downloadAndInstall(_ release: AppRelease) async {
        guard !isBusy else { return }
        state = .downloading(release)
        await recordUpdateTelemetry(TelemetryEvent(operation: .updateInstall, outcome: .started))
        var temporaryURL: URL?
        do {
            let downloadedURL = try await releaseReader.diskImage(of: release)
            temporaryURL = downloadedURL
            state = .installing(release)
            let staged = try await Task.detached(priority: .userInitiated) {
                try AppUpdateInstaller.prepare(diskImage: downloadedURL)
            }.value
            do {
                try AppUpdateInstaller.launchReplacement(stagedApp: staged)
            } catch {
                try? FileManager.default.removeItem(at: staged)
                throw error
            }
            temporaryURL = nil
            await recordUpdateTelemetry(TelemetryEvent(operation: .updateInstall, outcome: .succeeded))
            terminate()
        } catch {
            if let temporaryURL { try? FileManager.default.removeItem(at: temporaryURL) }
            state = .failed(Self.downloadMessage(for: error))
            await recordUpdateTelemetry(TelemetryEvent(failure: .updateFailed, operation: .updateInstall, outcome: .failed))
        }
    }

    /// Bound the entire telemetry RPC, including connection and retries. A task
    /// group would still wait for an uncooperative RPC after cancelling it.
    /// Keep the success await before termination so the core can flush first.
    func recordUpdateTelemetry(_ event: TelemetryEvent) async {
        let record = recordTelemetry
        let timeout = telemetryTimeoutNanoseconds
        await withCheckedContinuation { continuation in
            let completion = UpdateTelemetryCompletion(continuation)
            let recording = Task {
                await record(event)
                completion.finish()
            }
            let deadline = Task {
                try? await Task.sleep(nanoseconds: timeout)
                completion.finish()
            }
            completion.tasks = [recording, deadline]
        }
    }

    private var cacheWriter: String { "\(currentVersion) macOS \(macOSMajorVersion)" }

    /// Restores the last check's result when this app version on this macOS
    /// wrote it. Otherwise the cache and its throttle are discarded, so the
    /// next automatic check looks the release up again.
    private func restoreCachedState() {
        guard defaults.string(forKey: Self.cachedWriterKey) == cacheWriter else {
            for key in [
                Self.lastCheckKey, Self.cachedReleaseKey, Self.cachedNoReleaseKey, Self.cachedChannelKey,
                Self.cachedWriterKey,
            ] {
                defaults.removeObject(forKey: key)
            }
            return
        }
        guard currentVersion != "development build",
              defaults.bool(forKey: Self.cachedChannelKey) == includeBetaReleases,
              let lastCheck = defaults.object(forKey: Self.lastCheckKey) as? Date else { return }
        // A cloud feed check that found no newer release is up to date.
        if defaults.bool(forKey: Self.cachedNoReleaseKey) {
            state = .upToDate(lastCheck)
            return
        }
        guard let data = defaults.data(forKey: Self.cachedReleaseKey),
              let release = try? JSONDecoder().decode(AppRelease.self, from: data) else { return }
        state = AppUpdateMetadata.isNewer(release.version, than: currentVersion)
            ? .available(release)
            : .upToDate(lastCheck)
    }

    private static func checkMessage(for error: Error) -> String {
        switch error {
        case let error as ReleaseFeedFailure:
            return error.message
        case AppUpdateMetadataError.missingAsset:
            return AppUpdateMetadata.noCompatibleUpdateMessage
        case let AppUpdateMetadataError.requiresNewerMacOS(floor):
            return AppUpdateMetadata.requiresNewerMacOSMessage(floor: floor)
        default:
            return AppUpdateMetadata.unreadableMessage
        }
    }

    private static func downloadMessage(for error: Error) -> String {
        switch error {
        case let error as AppUpdateInstallError:
            return error.localizedDescription
        case let error as ReleaseFeedFailure:
            return error.message
        case ReleaseFeedError.invalidChecksum, ReleaseFeedError.checksumMismatch:
            return AppUpdateMetadata.unverifiedMessage
        default:
            return AppUpdateMetadata.downloadFailedMessage
        }
    }
}

/// Main-actor isolation serializes the deadline and reply, including late replies.
@MainActor
private final class UpdateTelemetryCompletion {
    private var continuation: CheckedContinuation<Void, Never>?
    var tasks: [Task<Void, Never>] = []

    init(_ continuation: CheckedContinuation<Void, Never>) {
        self.continuation = continuation
    }

    func finish() {
        guard let continuation else { return }
        self.continuation = nil
        tasks.forEach { $0.cancel() }
        tasks = []
        continuation.resume()
    }
}
