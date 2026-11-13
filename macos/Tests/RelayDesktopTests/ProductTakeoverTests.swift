import Foundation
import XCTest
@testable import RelayDesktop

final class ProductTakeoverTests: XCTestCase {
    static let own = "com.agentrelay.desktop"
    static let other = "com.example.relay"
    static let unmarked = "com.example.unmarked"
    /// A site profile built to coexist (`build-app.sh --coexist`): its own
    /// bundle id, no marker.
    static let coexisting = "com.agentrelay.desktop.devaws"

    func testNothingToReplaceLeavesThisProductAlone() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let ownAgent = try fixture.agent(id: Self.own, suffix: ".local", in: fixture.launchAgents)
        try fixture.data(for: Self.own)
        fixture.os.loaded = Set(ProductTakeover.labels(of: Self.own))
        fixture.os.running = [FakeApp(id: Self.own, url: fixture.ownBundle)]

        XCTAssertTrue(fixture.takeover.detect().isEmpty)
        XCTAssertEqual(fixture.takeover.run(), [])

        XCTAssertEqual(fixture.os.events, [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: ownAgent.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.ownBundle.path))
        XCTAssertEqual(fixture.os.loaded, Set(ProductTakeover.labels(of: Self.own)), "this product's core keeps running")
        XCTAssertTrue(fixture.os.running[0].running)
        XCTAssertTrue(fixture.os.domains.isEmpty)
    }

    func testAnEmptyMacHasNothingToDo() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        XCTAssertEqual(fixture.takeover.run(), [])
        XCTAssertEqual(fixture.os.events, [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.support.appendingPathComponent(Self.own).path))
    }

    func testDetectsOnlyMarkedLaunchAgentsAndBundles() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let local = try fixture.agent(id: Self.other, suffix: ".local", in: fixture.launchAgents)
        let session = try fixture.agent(id: Self.other, suffix: ".session", in: fixture.sessionServices)
        let bundle = try fixture.app(named: "Example", id: Self.other)
        try fixture.agent(id: Self.unmarked, suffix: ".local", in: fixture.launchAgents, marked: false)
        try fixture.app(named: "Unmarked", id: Self.unmarked, marked: false)
        // A marker under another product's label, or naming another bundle
        // than the one carrying it, is no family product either.
        try fixture.agent(id: Self.other, suffix: ".local", in: fixture.launchAgents,
                          label: "com.example.something.else", file: "stray.plist")
        try fixture.app(named: "Mismatched", id: "com.example.mismatched", marker: Self.other)

        let found = fixture.takeover.detect()

        XCTAssertEqual(Set(found.keys), [Self.other])
        XCTAssertEqual(Set(found[Self.other]?.plists.map(\.lastPathComponent) ?? []),
                       [local.lastPathComponent, session.lastPathComponent])
        XCTAssertEqual(found[Self.other]?.bundles.map(\.lastPathComponent), [bundle.lastPathComponent])
    }

    func testRunningAppsCountOnlyWithTheMarkerInTheirInfoPlist() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let marked = try fixture.app(named: "Example", id: Self.other, in: fixture.elsewhere)
        let unmarked = try fixture.app(named: "Unmarked", id: Self.unmarked, in: fixture.elsewhere, marked: false)
        fixture.os.running = [FakeApp(id: Self.other, url: marked), FakeApp(id: Self.unmarked, url: unmarked),
                              FakeApp(id: Self.own, url: fixture.ownBundle)]

        let found = fixture.takeover.detect()

        XCTAssertEqual(Set(found.keys), [Self.other])
        XCTAssertEqual(found[Self.other]?.running.count, 1)
        XCTAssertEqual(found[Self.other]?.bundles.map(\.lastPathComponent), ["Example.app"])
    }

    func testLaunchServicesCopiesOfADetectedProductNeedTheMarker() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.agent(id: Self.other, suffix: ".local", in: fixture.launchAgents)
        let copy = try fixture.app(named: "Example", id: Self.other, in: fixture.elsewhere)
        let unmarkedCopy = try fixture.app(named: "Example Old", id: Self.other, in: fixture.elsewhere, marked: false)
        let trashed = try fixture.app(named: "Example", id: Self.other, in: fixture.trash)
        let volumeTrash = fixture.root.appendingPathComponent("Volumes/External/.Trashes/501")
        try FileManager.default.createDirectory(at: volumeTrash, withIntermediateDirectories: true)
        let trashedOnVolume = try fixture.app(named: "Example", id: Self.other, in: volumeTrash)
        fixture.os.launchServices[Self.other] = [copy, unmarkedCopy, trashed, trashedOnVolume]

        let found = fixture.takeover.detect()

        XCTAssertEqual(found[Self.other]?.bundles.map(\.path), [copy.path])
    }

    func testTakeoverStopsTrashesQuitsRemovesThenCarriesData() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let plist = try fixture.agent(id: Self.other, suffix: ".local", in: fixture.launchAgents)
        let bundle = try fixture.app(named: "Example", id: Self.other)
        try fixture.data(for: Self.other)
        fixture.os.loaded = Set(["\(Self.other).core.local"] + ProductTakeover.labels(of: Self.own))
        fixture.os.running = [FakeApp(id: Self.other, url: bundle), FakeApp(id: Self.own, url: fixture.ownBundle)]

        XCTAssertEqual(fixture.takeover.run(), [Self.other])

        // Its core is stopped first, so its app is forced at once: a graceful
        // quit could only ask about keeping agents connected.
        XCTAssertEqual(fixture.os.events, [
            "bootout \(Self.other).core.local",
            "trash Example.app",
            "forceTerminate \(Self.other)",
            "remove \(plist.lastPathComponent)",
            "move \(Self.other) \(Self.own)"
        ])
        XCTAssertEqual(fixture.os.pausedFor, 0, "an unloaded core is not waited for")
        XCTAssertEqual(fixture.os.lockedPaths.map(\.path), [fixture.support.appendingPathComponent("AgentRelay/takeover.lock").path])
        XCTAssertEqual(fixture.os.lockReleases, 1, "the lock is held for the run only")
        XCTAssertEqual(fixture.os.loaded, Set(ProductTakeover.labels(of: Self.own)), "this product's core is never stopped")
        XCTAssertTrue(fixture.os.running[1].running)
        XCTAssertFalse(FileManager.default.fileExists(atPath: plist.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundle.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.ownBundle.path))
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.other])
    }

    func testAnAppWhoseCoreCouldNotBeStoppedIsAskedThenForcedAfterFiveSeconds() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let app = FakeApp(id: Self.other, url: bundle)
        app.quitsWhenAsked = false
        fixture.os.running = [app]
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.stuck = ["\(Self.other).core"]

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, [
            "bootout \(Self.other).core", "trash Example.app",
            "terminate \(Self.other)", "forceTerminate \(Self.other)",
            "bootout \(Self.other).core"
        ])
        XCTAssertGreaterThanOrEqual(fixture.os.pausedFor, 5, "it gets five seconds to quit")
        XCTAssertFalse(app.running)
    }

    func testAnotherTakeoverStillRunningAfterTheWaitSkipsThisLaunch() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        try fixture.data(for: Self.other)
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.running = [FakeApp(id: Self.other, url: bundle)]
        fixture.os.lockHeld = true

        XCTAssertEqual(fixture.takeover.takeOver(), ProductTakeover.Outcome(acted: [], replaced: false))

        XCTAssertEqual(fixture.os.lockWaits, [ProductTakeover.lockWait], "it waits for the other takeover, for a while")
        XCTAssertEqual(fixture.os.lockedPaths, [])
        XCTAssertEqual(fixture.os.events, [])
        XCTAssertEqual(fixture.os.loaded, ["\(Self.other).core"])
        XCTAssertTrue(fixture.os.running[0].running)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
        XCTAssertNil(fixture.os.domains[Self.own])
    }

    func testALaunchWhoseAppWasTrashedWhileItWaitedIsReplaced() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.running = [FakeApp(id: Self.other, url: bundle)]
        fixture.os.lockHeld = true
        fixture.os.releasedWhileWaiting = true
        let ownBundle = fixture.ownBundle
        fixture.os.whileWaiting = { _ = try? FileManager.default.removeItem(at: ownBundle) }

        XCTAssertEqual(fixture.takeover.takeOver(), ProductTakeover.Outcome(acted: [], replaced: true))

        XCTAssertEqual(fixture.os.events, [], "a replaced launch acts on nothing")
        XCTAssertEqual(fixture.os.lockReleases, 1)
        XCTAssertEqual(fixture.os.loaded, ["\(Self.other).core"])
        XCTAssertTrue(fixture.os.running[0].running)
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.path))
        XCTAssertNil(fixture.os.domains[Self.own])
    }

    func testALaunchWhoseCoreWasStoppedWhileItWaitedButWhoseAppStayedIsNotReplaced() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        fixture.os.loaded = ["\(Self.other).core", "\(Self.own).core"]
        fixture.os.running = [FakeApp(id: Self.other, url: bundle)]
        fixture.os.lockHeld = true
        fixture.os.releasedWhileWaiting = true
        // The other takeover stopped this app's core but could not move its
        // bundle to the Trash: this app survived it and does not quit.
        fixture.os.whileWaiting = { [weak os = fixture.os] in
            _ = os?.loaded.remove("\(ProductTakeoverTests.own).core")
        }

        XCTAssertEqual(fixture.takeover.takeOver(), ProductTakeover.Outcome(acted: [Self.other], replaced: false))

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "trash Example.app", "forceTerminate \(Self.other)"])
        XCTAssertEqual(fixture.os.lockReleases, 1)
        XCTAssertEqual(fixture.os.loaded, [])
        XCTAssertFalse(fixture.os.running[0].running)
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.ownBundle.path))
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.other])
    }

    func testALaunchThatSurvivedTheOtherTakeoverTakesOverOnceTheLockIsFree() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        fixture.os.loaded = ["\(Self.other).core", "\(Self.own).core"]
        fixture.os.running = [FakeApp(id: Self.other, url: bundle)]
        fixture.os.lockHeld = true
        fixture.os.releasedWhileWaiting = true

        XCTAssertEqual(fixture.takeover.takeOver(), ProductTakeover.Outcome(acted: [Self.other], replaced: false))

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "trash Example.app", "forceTerminate \(Self.other)"])
        XCTAssertEqual(fixture.os.lockedPaths.count, 1)
        XCTAssertEqual(fixture.os.lockReleases, 1, "the lock it waited for is held for the run, then released")
        XCTAssertEqual(fixture.os.loaded, ["\(Self.own).core"])
    }

    func testAnAppWhoseBundleCouldNotBeTrashedIsNotQuit() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"]]
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.running = [FakeApp(id: Self.other, url: bundle)]
        fixture.os.trashFailures = ["Example.app"]

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "trash Example.app"])
        XCTAssertTrue(fixture.os.running[0].running, "a product that stays installed is never quit")
        XCTAssertTrue(FileManager.default.fileExists(atPath: bundle.path))
        // It keeps its data and settings, and nothing is decided yet.
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.appendingPathComponent("store.json").path))
        XCTAssertEqual(fixture.os.domains[Self.other]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])
        let destination = fixture.support.appendingPathComponent(Self.own)
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.appendingPathComponent(ProductTakeover.dataMarkerName).path))

        // Once it can be moved to the Trash, its data comes over.
        fixture.os.trashFailures = []
        fixture.os.running[0].running = false
        fixture.os.events = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["trash Example.app", "move \(Self.other) \(Self.own)"])
        XCTAssertEqual(fixture.os.domains[Self.own]?["hiddenSessions.v1"] as? [String], ["a"])
    }

    func testAnAppThatStartedAfterThisOneIsNotQuit() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let app = FakeApp(id: Self.other, url: bundle)
        app.launchDate = fixture.takeover.launchedAt.addingTimeInterval(30)
        let earlier = try fixture.app(named: "Example Earlier", id: Self.other, in: fixture.elsewhere)
        let earlierApp = FakeApp(id: Self.other, url: earlier)
        earlierApp.launchDate = fixture.takeover.launchedAt.addingTimeInterval(-30)
        fixture.os.running = [app, earlierApp]

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["trash Example.app", "trash Example Earlier.app", "forceTerminate \(Self.other)"])
        XCTAssertTrue(app.running, "an app that started later is left to quit itself")
        XCTAssertFalse(earlierApp.running)
    }

    func testAnAppThatRefusesToQuitSurvivesAndKeepsItsData() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"]]
        let app = FakeApp(id: Self.other, url: bundle)
        app.quitsWhenAsked = false
        app.quitsWhenForced = false
        fixture.os.running = [app]
        fixture.os.loaded = ["\(Self.other).core"]

        fixture.takeover.run()

        // Moved to the Trash and its core stopped, but still running.
        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "trash Example.app", "forceTerminate \(Self.other)"])
        XCTAssertTrue(app.running)
        XCTAssertEqual(fixture.os.pausedFor, ProductTakeover.quitStep * Double(ProductTakeover.exitSteps),
                       "it is given a while to exit")
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.appendingPathComponent("store.json").path))
        XCTAssertEqual(fixture.os.domains[Self.other]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: fixture.support.appendingPathComponent(Self.own).appendingPathComponent(ProductTakeover.dataMarkerName).path))
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.other])
    }

    func testTheDataOfAnAppLeftRunningStaysItsOwn() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let bundle = try fixture.app(named: "Example", id: Self.other)
        let app = FakeApp(id: Self.other, url: bundle)
        app.launchDate = fixture.takeover.launchedAt.addingTimeInterval(30)
        fixture.os.running = [app]
        let source = try fixture.data(for: Self.other)
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"]]

        fixture.takeover.run()

        XCTAssertTrue(app.running)
        XCTAssertFalse(fixture.os.events.contains("move \(Self.other) \(Self.own)"))
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.path))
        XCTAssertNotNil(fixture.os.domains[Self.other])
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: fixture.support.appendingPathComponent(Self.own).appendingPathComponent(ProductTakeover.dataMarkerName).path))
    }

    func testDataAndSettingsCarryOverOnce() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        fixture.os.domains[Self.other] = [
            "hiddenSessions.v1": ["a"],
            "keepAgentsConnectedAfterQuit": false,
            "\(Self.other).core.startsAtLogin": false,
            "\(Self.other).core.usesLocalLaunchAgent": true,
            AppModel.registeredCoreKey: "theirs",
            "cachedAppUpdateRelease": Data()
        ]
        fixture.os.domains[Self.own] = ["keepAgentsConnectedAfterQuit": true]

        fixture.takeover.run()

        let destination = fixture.support.appendingPathComponent(Self.own)
        XCTAssertFalse(FileManager.default.fileExists(atPath: source.path))
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("store.json"), encoding: .utf8), Self.other)
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent(ProductTakeover.dataMarkerName), encoding: .utf8),
                       "\(Self.other)\n")
        let ours = try XCTUnwrap(fixture.os.domains[Self.own])
        XCTAssertEqual(ours["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertEqual(ours["keepAgentsConnectedAfterQuit"] as? Bool, true, "this product's own setting wins")
        XCTAssertEqual(ours["\(Self.own).core.startsAtLogin"] as? Bool, false)
        XCTAssertNil(ours["\(Self.other).core.usesLocalLaunchAgent"])
        XCTAssertNil(ours["\(Self.own).core.usesLocalLaunchAgent"])
        XCTAssertNil(ours[AppModel.registeredCoreKey])
        XCTAssertNil(ours["cachedAppUpdateRelease"])
        XCTAssertNil(fixture.os.domains[Self.other])

        // The same product reinstalled and used again: its data stays its own.
        try fixture.app(named: "Example", id: Self.other)
        let again = try fixture.data(for: Self.other)
        try FileManager.default.removeItem(at: destination.appendingPathComponent("store.json"))
        fixture.os.events = []

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["trash Example.app"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: again.path))
    }

    func testThisProductsOwnDataIsKept() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        let own = try fixture.data(for: Self.own)
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"]]

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["trash Example.app"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.path), "the other product's data is never deleted")
        XCTAssertEqual(try String(contentsOf: own.appendingPathComponent("store.json"), encoding: .utf8), Self.own)
        XCTAssertEqual(try String(contentsOf: own.appendingPathComponent(ProductTakeover.dataMarkerName), encoding: .utf8), "kept\n")
        XCTAssertNotNil(fixture.os.domains[Self.other])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])
    }

    func testAReplacedProductsCoreThatCameBackIsStoppedAgain() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.domains[Self.own] = [ProductTakeover.replacedKey: [Self.other]]
        fixture.os.loaded = ["\(Self.other).core", "\(Self.unmarked).core"] as Set

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core"])
        XCTAssertEqual(fixture.os.loaded, ["\(Self.unmarked).core"], "a core no marker named is never stopped")

        fixture.os.events = []
        fixture.os.pausedFor = 0
        fixture.takeover.run()
        XCTAssertEqual(fixture.os.events, [], "nothing left: running again does nothing")
        XCTAssertEqual(fixture.os.pausedFor, 0, "a remembered product whose core is not loaded costs no wait")
    }

    func testDataStaysWhileTheOtherCoreIsStillLoaded() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.agent(id: Self.other, suffix: "", in: fixture.launchAgents)
        try fixture.app(named: "Example", id: Self.other)
        let source = try fixture.data(for: Self.other)
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.stuck = ["\(Self.other).core"]
        let destination = fixture.support.appendingPathComponent(Self.own)

        fixture.takeover.run()

        XCTAssertFalse(fixture.os.events.contains("move \(Self.other) \(Self.own)"))
        XCTAssertTrue(FileManager.default.fileExists(atPath: source.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.appendingPathComponent(ProductTakeover.dataMarkerName).path),
                       "no marker: the next launch decides again")
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.other])

        // Next launch, the core lets go: the data comes over then.
        fixture.os.stuck = []
        fixture.os.events = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "move \(Self.other) \(Self.own)"])
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent(ProductTakeover.dataMarkerName), encoding: .utf8),
                       "\(Self.other)\n")
    }

    func testAMarkerThatIsNotABundleIDIsNoMarker() throws {
        for value in ["../../Library", "com.example/../../evil", "relay", "com example.relay", ".com.example", "com..example", ""] {
            XCTAssertNil(ProductTakeover.marker(in: [ProductTakeover.markerKey: value]), value)
        }
        XCTAssertEqual(ProductTakeover.marker(in: [ProductTakeover.markerKey: " com.example.relay-2\n"]), "com.example.relay-2")

        let fixture = try Fixture()
        defer { fixture.remove() }
        let traversal = "com.example/../../../evil"
        try fixture.agent(id: traversal, suffix: ".local", in: fixture.launchAgents, file: "evil.plist")
        try fixture.agent(id: "relay", suffix: ".local", in: fixture.launchAgents, file: "relay.plist")
        try fixture.app(named: "Evil", id: traversal)
        try fixture.app(named: "Plain", id: "relay")
        fixture.os.domains[Self.own] = [ProductTakeover.replacedKey: ["../../Library", traversal, "relay"]]
        fixture.os.loaded = ["../../Library.core", "relay.core"]

        XCTAssertTrue(fixture.takeover.detect().isEmpty)
        XCTAssertEqual(fixture.takeover.run(), [])
        XCTAssertEqual(fixture.os.events, [])
        XCTAssertEqual(fixture.os.loaded, ["../../Library.core", "relay.core"])
    }

    func testOnlyAReleaseBuildInstalledToKeepItsCoreRuns() {
        let own = Self.own
        XCTAssertTrue(ProductTakeover.shouldRun(marker: own, bundleIdentifier: own, installationProblem: nil))
        // Agent Relay Dev carries no marker.
        XCTAssertFalse(ProductTakeover.shouldRun(marker: nil, bundleIdentifier: "com.agentrelay.desktop.dev", installationProblem: nil))
        // Nor does a profile built to coexist, nor any build without one.
        XCTAssertFalse(ProductTakeover.shouldRun(marker: nil, bundleIdentifier: Self.coexisting, installationProblem: nil))
        XCTAssertFalse(ProductTakeover.shouldRun(marker: nil, bundleIdentifier: own, installationProblem: nil))
        XCTAssertFalse(ProductTakeover.shouldRun(marker: Self.other, bundleIdentifier: own, installationProblem: nil))
        XCTAssertFalse(ProductTakeover.shouldRun(marker: "../\(own)", bundleIdentifier: "../\(own)", installationProblem: nil))
        // Opened from a disk image, a temporary folder or the Trash.
        for problem in ["disk image", "temporary folder", "Trash"] {
            XCTAssertFalse(ProductTakeover.shouldRun(marker: own, bundleIdentifier: own, installationProblem: problem), problem)
        }
    }

    func testAnUnmarkedBuildDoesNothingAtLaunchBesideRunningProducts() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // Agent Relay and another product are installed, running, with
        // their cores loaded; a build with no marker of its own launches.
        let other = try fixture.app(named: "Example", id: Self.other)
        try fixture.agent(id: Self.other, suffix: "", in: fixture.launchAgents)
        try fixture.data(for: Self.own)
        try fixture.data(for: Self.other)
        let coexisting = try fixture.app(named: "Agent Relay Devaws", id: Self.coexisting, marked: false)
        fixture.os.loaded = Set(ProductTakeover.labels(of: Self.own) + ["\(Self.other).core"])
        fixture.os.running = [FakeApp(id: Self.own, url: fixture.ownBundle), FakeApp(id: Self.other, url: other),
                              FakeApp(id: Self.coexisting, url: coexisting)]
        var checkedInstallation = false
        var built = false

        let outcome = ProductTakeover.launch(marker: nil, bundleIdentifier: Self.coexisting,
                                             installationProblem: { checkedInstallation = true; return nil }) {
            built = true
            return ProductTakeover(bundleIdentifier: Self.coexisting, bundleURL: coexisting, homeDirectory: fixture.home,
                                   applicationDirectories: [fixture.applications], supportDirectory: fixture.support,
                                   system: fixture.os.system)
        }

        XCTAssertNil(outcome)
        XCTAssertFalse(checkedInstallation)
        XCTAssertFalse(built, "no takeover is made for an unmarked build")
        XCTAssertEqual(fixture.os.events, [])
        XCTAssertEqual(fixture.os.lockedPaths, [], "not even the takeover lock is taken")
        XCTAssertEqual(fixture.os.verified, [])
        XCTAssertEqual(fixture.os.loaded, Set(ProductTakeover.labels(of: Self.own) + ["\(Self.other).core"]))
        XCTAssertTrue(fixture.os.running.allSatisfy(\.running))
        for url in [fixture.ownBundle, other, coexisting] {
            XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), url.lastPathComponent)
        }
        XCTAssertTrue(fixture.os.domains.isEmpty)

        // A marked build in the same place does take over: the gate is the marker.
        let marked = ProductTakeover.launch(marker: Self.own, bundleIdentifier: Self.own,
                                            installationProblem: { nil }) { fixture.takeover }
        XCTAssertEqual(marked, ProductTakeover.Outcome(acted: [Self.other], replaced: false))
    }

    func testAMarkedProductLeavesAnUnmarkedBuildWithAnotherIDAlone() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // Installed in Applications, a copy elsewhere running, its bundled
        // LaunchAgent and fallback without a marker, its core loaded.
        let installed = try fixture.app(named: "Agent Relay Devaws", id: Self.coexisting, marked: false)
        let copy = try fixture.app(named: "Agent Relay Devaws", id: Self.coexisting, in: fixture.elsewhere, marked: false)
        let agent = try fixture.agent(id: Self.coexisting, suffix: "", in: fixture.launchAgents, marked: false)
        let fallback = try fixture.agent(id: Self.coexisting, suffix: ".local", in: fixture.launchAgents, marked: false)
        let data = try fixture.data(for: Self.coexisting)
        fixture.os.domains[Self.coexisting] = ["hiddenSessions.v1": ["a"]]
        fixture.os.launchServices[Self.coexisting] = [installed, copy]
        fixture.os.loaded = Set(ProductTakeover.labels(of: Self.coexisting) + ProductTakeover.labels(of: Self.own))
        fixture.os.running = [FakeApp(id: Self.coexisting, url: copy), FakeApp(id: Self.own, url: fixture.ownBundle)]

        XCTAssertTrue(fixture.takeover.detect().isEmpty)
        XCTAssertEqual(fixture.takeover.run(), [])

        XCTAssertEqual(fixture.os.events, [], "nothing of it is booted out, trashed, quit or removed")
        XCTAssertEqual(fixture.os.verified, [], "an unmarked bundle is not even a claim to check")
        XCTAssertEqual(fixture.os.loaded,
                       Set(ProductTakeover.labels(of: Self.coexisting) + ProductTakeover.labels(of: Self.own)))
        XCTAssertTrue(fixture.os.running.allSatisfy(\.running))
        for url in [installed, copy, agent, fallback, data] {
            XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), url.lastPathComponent)
        }
        XCTAssertEqual(fixture.os.domains[Self.coexisting]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own])
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.support.appendingPathComponent(Self.own).path))
    }

    func testAReplacedProductRebuiltUnmarkedIsLeftAloneWhileInstalled() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // This product once replaced a marked build with this id, whose data
        // it carried over and whose settings are still pending; the id is now
        // an unmarked build's, installed and running with its core loaded.
        let destination = try fixture.data(for: Self.own)
        let marker = destination.appendingPathComponent(ProductTakeover.dataMarkerName)
        try Data("\(Self.coexisting)\npending \(Self.coexisting)\n".utf8).write(to: marker)
        fixture.os.domains[Self.own] = [ProductTakeover.replacedKey: [Self.coexisting]]
        fixture.os.domains[Self.coexisting] = ["hiddenSessions.v1": ["a"]]
        let bundle = try fixture.app(named: "Agent Relay Devaws", id: Self.coexisting, marked: false)
        fixture.os.loaded = ["\(Self.coexisting).core"]
        fixture.os.running = [FakeApp(id: Self.coexisting, url: bundle)]

        XCTAssertEqual(fixture.takeover.run(), [])

        XCTAssertEqual(fixture.os.events, [], "its core is not booted out")
        XCTAssertEqual(fixture.os.loaded, ["\(Self.coexisting).core"])
        XCTAssertTrue(fixture.os.running[0].running)
        XCTAssertEqual(fixture.os.domains[Self.coexisting]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.coexisting)\npending \(Self.coexisting)\n")
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.coexisting],
                       "it stays remembered")

        // In the Trash but still running, it still holds the id: its core
        // is in use.
        let trashed = fixture.trash.appendingPathComponent(bundle.lastPathComponent)
        try FileManager.default.moveItem(at: bundle, to: trashed)
        fixture.os.running = [FakeApp(id: Self.coexisting, url: trashed)]
        XCTAssertEqual(fixture.takeover.run(), [])
        XCTAssertEqual(fixture.os.events, [], "a running build in the Trash keeps its core")
        XCTAssertEqual(fixture.os.domains[Self.coexisting]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])

        // Once that build is in the Trash and has quit, a core that comes
        // back under the id is the replaced product's again.
        fixture.os.running = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.coexisting).core"])
        XCTAssertEqual(fixture.os.domains[Self.own]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.coexisting])
    }

    func testTheMostRecentlyUsedDataIsCarriedOver() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let second = "com.example.second"
        let older = try fixture.data(for: Self.other)
        let newer = try fixture.data(for: second)
        try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSinceNow: -86_400)], ofItemAtPath: older.path)
        try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSinceNow: -60)], ofItemAtPath: newer.path)

        fixture.takeover.carryData(from: [Self.other, second])

        let destination = fixture.support.appendingPathComponent(Self.own)
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("store.json"), encoding: .utf8), second)
        XCTAssertTrue(FileManager.default.fileExists(atPath: older.path), "the older product's data stays")
        XCTAssertFalse(FileManager.default.fileExists(atPath: newer.path))
    }

    func testAnOwnDirectoryHoldingOnlyFinderStateCountsAsEmpty() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.data(for: Self.other)
        let destination = fixture.support.appendingPathComponent(Self.own)
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        try Data().write(to: destination.appendingPathComponent(".DS_Store"))

        fixture.takeover.carryData(from: [Self.other])

        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("store.json"), encoding: .utf8), Self.other)
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent(ProductTakeover.dataMarkerName), encoding: .utf8),
                       "\(Self.other)\n")
    }

    func testDataALinkPointsToElsewhereIsNotMoved() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let target = fixture.elsewhere.appendingPathComponent("data")
        try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
        try Data(Self.other.utf8).write(to: target.appendingPathComponent("store.json"))
        try FileManager.default.createSymbolicLink(at: fixture.support.appendingPathComponent(Self.other), withDestinationURL: target)

        fixture.takeover.carryData(from: [Self.other])

        XCTAssertEqual(fixture.os.events, [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: target.appendingPathComponent("store.json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.support.appendingPathComponent(Self.own).path))
    }

    func testDefaultsKeyedOnTheOtherLabelStayBehind() {
        let carried = ProductTakeover.carriedDefaults(
            ["\(Self.other).core.startsAtLogin": true, "\(Self.other).core.local.fallbackPolicyRevision": 1,
             "\(Self.other).coreish": "kept", "AppAppearance": "dark", ProductTakeover.replacedKey: [Self.own]],
            from: Self.other, to: Self.own)
        XCTAssertEqual(carried.count, 3)
        XCTAssertEqual(carried["\(Self.own).core.startsAtLogin"] as? Bool, true)
        XCTAssertEqual(carried["\(Self.other).coreish"] as? String, "kept")
        XCTAssertEqual(carried["AppAppearance"] as? String, "dark")
    }

    func testSettingsLeftBehindByAnInterruptedCarryOverFollowAtTheNextLaunch() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // The launch that moved the data stopped before the settings.
        let destination = try fixture.data(for: Self.own)
        let marker = destination.appendingPathComponent(ProductTakeover.dataMarkerName)
        try Data("\(Self.other)\npending \(Self.other)\n".utf8).write(to: marker)
        fixture.os.domains[Self.own] = [ProductTakeover.replacedKey: [Self.other]]
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"], "\(Self.other).core.startsAtLogin": false]
        XCTAssertEqual(fixture.takeover.pendingSettingsSources(), [Self.other])
        // While the product is still installed, its settings stay its own.
        try fixture.app(named: "Example", id: Self.other)
        fixture.os.trashFailures = ["Example.app"]

        fixture.takeover.run()

        XCTAssertNotNil(fixture.os.domains[Self.other])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\npending \(Self.other)\n")

        fixture.os.trashFailures = []
        fixture.os.events = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["trash Example.app"], "the data is not moved again")
        let ours = try XCTUnwrap(fixture.os.domains[Self.own])
        XCTAssertEqual(ours["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertEqual(ours["\(Self.own).core.startsAtLogin"] as? Bool, false)
        XCTAssertNil(fixture.os.domains[Self.other])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\n")
        XCTAssertEqual(fixture.takeover.pendingSettingsSources(), [])

        // Complete: settings that appear later are not merged.
        fixture.os.domains[Self.other] = ["AppAppearance": "dark"]
        fixture.takeover.run()
        XCTAssertNil(fixture.os.domains[Self.own]?["AppAppearance"])
        XCTAssertNotNil(fixture.os.domains[Self.other])
    }

    func testPendingSettingsWaitWhileThatProductsCoreIsLoaded() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // Its data moved, the trashed product's core came back through its
        // retained launchd registration; no bundle or LaunchAgent is left.
        let destination = try fixture.data(for: Self.own)
        let marker = destination.appendingPathComponent(ProductTakeover.dataMarkerName)
        try Data("\(Self.other)\npending \(Self.other)\n".utf8).write(to: marker)
        fixture.os.domains[Self.own] = [ProductTakeover.replacedKey: [Self.other]]
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["a"]]
        fixture.os.loaded = ["\(Self.other).core"]
        fixture.os.stuck = ["\(Self.other).core"]

        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core", "bootout \(Self.other).core"])
        XCTAssertEqual(fixture.os.domains[Self.other]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.own]?["hiddenSessions.v1"])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\npending \(Self.other)\n")

        // Once its core lets go, its settings follow.
        fixture.os.stuck = []
        fixture.os.events = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, ["bootout \(Self.other).core"])
        XCTAssertEqual(fixture.os.domains[Self.own]?["hiddenSessions.v1"] as? [String], ["a"])
        XCTAssertNil(fixture.os.domains[Self.other])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\n")
    }

    func testSettingsTheReplacedProductHadPendingStayPendingAheadOfItsOwn() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let third = "com.example.third"
        try fixture.app(named: "Example", id: Self.other)
        // The other product carried over the third's data, then stopped
        // before the third's settings followed.
        let source = try fixture.data(for: Self.other)
        try Data("\(third)\npending \(third)\n".utf8).write(to: source.appendingPathComponent(ProductTakeover.dataMarkerName))
        fixture.os.domains[Self.other] = ["hiddenSessions.v1": ["b"], "AppAppearance": "light", ProductTakeover.replacedKey: [third]]
        fixture.os.domains[third] = ["AppAppearance": "dark"]

        fixture.takeover.run()

        let destination = fixture.support.appendingPathComponent(Self.own)
        let marker = destination.appendingPathComponent(ProductTakeover.dataMarkerName)
        XCTAssertEqual(fixture.os.events, ["trash Example.app", "move \(Self.other) \(Self.own)"])
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("store.json"), encoding: .utf8), Self.other)
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\npending \(third)\npending \(Self.other)\n")
        XCTAssertEqual(fixture.os.domains[Self.own]?[ProductTakeover.replacedKey] as? [String], [Self.other, third])
        XCTAssertNotNil(fixture.os.domains[Self.other])
        XCTAssertNotNil(fixture.os.domains[third])

        // The next launch merges the other's settings, then the third's.
        fixture.os.events = []
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, [])
        let ours = try XCTUnwrap(fixture.os.domains[Self.own])
        XCTAssertEqual(ours["AppAppearance"] as? String, "light", "the newest pending product wins a shared key")
        XCTAssertEqual(ours["hiddenSessions.v1"] as? [String], ["b"])
        XCTAssertNil(fixture.os.domains[third])
        XCTAssertNil(fixture.os.domains[Self.other])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\n")
    }

    func testAPendingProductThisOneNeverReplacedIsIgnored() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let finder = "com.apple.finder"
        let destination = try fixture.data(for: Self.own)
        let marker = destination.appendingPathComponent(ProductTakeover.dataMarkerName)
        try Data("\(Self.other)\npending \(finder)\n".utf8).write(to: marker)
        fixture.os.domains[finder] = ["FXPreferredViewStyle": "Nlsv"]

        XCTAssertEqual(fixture.takeover.pendingSettingsSources(), [])
        fixture.takeover.run()

        XCTAssertEqual(fixture.os.events, [])
        XCTAssertEqual(fixture.os.domains[finder]?["FXPreferredViewStyle"] as? String, "Nlsv")
        XCTAssertNil(fixture.os.domains[Self.own]?["FXPreferredViewStyle"])
        XCTAssertEqual(try String(contentsOf: marker, encoding: .utf8), "\(Self.other)\n")
    }

    func testAMarkerInABundleNotSignedAsThatProductIsNotTrusted() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        // A counterfeit claiming the development build's id, which carries
        // no marker of its own.
        let dev = "com.agentrelay.desktop.dev"
        let counterfeit = try fixture.app(named: "Counterfeit", id: dev, signed: false)
        let running = try fixture.app(named: "Counterfeit Running", id: dev, in: fixture.elsewhere, signed: false)
        let agent = try fixture.agent(id: dev, suffix: ".local", in: fixture.launchAgents)
        // A LaunchAgent with no app of its product anywhere.
        let orphan = try fixture.agent(id: Self.other, suffix: ".local", in: fixture.launchAgents)
        let data = try fixture.data(for: dev)
        fixture.os.domains[dev] = ["hiddenSessions.v1": ["a"]]
        fixture.os.loaded = ["\(dev).core.local", "\(Self.other).core.local"]
        fixture.os.running = [FakeApp(id: dev, url: running)]
        fixture.os.launchServices[dev] = [counterfeit, running]

        XCTAssertTrue(fixture.takeover.detect().isEmpty)
        XCTAssertEqual(Set(fixture.os.verified), ["Counterfeit.app \(dev)", "Counterfeit Running.app \(dev)"])
        XCTAssertEqual(fixture.os.verified.count, 2, "each bundle is checked once")
        XCTAssertEqual(fixture.takeover.run(), [])

        XCTAssertEqual(fixture.os.events, [])
        XCTAssertEqual(fixture.os.loaded, ["\(dev).core.local", "\(Self.other).core.local"])
        XCTAssertTrue(fixture.os.running[0].running)
        for url in [counterfeit, running, agent, orphan, data] {
            XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), url.lastPathComponent)
        }
        XCTAssertNotNil(fixture.os.domains[dev])
        XCTAssertNil(fixture.os.domains[Self.own])
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.support.appendingPathComponent(Self.own).path))

        // Signed as that product, the same bundles and LaunchAgent count.
        fixture.os.unsigned = []
        let found = fixture.takeover.detect()
        XCTAssertEqual(Set(found.keys), [dev])
        XCTAssertEqual(found[dev]?.plists.map(\.lastPathComponent), [agent.lastPathComponent])
        XCTAssertEqual(Set(found[dev]?.bundles.map(\.lastPathComponent) ?? []), ["Counterfeit.app", "Counterfeit Running.app"])
    }

    // MARK: Fakes

    final class FakeApp {
        let id: String
        let url: URL
        var running = true
        var quitsWhenAsked = true
        var quitsWhenForced = true
        var launchDate: Date?
        init(id: String, url: URL) { self.id = id; self.url = url }
    }

    final class FakeOS {
        var events: [String] = []
        var loaded: Set<String> = []
        var running: [FakeApp] = []
        var launchServices: [String: [URL]] = [:]
        var domains: [String: [String: Any]] = [:]
        /// Seconds the takeover paused for in all.
        var pausedFor: TimeInterval = 0
        /// Labels launchd refuses to unload.
        var stuck: Set<String> = []
        /// Bundle names the Trash refuses.
        var trashFailures: Set<String> = []
        /// Another process holds the takeover lock.
        var lockHeld = false
        /// The other process releases the lock while this one waits for it.
        var releasedWhileWaiting = false
        /// What the other process's takeover does while this one waits.
        var whileWaiting: (() -> Void)?
        /// How long each wait for the lock was allowed to last.
        var lockWaits: [TimeInterval] = []
        var lockedPaths: [URL] = []
        var lockReleases = 0
        /// Bundle names whose signature is not Agent Relay's.
        var unsigned: Set<String> = []
        /// Every signature check: "<bundle name> <claimed id>".
        var verified: [String] = []
        let trash: URL

        init(trash: URL) { self.trash = trash }

        var system: ProductTakeover.System {
            ProductTakeover.System(
                launchctl: { self.launchctl($0) },
                runningApplications: {
                    self.running.map { app in
                        ProductTakeover.RunningApp(
                            bundleIdentifier: app.id, bundleURL: app.url, launchDate: app.launchDate,
                            isRunning: { app.running },
                            terminate: {
                                self.events.append("terminate \(app.id)")
                                if app.quitsWhenAsked { app.running = false }
                            },
                            forceTerminate: {
                                self.events.append("forceTerminate \(app.id)")
                                if app.quitsWhenForced { app.running = false }
                            })
                    }
                },
                applicationURLs: { self.launchServices[$0] ?? [] },
                removeItem: { url in
                    self.events.append("remove \(url.lastPathComponent)")
                    try FileManager.default.removeItem(at: url)
                },
                trashItem: { url in
                    self.events.append("trash \(url.lastPathComponent)")
                    if self.trashFailures.contains(url.lastPathComponent) { throw CocoaError(.fileWriteNoPermission) }
                    try FileManager.default.moveItem(
                        at: url, to: self.trash.appendingPathComponent("\(UUID().uuidString)-\(url.lastPathComponent)"))
                },
                moveItem: { from, to in
                    self.events.append("move \(from.lastPathComponent) \(to.lastPathComponent)")
                    try FileManager.default.moveItem(at: from, to: to)
                },
                pause: { self.pausedFor += $0 },
                readDefaults: { self.domains[$0] },
                writeDefaults: { domain, name in self.domains[name] = domain },
                removeDefaults: { self.domains[$0] = nil },
                lock: { url, wait in
                    if self.lockHeld {
                        guard wait > 0 else { return nil }
                        self.lockWaits.append(wait)
                        self.whileWaiting?()
                        guard self.releasedWhileWaiting else { return nil }
                    }
                    self.lockedPaths.append(url)
                    return { self.lockReleases += 1 }
                },
                verifyProduct: { url, id in
                    self.verified.append("\(url.lastPathComponent) \(id)")
                    return !self.unsigned.contains(url.lastPathComponent)
                })
        }

        func launchctl(_ arguments: [String]) -> (success: Bool, output: String) {
            guard arguments.count == 2, let label = arguments[1].split(separator: "/").last.map(String.init) else {
                return (false, "unexpected command")
            }
            switch arguments[0] {
            case "print":
                return (loaded.contains(label), "")
            case "bootout":
                events.append("bootout \(label)")
                guard !stuck.contains(label) else { return (false, "Boot-out failed: 5: Input/output error") }
                return (loaded.remove(label) != nil, "")
            default:
                return (false, "unexpected command")
            }
        }
    }

    final class Fixture {
        let root: URL
        let home: URL
        let applications: URL
        let elsewhere: URL
        let trash: URL
        let support: URL
        let launchAgents: URL
        let sessionServices: URL
        let ownBundle: URL
        let os: FakeOS
        let takeover: ProductTakeover

        init() throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent("takeover-\(UUID())")
            let home = root.appendingPathComponent("home")
            let applications = root.appendingPathComponent("Applications")
            let support = home.appendingPathComponent("Library/Application Support")
            let os = FakeOS(trash: home.appendingPathComponent(".Trash"))
            let ownBundle = applications.appendingPathComponent("Agent Relay.app")
            self.root = root
            self.home = home
            self.applications = applications
            elsewhere = root.appendingPathComponent("Elsewhere")
            trash = os.trash
            self.support = support
            launchAgents = home.appendingPathComponent("Library/LaunchAgents")
            sessionServices = support.appendingPathComponent("AgentRelay/SessionServices")
            self.ownBundle = ownBundle
            self.os = os
            takeover = ProductTakeover(bundleIdentifier: ProductTakeoverTests.own, bundleURL: ownBundle, homeDirectory: home,
                                       applicationDirectories: [applications], supportDirectory: support, system: os.system)
            for directory in [applications, elsewhere, trash, support, launchAgents, sessionServices] {
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            }
            try app(named: "Agent Relay", id: ProductTakeoverTests.own)
        }

        @discardableResult
        func app(named name: String, id: String, in directory: URL? = nil, marked: Bool = true, marker: String? = nil,
                 signed: Bool = true) throws -> URL {
            let bundle = (directory ?? applications).appendingPathComponent("\(name).app")
            if !signed { os.unsigned.insert(bundle.lastPathComponent) }
            let contents = bundle.appendingPathComponent("Contents")
            try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
            var info: [String: Any] = ["CFBundleIdentifier": id, "CFBundleName": name]
            if marked { info[ProductTakeover.markerKey] = marker ?? id }
            try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
                .write(to: contents.appendingPathComponent("Info.plist"))
            return bundle
        }

        @discardableResult
        func agent(id: String, suffix: String, in directory: URL, marked: Bool = true,
                   label: String? = nil, file: String? = nil) throws -> URL {
            let label = label ?? "\(id).core\(suffix)"
            let url = directory.appendingPathComponent(file ?? "\(label).plist")
            var plist: [String: Any] = ["Label": label, "Program": "/Applications/x.app/Contents/Helpers/Helper",
                                        "AssociatedBundleIdentifiers": [id]]
            if marked { plist[ProductTakeover.markerKey] = id }
            try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0).write(to: url)
            return url
        }

        @discardableResult
        func data(for id: String) throws -> URL {
            let directory = support.appendingPathComponent(id)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try Data(id.utf8).write(to: directory.appendingPathComponent("store.json"))
            return directory
        }

        func remove() { try? FileManager.default.removeItem(at: root) }
    }
}
