import Foundation
import RelayDesktopCore

/// Constants about this app itself. What a build is made for (its Agent
/// Relay site, link scheme, bundle id and update repository) is fixed when
/// `scripts/build-app.sh` makes the bundle, which records each beside the
/// app; nothing at run time changes any of them. A bundle without a record
/// (`swift run`) is production's.
enum AppInfo {
    static let subsystem = "com.agentrelay.desktop"
    /// The default profile's product name, whose artwork (the wordmark) the
    /// app carries; any other profile is named in text.
    static let defaultDisplayName = "Agent Relay"
    static let productionSiteURL = "https://agentrelay.com"
    static let productionBundleIdentifier = "com.agentrelay.desktop"
    /// Agent Relay Dev's bundle id, whose core keeps today's names.
    static let developmentBundleIdentifier = "com.agentrelay.desktop.dev"
    static let defaultURLScheme = "agentrelay"

    /// The site this build signs in to, from Resources/site-url.txt.
    static var defaultSiteURL: String { siteURL(from: resource("site-url")) }

    /// `defaultSiteURL` from the record's text: production's when there is
    /// none or it is no http(s) URL with a host.
    static func siteURL(from text: String?) -> String {
        guard let text else { return productionSiteURL }
        let site = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: site), ["http", "https"].contains(url.scheme ?? ""), url.host != nil else {
            return productionSiteURL
        }
        return site
    }

    /// The product name the window title, the brand's accessibility label
    /// and the wordmark's text say, from CFBundleDisplayName, which
    /// `build-app.sh --product-name` writes ("Agent Relay" by default).
    static var displayName: String {
        displayName(from: Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
    }

    /// `displayName` from the bundle's value: the default when there is none
    /// or it is blank.
    static func displayName(from text: String?) -> String {
        guard let name = text?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else {
            return defaultDisplayName
        }
        return name
    }

    /// Whether the build with `bundleIdentifier` and `productName` draws
    /// Agent Relay's wordmark: only production's does. Any other build
    /// (a site profile's, or Agent Relay Dev) is named in text, as on Linux
    /// and Windows, so it never looks like the production app beside it.
    /// A co-branded build (`coBrand`) named Agent Relay draws it too: the
    /// "for <customer>" line under it tells it apart.
    static func drawsWordmark(bundleIdentifier: String, productName: String, coBranded: Bool = false) -> Bool {
        productName == defaultDisplayName && (bundleIdentifier == productionBundleIdentifier || coBranded)
    }

    /// The short marker a build named Agent Relay (or Agent Relay Dev) other
    /// than production's carries beside its name (menu bar, window title,
    /// header): its `profileIdentity`. Nil for production and for a
    /// customer's branded build (`isBranded`), whose name already tells it
    /// apart and whose people never see the opaque profile id beside it.
    static var profileBadge: String? {
        profileBadge(bundleIdentifier: markedBundleIdentifier, profileID: profileID, productName: displayName,
                     coBranded: coBrand != nil)
    }

    /// Agent Relay Dev's product name, which is no customer's brand.
    static let developmentDisplayName = "Agent Relay Dev"

    /// Whether `productName` is a customer's own rather than Agent Relay's:
    /// any name but "Agent Relay" and "Agent Relay Dev", as on Linux and
    /// Windows.
    static func isBranded(productName: String) -> Bool {
        productName != defaultDisplayName && productName != developmentDisplayName
    }

    /// The bundle id this build is marked by: its own when `build-app.sh`
    /// made the bundle (it records Resources/site-url.txt in every one),
    /// else production's, like every other record. A host that is no such
    /// bundle (`swift run`, the XCTest runner) is never marked.
    static var markedBundleIdentifier: String {
        markedBundleIdentifier(bundleIdentifier: bundleIdentifier, recorded: resource("site-url") != nil)
    }

    /// `markedBundleIdentifier` for a bundle with `bundleIdentifier`, made by
    /// `build-app.sh` when `recorded`.
    static func markedBundleIdentifier(bundleIdentifier: String, recorded: Bool) -> String {
        recorded ? bundleIdentifier : productionBundleIdentifier
    }

    /// `profileBadge` for the build with `bundleIdentifier`, `profileID` and
    /// `productName`; nil when `coBranded`, whose "for <customer>" line
    /// tells it apart.
    static func profileBadge(bundleIdentifier: String, profileID: String, productName: String = defaultDisplayName,
                             coBranded: Bool = false) -> String? {
        guard !isBranded(productName: productName), !coBranded else { return nil }
        return profileIdentity(bundleIdentifier: bundleIdentifier, profileID: profileID)
    }

    /// What tells a build other than production's apart in About, branded
    /// or not: its site profile id, else its bundle id's part after
    /// `com.agentrelay.desktop.` (`dev` for Agent Relay Dev), else its
    /// bundle id; nil for production.
    static func profileIdentity(bundleIdentifier: String, profileID: String) -> String? {
        guard bundleIdentifier != productionBundleIdentifier else { return nil }
        if !profileID.isEmpty { return profileID }
        let suffix = bundleIdentifier.dropFirst(productionBundleIdentifier.count + 1)
        if bundleIdentifier.hasPrefix(productionBundleIdentifier + "."), !suffix.isEmpty { return String(suffix) }
        return bundleIdentifier
    }

    /// The main window's title: the product name, then the profile badge
    /// in parentheses for a build other than production's.
    static var windowTitle: String { windowTitle(productName: displayName, badge: profileBadge) }

    /// `windowTitle` from a product name and badge, as on Linux and Windows.
    static func windowTitle(productName: String, badge: String?) -> String {
        guard let badge else { return productName }
        return "\(productName) (\(badge))"
    }

    /// The host of `site`, without scheme, port, path or query; nil when it
    /// has none.
    static func siteHost(_ site: String) -> String? {
        URL(string: site)?.host
    }

    /// What About says about a build other than production's, label and
    /// value, word for word as on Linux and Windows: its profile, app id,
    /// site host and core socket. Empty for production, whose About is
    /// unchanged. Never a token, key or query string.
    static func identityRows(bundleIdentifier: String, profileID: String, site: String) -> [DesktopDiagnostics.Row] {
        guard let identity = profileIdentity(bundleIdentifier: bundleIdentifier, profileID: profileID) else { return [] }
        return [
            DesktopDiagnostics.Row("Profile", identity),
            DesktopDiagnostics.Row("App ID", bundleIdentifier),
            DesktopDiagnostics.Row("Site", siteHost(site)),
            DesktopDiagnostics.Row("Core socket", coreSocketName(bundleIdentifier: bundleIdentifier)),
        ]
    }

    /// `identityRows` for this build.
    static var identityRows: [DesktopDiagnostics.Row] {
        identityRows(bundleIdentifier: markedBundleIdentifier, profileID: profileID, site: defaultSiteURL)
    }

    /// Whether the app icon is the profile's mark (`build-app.sh --icon`,
    /// recorded as Resources/mark.txt), which the brand then draws in place
    /// of Agent Relay's.
    static var marksWithAppIcon: Bool { marksWithAppIcon(from: resource("mark")) }

    /// `marksWithAppIcon` from the record's text: only `AppIcon` says so.
    static func marksWithAppIcon(from text: String?) -> Bool {
        text?.trimmingCharacters(in: .whitespacesAndNewlines) == "AppIcon"
    }

    /// The customer a site profile's build is made for, which the sign-in
    /// window and the signed-out menu name under the Agent Relay lockup
    /// ("for <customer>"); the product stays Agent Relay.
    struct CoBrand: Equatable {
        /// The customer's name.
        var name: String
        /// The customer's mark, an SVG or a PNG.
        var mark: URL
    }

    /// This build's co-brand, from Resources/cobrand-name.txt and
    /// Resources/cobrand.svg or cobrand.png (`build-app.sh --cobrand-mark`
    /// and `--cobrand-name`, a site profile's only); nil without both, as
    /// for every build but a co-branded one.
    static let coBrand: CoBrand? = readCoBrand(
        name: resource("cobrand-name"),
        mark: Bundle.main.url(forResource: "cobrand", withExtension: "svg")
            ?? Bundle.main.url(forResource: "cobrand", withExtension: "png"))

    /// `coBrand` from the name's record and the mark's file: nil unless
    /// both are there and the name is usable (`coBrandName`).
    static func readCoBrand(name text: String?, mark: URL?) -> CoBrand? {
        guard let mark, let name = coBrandName(from: text) else { return nil }
        return CoBrand(name: name, mark: mark)
    }

    /// The customer's name from the record's text, trimmed: nil when there
    /// is none, or it is blank, more than one line, holds a control
    /// character or is over 80 characters.
    static func coBrandName(from text: String?) -> String? {
        guard let name = text?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty,
              name.count <= 80,
              name.unicodeScalars.allSatisfy({ $0.properties.generalCategory != .control }) else { return nil }
        return name
    }

    /// The scheme of the links this build opens (`agentrelay://…`), from
    /// Resources/url-scheme.txt; the same scheme is in its Info.plist.
    static var urlScheme: String { urlScheme(from: resource("url-scheme")) }

    /// `urlScheme` from the record's text: `agentrelay` when there is none
    /// or it is no scheme name (a letter, then letters, digits, `+`, `-`
    /// and `.`; never one of the web's).
    static func urlScheme(from text: String?) -> String {
        guard let text else { return defaultURLScheme }
        let scheme = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard scheme.range(of: "^[a-z][a-z0-9+.-]*$", options: .regularExpression) != nil,
              !["http", "https", "ws", "wss", "ftp", "file"].contains(scheme) else { return defaultURLScheme }
        return scheme
    }

    /// The GitHub repository (`OWNER/NAME`) whose releases the app checks
    /// for updates, from Resources/releases-repository.txt; empty for a
    /// build with a cloud release feed (Resources/release-feed.txt).
    static var releasesRepository: String {
        releasesRepository(from: resource("releases-repository"), feed: resource("release-feed"))
    }

    /// `releasesRepository` from the record's text: the default when there
    /// is none or it is not `OWNER/NAME`. With a cloud release feed record
    /// (`feed`) it is empty whatever the text: such a build reads no GitHub
    /// repository, and never production's.
    static func releasesRepository(from text: String?, feed: String? = nil) -> String {
        if isCloudFeed(feed) { return "" }
        guard let text else { return AppUpdateMetadata.defaultReleasesRepository }
        let repository = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard repository.range(of: "^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", options: .regularExpression) != nil else {
            return AppUpdateMetadata.defaultReleasesRepository
        }
        return repository
    }

    /// Where this build's releases come from, from Resources/release-feed.txt:
    /// `github:OWNER/NAME`, a GitHub repository's releases, or `cloud`, the
    /// site's own feed. A build without the record (every build before
    /// there was a feed, and the default profile's) reads
    /// `releasesRepository`'s releases.
    static var releaseFeed: String { releaseFeed(from: resource("release-feed")) }

    /// `releaseFeed` from the record's text: `github:` and
    /// `releasesRepository` when there is none or it is neither `cloud` nor
    /// `github:OWNER/NAME`.
    static func releaseFeed(from text: String?) -> String {
        if isCloudFeed(text) { return AppUpdateMetadata.cloudReleaseFeed }
        let fallback = "github:" + releasesRepository
        guard let text else { return fallback }
        let feed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard feed == AppUpdateMetadata.cloudReleaseFeed
                || feed.range(of: "^github:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", options: .regularExpression) != nil else {
            return fallback
        }
        return feed
    }

    /// Whether a release feed record's text names the cloud feed.
    static func isCloudFeed(_ text: String?) -> Bool {
        text?.trimmingCharacters(in: .whitespacesAndNewlines) == AppUpdateMetadata.cloudReleaseFeed
    }

    /// The Ed25519 public key (base64 of 32 bytes) a cloud feed's release
    /// records are signed with, from Resources/feed-public-key.txt; empty
    /// for a `github:` feed.
    static var feedPublicKey: String { feedPublicKey(from: resource("feed-public-key")) }

    /// `feedPublicKey` from the record's text: empty when there is none or
    /// it is not the base64 of 32 bytes.
    static func feedPublicKey(from text: String?) -> String {
        guard let text else { return "" }
        let key = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard key.range(of: "^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$", options: .regularExpression) != nil else {
            return ""
        }
        return key
    }

    /// The first part of this build's release asset names (`AgentRelay`, or
    /// a site profile's `Desktop-<id>`), from Resources/asset-prefix.txt;
    /// the updater looks for its own images, not production's.
    static var assetPrefix: String { assetPrefix(from: resource("asset-prefix")) }

    /// `assetPrefix` from the record's text: the default when there is none
    /// or it is not a plain name (letters, digits and hyphens, no leading
    /// or trailing hyphen).
    static func assetPrefix(from text: String?) -> String {
        guard let text else { return AppUpdateMetadata.defaultAssetPrefix }
        let prefix = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard prefix.range(of: "^[A-Za-z0-9]+(-[A-Za-z0-9]+)*$", options: .regularExpression) != nil else {
            return AppUpdateMetadata.defaultAssetPrefix
        }
        return prefix
    }

    /// The site profile id this build was made for: its asset prefix's part
    /// after `Desktop-` (as gtk/build.rs derives it); empty for the default
    /// profile, which reads no cloud feed.
    static var profileID: String { profileID(fromAssetPrefix: assetPrefix) }

    /// `profileID` from an asset prefix: empty unless it is `Desktop-<id>`.
    static func profileID(fromAssetPrefix prefix: String) -> String {
        guard prefix.hasPrefix("Desktop-") else { return "" }
        return String(prefix.dropFirst("Desktop-".count))
    }

    /// A development build is one made for a loopback site (Agent Relay
    /// Dev, for a Cloud on this Mac). A build for any other site is a
    /// release build like production's: it opens at login and installs its
    /// command-line copy the same way.
    static var isDevelopment: Bool { isLoopbackSite(defaultSiteURL) }

    /// Whether `site` is on this Mac: `http://localhost[:port]` or
    /// `http://127.0.0.1[:port]`, the only loopback sites a build can be
    /// made for.
    static func isLoopbackSite(_ site: String) -> Bool {
        guard let url = URL(string: site), let host = url.host?.lowercased() else { return false }
        return host == "localhost" || host == "127.0.0.1"
    }

    /// This build's bundle id, which its core's LaunchAgent label, socket
    /// and data directory follow (`build-app.sh` derives them from the same
    /// id, so builds with different ids never share a core).
    static var bundleIdentifier: String { Bundle.main.bundleIdentifier ?? productionBundleIdentifier }

    /// The bundle's file name as it was built (`CFBundleName` plus `.app`),
    /// which is what an update must be named: a renamed copy on disk does
    /// not change it.
    static var bundleFileName: String {
        let name = (Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String)
            .flatMap { $0.isEmpty ? nil : $0 } ?? defaultDisplayName
        return name + ".app"
    }

    /// This build's directory in Application Support, which the core owns:
    /// the agent store, outboxes, conversations and the relay-send socket.
    /// Named after the bundle id; Agent Relay Dev keeps its own.
    static var dataDirectory: URL { dataDirectory(in: nil) }

    static func dataDirectory(in supportDirectory: URL?, bundleIdentifier: String = AppInfo.bundleIdentifier) -> URL {
        (supportDirectory ?? applicationSupport).appendingPathComponent(bundleIdentifier)
    }

    static var applicationSupport: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support")
    }

    /// The core's bridge socket, as its LaunchAgent serves it:
    /// `~/.agentworkforce/desktop/core.sock` (`core.dev.sock` for Agent
    /// Relay Dev, `core.<bundle id>.sock` for a build with another id).
    static var coreSocket: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".agentworkforce/desktop/\(coreSocketName(bundleIdentifier: bundleIdentifier))").path
    }

    /// The socket file name the core of the build with `bundleIdentifier`
    /// serves, as `build-app.sh` writes it into that build's LaunchAgent.
    static func coreSocketName(bundleIdentifier: String) -> String {
        switch bundleIdentifier {
        case productionBundleIdentifier: return "core.sock"
        case developmentBundleIdentifier: return "core.dev.sock"
        default: return "core.\(bundleIdentifier).sock"
        }
    }

    /// The core version in this bundle, from Resources/probe-version.txt.
    static var bundledCoreVersion: String? {
        guard let text = resource("probe-version") else { return nil }
        let version = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return version.isEmpty ? nil : version
    }

    /// The core crates this build's bundled core was compiled with: the
    /// commit of agent-relay-node and the version of relaycast its lockfiles
    /// resolved, the pins or an internal build's overrides
    /// (`scripts/apply-crate-overrides.sh`).
    struct CoreCrates: Equatable {
        let agentRelayNode: String
        let relaycast: String
    }

    /// From Resources/crates.json, which `build-app.sh` writes; nil when the
    /// bundle has none (a development reload).
    static var coreCrates: CoreCrates? {
        guard let url = Bundle.main.url(forResource: "crates", withExtension: "json"),
              let data = try? Data(contentsOf: url) else { return nil }
        return coreCrates(from: data)
    }

    /// `{"agent-relay-node":{"rev":SHA},"relaycast":{"version":V}}`, the
    /// shape `apply-crate-overrides.sh --json` writes; anything else is nil.
    static func coreCrates(from data: Data) -> CoreCrates? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        func text(_ crate: String, _ field: String) -> String? {
            let value = ((object[crate] as? [String: Any])?[field] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return value?.isEmpty == false ? value : nil
        }
        guard let node = text("agent-relay-node", "rev"),
              let relaycast = text("relaycast", "version") else { return nil }
        return CoreCrates(agentRelayNode: node, relaycast: relaycast)
    }

    /// The welcome screen and connected settings share one persistent window.
    static let mainWindowID = "main"

    static var buildRevision: String? {
        guard let value = resource("build-revision") else { return nil }
        let revision = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return revision.isEmpty ? nil : revision
    }

    static var displayVersion: String {
        if isDevelopment || version == "0.0.0" || version == "development build" {
            return "Development (\(buildRevision ?? "unknown revision"))"
        }
        return version
    }

    /// Marketing version from the bundle, e.g. "1.2.0".
    static var version: String {
        let value = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        guard let value, !value.isEmpty else { return "development build" }
        return value
    }

    /// The text of Resources/<name>.txt, which `build-app.sh` writes; nil
    /// when the bundle has none.
    private static func resource(_ name: String) -> String? {
        guard let url = Bundle.main.url(forResource: name, withExtension: "txt") else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }
}
