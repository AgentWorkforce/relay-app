import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// What a build is made for is read from the records build-app.sh writes
/// beside the app, and what the app says about its site and links matches
/// Linux and Windows word for word.
@MainActor
final class SiteProfileTests: XCTestCase {
    func testTheSiteRecordIsReadWithProductionAsTheFallback() {
        XCTAssertEqual(AppInfo.siteURL(from: "http://localhost:3100\n"), "http://localhost:3100")
        XCTAssertEqual(AppInfo.siteURL(from: "  https://staging.example  "), "https://staging.example")
        XCTAssertEqual(AppInfo.siteURL(from: nil), AppInfo.productionSiteURL)
        XCTAssertEqual(AppInfo.siteURL(from: ""), AppInfo.productionSiteURL)
        XCTAssertEqual(AppInfo.siteURL(from: "file:///tmp"), AppInfo.productionSiteURL)
        XCTAssertEqual(AppInfo.siteURL(from: "not a url"), AppInfo.productionSiteURL)
    }

    func testTheSchemeRecordIsReadWithTheDefaultAsTheFallback() {
        XCTAssertEqual(AppInfo.urlScheme(from: "agentrelay-dev\n"), "agentrelay-dev")
        XCTAssertEqual(AppInfo.urlScheme(from: " Relay-Staging "), "relay-staging")
        XCTAssertEqual(AppInfo.urlScheme(from: nil), "agentrelay")
        XCTAssertEqual(AppInfo.urlScheme(from: ""), "agentrelay")
        XCTAssertEqual(AppInfo.urlScheme(from: "1relay"), "agentrelay")
        XCTAssertEqual(AppInfo.urlScheme(from: "agent relay"), "agentrelay")
        // The web's schemes, which a link of the app's cannot use.
        XCTAssertEqual(AppInfo.urlScheme(from: "https"), "agentrelay")
    }

    func testTheReleasesRepositoryRecordIsReadWithTheDefaultAsTheFallback() {
        XCTAssertEqual(AppInfo.releasesRepository(from: "Example/relay-releases\n"), "Example/relay-releases")
        XCTAssertEqual(AppInfo.releasesRepository(from: nil), "AgentWorkforce/relay-desktop-releases")
        XCTAssertEqual(AppInfo.releasesRepository(from: nil), AppUpdateMetadata.defaultReleasesRepository)
        XCTAssertEqual(AppInfo.releasesRepository(from: ""), AppUpdateMetadata.defaultReleasesRepository)
        XCTAssertEqual(AppInfo.releasesRepository(from: "nope"), AppUpdateMetadata.defaultReleasesRepository)
        XCTAssertEqual(AppInfo.releasesRepository(from: "a/b/c"), AppUpdateMetadata.defaultReleasesRepository)
        XCTAssertEqual(AppInfo.releasesRepository(from: "https://github.com/a/b"), AppUpdateMetadata.defaultReleasesRepository)
    }

    func testACloudFeedNeverFallsBackToProductionsRepository() {
        // A cloud build records no repository; it reads none, not the default.
        XCTAssertEqual(AppInfo.releasesRepository(from: nil, feed: "cloud\n"), "")
        XCTAssertEqual(AppInfo.releasesRepository(from: "", feed: "cloud"), "")
        XCTAssertEqual(AppInfo.releasesRepository(from: "Example/relay-releases", feed: "cloud"), "")
        XCTAssertNotEqual(AppInfo.releaseFeed(from: "cloud"), "github:" + AppUpdateMetadata.defaultReleasesRepository)
        // Any other feed record leaves the repository record as it was.
        XCTAssertEqual(AppInfo.releasesRepository(from: nil, feed: "github:Example/relay-releases"),
                       AppUpdateMetadata.defaultReleasesRepository)
        XCTAssertEqual(AppInfo.releasesRepository(from: "Example/relay-releases", feed: "Cloud"), "Example/relay-releases")
        XCTAssertEqual(AppInfo.releasesRepository(from: "Example/relay-releases", feed: nil), "Example/relay-releases")
    }

    func testTheReleaseFeedRecordIsReadWithTheRepositoryAsTheFallback() {
        let fallback = "github:" + AppInfo.releasesRepository
        XCTAssertEqual(AppInfo.releaseFeed(from: "cloud\n"), "cloud")
        XCTAssertEqual(AppInfo.releaseFeed(from: "github:Example/relay-releases\n"), "github:Example/relay-releases")
        XCTAssertEqual(AppInfo.releaseFeed(from: nil), fallback)
        XCTAssertEqual(AppInfo.releaseFeed(from: ""), fallback)
        XCTAssertEqual(AppInfo.releaseFeed(from: "Cloud"), fallback)
        XCTAssertEqual(AppInfo.releaseFeed(from: "github:a/b/c"), fallback)
        XCTAssertEqual(AppInfo.releaseFeed(from: "https://relay.example"), fallback)
    }

    func testTheFeedPublicKeyRecordIsAnEd25519KeyOrNone() {
        let key = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="
        XCTAssertEqual(AppInfo.feedPublicKey(from: key + "\n"), key)
        XCTAssertEqual(AppInfo.feedPublicKey(from: nil), "")
        XCTAssertEqual(AppInfo.feedPublicKey(from: ""), "")
        XCTAssertEqual(AppInfo.feedPublicKey(from: String(key.dropLast())), "")
        // A last character with stray bits is not the base64 of 32 bytes.
        XCTAssertEqual(AppInfo.feedPublicKey(from: "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURp="), "")
    }

    func testTheProductNameBrandsTheAppLikeLinuxAndWindows() {
        // The window title and the brand's accessibility label say the
        // bundle's product name; the default when there is none.
        XCTAssertEqual(AppInfo.displayName(from: "Acme Desktop"), "Acme Desktop")
        XCTAssertEqual(AppInfo.displayName(from: " Acme Desktop\n"), "Acme Desktop")
        XCTAssertEqual(AppInfo.displayName(from: "Agent Relay"), "Agent Relay")
        XCTAssertEqual(AppInfo.displayName(from: nil), "Agent Relay")
        XCTAssertEqual(AppInfo.displayName(from: "  "), "Agent Relay")
        XCTAssertEqual(AppInfo.displayName(from: nil), AppInfo.defaultDisplayName)
        // Only production draws the wordmark; any other build is named in text.
        let production = AppInfo.productionBundleIdentifier
        XCTAssertTrue(BrandWordmarkView(productName: "Agent Relay", bundleIdentifier: production).drawsArtwork)
        XCTAssertFalse(BrandWordmarkView(productName: "Acme Desktop", bundleIdentifier: production).drawsArtwork)
        XCTAssertFalse(BrandWordmarkView(productName: "Agent Relay Dev",
                                         bundleIdentifier: AppInfo.developmentBundleIdentifier).drawsArtwork)
        XCTAssertFalse(BrandWordmarkView(productName: "Agent Relay Dev",
                                         bundleIdentifier: "com.agentrelay.desktop.devaws").drawsArtwork)
        XCTAssertFalse(BrandWordmarkView(productName: "Agent Relay",
                                         bundleIdentifier: "com.agentrelay.desktop.devaws").drawsArtwork)
        // The app icon is the mark only when build-app.sh recorded a profile's.
        XCTAssertTrue(AppInfo.marksWithAppIcon(from: "AppIcon\n"))
        XCTAssertFalse(AppInfo.marksWithAppIcon(from: nil))
        XCTAssertFalse(AppInfo.marksWithAppIcon(from: ""))
    }

    func testABuildOtherThanProductionCarriesItsProfileBadge() {
        let production = AppInfo.productionBundleIdentifier
        XCTAssertNil(AppInfo.profileBadge(bundleIdentifier: production, profileID: ""))
        XCTAssertEqual(AppInfo.profileBadge(bundleIdentifier: "com.agentrelay.desktop.devaws", profileID: "d4vq8z"), "d4vq8z")
        XCTAssertEqual(AppInfo.profileBadge(bundleIdentifier: "com.agentrelay.desktop.devaws", profileID: ""), "devaws")
        XCTAssertEqual(AppInfo.profileBadge(bundleIdentifier: AppInfo.developmentBundleIdentifier, profileID: ""), "dev")
        XCTAssertEqual(AppInfo.profileBadge(bundleIdentifier: "com.example.relay", profileID: ""), "com.example.relay")
        // A host build-app.sh did not make (swift run, the XCTest runner) counts as production's.
        XCTAssertEqual(AppInfo.markedBundleIdentifier(bundleIdentifier: "com.apple.dt.xctest.tool", recorded: false), production)
        XCTAssertEqual(AppInfo.markedBundleIdentifier(bundleIdentifier: "com.agentrelay.desktop.devaws", recorded: true),
                       "com.agentrelay.desktop.devaws")
        // The window title appends it, as on Linux and Windows; production's is unchanged.
        XCTAssertEqual(AppInfo.windowTitle(productName: "Agent Relay", badge: nil), "Agent Relay")
        XCTAssertEqual(AppInfo.windowTitle(productName: "Agent Relay Dev", badge: "d4vq8z"), "Agent Relay Dev (d4vq8z)")
    }

    func testABrandedBuildCarriesNoBadgeButKeepsItsIdentity() {
        let branded = "Agent Relay for Automation Anywhere"
        XCTAssertTrue(AppInfo.isBranded(productName: branded))
        XCTAssertNil(AppInfo.profileBadge(bundleIdentifier: "com.agentrelay.desktop.m5lxqm2osk", profileID: "m5lxqm2osk", productName: branded))
        XCTAssertEqual(AppInfo.windowTitle(productName: branded, badge: nil), branded)
        XCTAssertEqual(AppInfo.profileIdentity(bundleIdentifier: "com.agentrelay.desktop.m5lxqm2osk", profileID: "m5lxqm2osk"), "m5lxqm2osk")
        for name in [AppInfo.defaultDisplayName, AppInfo.developmentDisplayName] {
            XCTAssertFalse(AppInfo.isBranded(productName: name))
            XCTAssertEqual(AppInfo.profileBadge(bundleIdentifier: "com.agentrelay.desktop.devaws", profileID: "d4vq8z", productName: name), "d4vq8z")
        }
        let rows = AppInfo.identityRows(bundleIdentifier: "com.agentrelay.desktop.m5lxqm2osk", profileID: "m5lxqm2osk", site: "https://ar.example.com")
        XCTAssertEqual(rows.first?.value, "m5lxqm2osk")
    }

    func testAboutShowsABuildsIdentityButNothingForProduction() {
        XCTAssertTrue(AppInfo.identityRows(bundleIdentifier: AppInfo.productionBundleIdentifier, profileID: "",
                                           site: AppInfo.productionSiteURL).isEmpty)
        let rows = AppInfo.identityRows(bundleIdentifier: "com.agentrelay.desktop.devaws", profileID: "d4vq8z",
                                        site: "https://dev.example.com:8443/app?token=secret#x")
        XCTAssertEqual(rows.map(\.label), ["Profile", "App ID", "Site", "Core socket"])
        XCTAssertEqual(rows.map(\.value),
                       ["d4vq8z", "com.agentrelay.desktop.devaws", "dev.example.com", "core.com.agentrelay.desktop.devaws.sock"])
        // The site's host only: never its query, path or port.
        XCTAssertFalse(rows.contains { $0.value.contains("token") || $0.value.contains("8443") })
        XCTAssertEqual(AppInfo.identityRows(bundleIdentifier: AppInfo.developmentBundleIdentifier, profileID: "",
                                            site: "http://localhost:3100").map(\.value),
                       ["dev", "com.agentrelay.desktop.dev", "localhost", "core.dev.sock"])
    }

    func testOnlyALoopbackSiteIsADevelopmentBuild() {
        XCTAssertTrue(AppInfo.isLoopbackSite("http://localhost:3100"))
        XCTAssertTrue(AppInfo.isLoopbackSite("http://127.0.0.1:3100"))
        XCTAssertTrue(AppInfo.isLoopbackSite("http://localhost"))
        // A build for another site is a release build like production's.
        XCTAssertFalse(AppInfo.isLoopbackSite("https://staging.example"))
        XCTAssertFalse(AppInfo.isLoopbackSite("https://agentrelay.com"))
        XCTAssertFalse(AppInfo.isLoopbackSite("https://localhost.example"))
        XCTAssertFalse(AppInfo.isLoopbackSite(""))
    }

    func testTheCoreSocketAndDataDirectoryFollowTheBundleID() {
        // Today's names for production and Agent Relay Dev; another build's
        // never collide with either.
        XCTAssertEqual(AppInfo.coreSocketName(bundleIdentifier: "com.agentrelay.desktop"), "core.sock")
        XCTAssertEqual(AppInfo.coreSocketName(bundleIdentifier: "com.agentrelay.desktop.dev"), "core.dev.sock")
        XCTAssertEqual(AppInfo.coreSocketName(bundleIdentifier: "com.example.relay"), "core.com.example.relay.sock")
        let support = URL(fileURLWithPath: "/tmp/Application Support", isDirectory: true)
        XCTAssertEqual(AppInfo.dataDirectory(in: support, bundleIdentifier: "com.agentrelay.desktop").lastPathComponent,
                       "com.agentrelay.desktop")
        XCTAssertEqual(AppInfo.dataDirectory(in: support, bundleIdentifier: "com.agentrelay.desktop.dev").lastPathComponent,
                       "com.agentrelay.desktop.dev")
        XCTAssertEqual(AppInfo.dataDirectory(in: support, bundleIdentifier: "com.example.relay").lastPathComponent,
                       "com.example.relay")
    }

    func testTheDevelopmentBadgeNamesAnySiteButProduction() {
        XCTAssertNil(AppModel.developmentBadge(reported: "https://agentrelay.com", builtIn: "https://agentrelay.com"))
        XCTAssertEqual(AppModel.developmentBadge(reported: "http://localhost:3100", builtIn: "https://agentrelay.com"),
                       "http://localhost:3100")
        XCTAssertEqual(AppModel.developmentBadge(reported: "https://staging.example", builtIn: "https://agentrelay.com"),
                       "https://staging.example")
        // Nothing reported yet: the build's own site decides, whichever it
        // is; once the core has reported, its word wins over the build's.
        XCTAssertEqual(AppModel.developmentBadge(reported: nil, builtIn: "http://localhost:3100"), "http://localhost:3100")
        XCTAssertNil(AppModel.developmentBadge(reported: nil, builtIn: "https://agentrelay.com"))
        XCTAssertNil(AppModel.developmentBadge(reported: "https://agentrelay.com", builtIn: "http://localhost:3100"))
        XCTAssertEqual(AppModel.developmentBadge(reported: "https://staging.example", builtIn: "https://agentrelay.com"),
                       "https://staging.example")
    }

    func testALinkForAnotherSiteGetsTheRemedyAndNothingElseDoes() {
        XCTAssertEqual(
            AppModel.linkRefusal("That link connects to https://other.example, but this app is built for https://agentrelay.com."),
            "That link is for another Agent Relay site. Open it with the Agent Relay app for other.example, or ask for a link for this app's site (https://agentrelay.com)."
        )
        // The link's site is whatever the link carried: its host only, cut
        // short, never a path, query or user info; none at all when it is
        // no URL.
        let long = String(repeating: "a", count: 80) + ".example"
        XCTAssertEqual(
            AppModel.linkRefusal("That link connects to https://user:pw@\(long)/x?y, but this app is built for http://localhost:3100."),
            "That link is for another Agent Relay site. Open it with the Agent Relay app for \(String(repeating: "a", count: 64))…, or ask for a link for this app's site (http://localhost:3100)."
        )
        XCTAssertEqual(
            AppModel.linkRefusal("That link connects to ???, but this app is built for https://agentrelay.com."),
            "That link is for another Agent Relay site. Open it with that site's Agent Relay app, or ask for a link for this app's site (https://agentrelay.com)."
        )
        XCTAssertEqual(
            AppModel.linkRefusal("That is not an Agent Relay link this app opens."),
            "That is not an Agent Relay link this app opens."
        )
        XCTAssertEqual(
            AppModel.linkRefusal("Agent Relay is starting. Try again in a moment."),
            "Agent Relay is starting. Try again in a moment."
        )
    }
}
