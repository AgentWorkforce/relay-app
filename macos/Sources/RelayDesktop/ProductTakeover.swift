import AppKit
import Foundation
import OSLog
import Security

/// One Agent Relay app per Mac. A site profile build has its own bundle id,
/// so it would otherwise install beside Agent Relay with its own core, data
/// directory and login item. Launched where another Agent Relay product is
/// installed, this build takes over silently instead: it stops the other
/// product's core, moves its app to the Trash, quits it, removes its
/// LaunchAgent and carries the person's data over once. Plain Agent Relay
/// does the same to a profile build. One takeover runs at a time on a Mac
/// (an exclusive lock on a file every product shares; a launch that finds
/// it held waits for it, then quits if it was itself replaced meanwhile),
/// and an app whose bundle stays in place, or that started after this one,
/// is never quit, so two products that both survive cannot keep quitting
/// each other. A product that survives keeps its data and settings.
///
/// A product is recognised only by its marker, never by its name:
/// `build-app.sh` writes `AgentRelayProduct` (the bundle id) into a release
/// build's Info.plist and its core's LaunchAgent, and `LaunchAgentService`
/// copies it into the fallback LaunchAgent it writes. A marker is trusted
/// only in a bundle signed by Agent Relay's team under that bundle id
/// (`System.verifyProduct`), and a LaunchAgent's only for a product one of
/// whose bundles is. A bundle, running app or property list without a
/// trusted marker is never touched; Agent Relay Dev (a development build)
/// and a site profile built to coexist (`build-app.sh --coexist`, the
/// profile's `DESKTOP_COEXIST`) carry none, so they neither take over nor
/// are taken over, and an Agent Relay installed before this marker existed
/// is not found until it has updated to a marked build. A product this one
/// replaced earlier whose bundle id an unmarked build now has is left alone
/// while that build is installed or running (`unmarkedBuilds`). A marker
/// that is not a bundle id (`isProductID`) is no marker. Every step finds
/// its object first, so running this again, or on a Mac with nothing to
/// replace, does nothing.
struct ProductTakeover {
    /// The key, in Info.plist and in the core's LaunchAgent, whose value is
    /// the bundle id of the Agent Relay product that wrote it.
    static let markerKey = "AgentRelayProduct"
    /// In this product's data directory once its data was settled: its
    /// first line names the product the data was carried over from, or says
    /// `kept`. Never moved over again. Each following line is
    /// `settingsPending` and the bundle id of a product whose settings are
    /// still to be merged, oldest first, and every launch finishes them
    /// first; once none remain only the first line is left.
    static let dataMarkerName = ".carried-over-from"
    static let settingsPending = "pending"
    /// The products this one replaced, kept in its own defaults. A replaced
    /// product's core that comes back (launchd keeps its registration until
    /// its bundle leaves the Trash) is stopped again at the next launch.
    static let replacedKey = "AgentRelayProduct.replaced"
    /// Defaults a replaced product kept that describe that build, not the
    /// person: its registered core, its update cache from its own releases.
    static let uncarriedDefaults: Set<String> = [
        AppModel.registeredCoreKey, replacedKey,
        "cachedAppUpdateRelease", "cachedAppUpdateWriter", "cachedAppUpdateIncludesBeta", "lastAppUpdateCheck"
    ]

    private static let logger = Logger(subsystem: AppInfo.subsystem, category: "takeover")

    /// A running app, as `NSRunningApplication` reports it.
    struct RunningApp {
        var bundleIdentifier: String?
        var bundleURL: URL?
        /// When it started; nil when macOS does not say.
        var launchDate: Date?
        var isRunning: () -> Bool
        var terminate: () -> Void
        var forceTerminate: () -> Void
    }

    /// OS calls are replaceable so tests never stop, quit or trash anything real.
    struct System {
        var launchctl: ([String]) -> (success: Bool, output: String)
        var runningApplications: () -> [RunningApp]
        /// Every copy of an app LaunchServices knows by bundle id.
        var applicationURLs: (String) -> [URL]
        var removeItem: (URL) throws -> Void
        var trashItem: (URL) throws -> Void
        var moveItem: (URL, URL) throws -> Void
        var pause: (TimeInterval) -> Void
        /// A defaults domain by name: read, replace, remove.
        var readDefaults: (String) -> [String: Any]?
        var writeDefaults: ([String: Any], String) -> Void
        var removeDefaults: (String) -> Void
        /// Takes an exclusive lock on the file, created if missing, waiting
        /// up to the interval for another process to release it (0: not at
        /// all). Returns its release, or nil when it is still held.
        var lock: (URL, TimeInterval) -> (() -> Void)?
        /// Whether the bundle at the URL is the Agent Relay product with this
        /// bundle id: its code signature is valid, Agent Relay's team signed
        /// it (`AppUpdateInstaller.signingRequirement`) and it is signed
        /// under that identifier, which seals the Info.plist carrying the
        /// marker. Ad-hoc, development and other teams' signatures fail.
        var verifyProduct: (URL, String) -> Bool

        static func live() -> Self {
            let defaults = UserDefaults.standard
            return Self(
                launchctl: LaunchAgentService.runLaunchctl,
                runningApplications: {
                    NSWorkspace.shared.runningApplications.map { app -> RunningApp in
                        let pid = app.processIdentifier
                        // isTerminated is updated on the main run loop, which
                        // the wait below blocks; the process table is not.
                        return RunningApp(bundleIdentifier: app.bundleIdentifier, bundleURL: app.bundleURL,
                                          launchDate: app.launchDate,
                                          isRunning: { kill(pid, 0) == 0 },
                                          terminate: { _ = app.terminate() },
                                          forceTerminate: { _ = app.forceTerminate() })
                    }
                },
                applicationURLs: { NSWorkspace.shared.urlsForApplications(withBundleIdentifier: $0) },
                removeItem: { try FileManager.default.removeItem(at: $0) },
                trashItem: { try FileManager.default.trashItem(at: $0, resultingItemURL: nil) },
                moveItem: { try FileManager.default.moveItem(at: $0, to: $1) },
                pause: { Thread.sleep(forTimeInterval: $0) },
                readDefaults: { defaults.persistentDomain(forName: $0) },
                writeDefaults: { defaults.setPersistentDomain($0, forName: $1) },
                removeDefaults: { defaults.removePersistentDomain(forName: $0) },
                lock: { url, wait in
                    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                             withIntermediateDirectories: true)
                    let descriptor = open(url.path, O_RDWR | O_CREAT | O_CLOEXEC, 0o644)
                    guard descriptor >= 0 else { return nil }
                    let deadline = Date().addingTimeInterval(wait)
                    while flock(descriptor, LOCK_EX | LOCK_NB) != 0 {
                        guard Date() < deadline else {
                            _ = close(descriptor)
                            return nil
                        }
                        Thread.sleep(forTimeInterval: 0.1)
                    }
                    return {
                        _ = flock(descriptor, LOCK_UN)
                        _ = close(descriptor)
                    }
                },
                verifyProduct: { url, id in
                    guard ProductTakeover.isProductID(id) else { return false }
                    var staticCode: SecStaticCode?
                    guard SecStaticCodeCreateWithPath(url as CFURL, SecCSFlags(), &staticCode) == errSecSuccess,
                          let code = staticCode else { return false }
                    var parsed: SecRequirement?
                    let text = "\(AppUpdateInstaller.signingRequirement) and identifier \"\(id)\""
                    guard SecRequirementCreateWithString(text as CFString, SecCSFlags(), &parsed) == errSecSuccess,
                          let requirement = parsed else { return false }
                    let flags = SecCSFlags(rawValue: SecCSFlags.RawValue(kSecCSCheckAllArchitectures)
                                                    | SecCSFlags.RawValue(kSecCSStrictValidate))
                    return SecStaticCodeCheckValidity(code, flags, requirement) == errSecSuccess
                })
        }
    }

    /// What a takeover did at launch.
    struct Outcome: Equatable {
        /// The products it acted on.
        var acted: [String] = []
        /// Another product's takeover, waited for, replaced this one: its
        /// bundle went to the Trash. It must quit before its core registers.
        /// One whose bundle stayed survived that takeover and does not quit,
        /// even when its core was booted out.
        var replaced = false
    }

    /// What was found of one other product.
    struct Found {
        var plists: [URL] = []
        var bundles: [URL] = []
        var running: [RunningApp] = []
    }

    let bundleIdentifier: String
    let bundleURL: URL
    let homeDirectory: URL
    let applicationDirectories: [URL]
    let supportDirectory: URL
    /// When this process started: a running product that started later is
    /// never quit by this one.
    let launchedAt: Date
    let system: System

    init(bundleIdentifier: String, bundleURL: URL, homeDirectory: URL,
         applicationDirectories: [URL]? = nil, supportDirectory: URL? = nil,
         launchedAt: Date = Date(), system: System = .live()) {
        self.bundleIdentifier = bundleIdentifier
        self.bundleURL = bundleURL
        self.homeDirectory = homeDirectory
        self.applicationDirectories = applicationDirectories
            ?? [URL(fileURLWithPath: "/Applications", isDirectory: true),
                homeDirectory.appendingPathComponent("Applications", isDirectory: true)]
        self.supportDirectory = supportDirectory
            ?? homeDirectory.appendingPathComponent("Library/Application Support", isDirectory: true)
        self.launchedAt = launchedAt
        self.system = system
    }

    /// At launch, before the model reads its defaults or registers the core:
    /// only a marked release build installed where it can keep its core
    /// (`LaunchAgentService.installationProblem`: not a disk image, a
    /// temporary folder or the Trash) replaces anything.
    /// It runs synchronously: the core must not register before it is done.
    /// It only waits when it stopped another product's core in this run, or
    /// when another product's takeover holds the lock; if that one replaced
    /// this app, this app quits here.
    static func runAtLaunch() {
        let own = AppInfo.bundleIdentifier
        let outcome = launch(marker: Bundle.main.object(forInfoDictionaryKey: markerKey) as? String,
                             bundleIdentifier: own,
                             installationProblem: { LaunchAgentService().installationProblem }) {
            ProductTakeover(bundleIdentifier: own, bundleURL: Bundle.main.bundleURL,
                            homeDirectory: FileManager.default.homeDirectoryForCurrentUser,
                            supportDirectory: AppInfo.applicationSupport,
                            launchedAt: NSRunningApplication.current.launchDate ?? Date())
        }
        if outcome?.replaced == true {
            logger.notice("another Agent Relay product replaced this one while it waited; quitting before its core registers")
            exit(0)
        }
    }

    /// What a launch with this Info.plist marker does (`runAtLaunch`): nil
    /// when this copy replaces nothing, else the outcome of the takeover
    /// `make` builds. A build whose marker is not its own bundle id, such as
    /// an unmarked one (Agent Relay Dev, a profile built to coexist), stops
    /// here: its installation is not checked, no lock is taken and nothing
    /// is looked for or touched.
    static func launch(marker: String?, bundleIdentifier own: String,
                       installationProblem: () -> String?, make: () -> ProductTakeover) -> Outcome? {
        guard marker == own else { return nil }
        let problem = installationProblem()
        guard shouldRun(marker: marker, bundleIdentifier: own, installationProblem: problem) else {
            if let problem {
                logger.notice("not replacing other Agent Relay products from this copy: \(problem, privacy: .public)")
            }
            return nil
        }
        return make().takeOver()
    }

    /// Whether this copy replaces anything: a release build (its marker is
    /// its own bundle id; a development build has none) installed where it
    /// can keep its core (no installation problem: not a disk image, a
    /// temporary folder or the Trash).
    static func shouldRun(marker: String?, bundleIdentifier: String, installationProblem: String?) -> Bool {
        guard let marker = marker, isProductID(marker), marker == bundleIdentifier else { return false }
        return installationProblem == nil
    }

    // MARK: Detection

    /// The core's LaunchAgent labels of the product with `id`: the bundled
    /// job and the two fallbacks `LaunchAgentService` writes.
    static func labels(of id: String) -> [String] {
        ["\(id).core", "\(id).core.local", "\(id).core.session"]
    }

    static func propertyList(at url: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return (try? PropertyListSerialization.propertyList(from: data, format: nil)) as? [String: Any]
    }

    /// A bundle id: dot-separated parts of letters, digits and hyphens. Only
    /// such a value is used as a path component, a launchctl label or a
    /// defaults domain.
    static func isProductID(_ value: String) -> Bool {
        value.range(of: "^[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$", options: .regularExpression) != nil
    }

    static func marker(in plist: [String: Any]) -> String? {
        guard let value = plist[markerKey] as? String else { return nil }
        let id = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return isProductID(id) ? id : nil
    }

    /// The product a LaunchAgent belongs to: its marker, when its label is
    /// that product's core.
    static func product(ofLaunchAgentAt url: URL) -> String? {
        guard let plist = propertyList(at: url), let id = marker(in: plist),
              let label = plist["Label"] as? String, labels(of: id).contains(label) else { return nil }
        return id
    }

    /// The product an app bundle says it is: its marker, when it is the
    /// bundle's id. Not trusted until `System.verifyProduct` confirms it.
    static func product(ofBundleAt url: URL) -> String? {
        guard let info = propertyList(at: url.appendingPathComponent("Contents/Info.plist")),
              let id = marker(in: info), info["CFBundleIdentifier"] as? String == id else { return nil }
        return id
    }

    private func isOther(_ id: String) -> Bool {
        id != bundleIdentifier && Set(Self.labels(of: id)).isDisjoint(with: Self.labels(of: bundleIdentifier))
    }

    private func canonicalPath(_ url: URL) -> String {
        url.standardizedFileURL.resolvingSymlinksInPath().path
    }

    private func isOwnBundle(_ url: URL) -> Bool { canonicalPath(url) == canonicalPath(bundleURL) }

    /// The home Trash, or a volume's.
    private static func isInTrash(_ url: URL) -> Bool {
        let components = url.standardizedFileURL.pathComponents
        return components.contains(".Trash") || components.contains(".Trashes")
    }

    private func contents(of directory: URL) -> [URL] {
        (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
    }

    private var launchAgentDirectories: [URL] {
        [homeDirectory.appendingPathComponent("Library/LaunchAgents", isDirectory: true),
         homeDirectory.appendingPathComponent("Library/Application Support/AgentRelay/SessionServices", isDirectory: true)]
    }

    /// Other marked products: by their running apps and their app bundles
    /// in the Applications folders and wherever LaunchServices knows a copy,
    /// each signed as that product (`System.verifyProduct`), and by their
    /// LaunchAgents once one of their bundles is. This product's own never
    /// count.
    func detect() -> [String: Found] {
        var found: [String: Found] = [:]
        var verdicts: [String: Bool] = [:]
        func isSigned(_ url: URL, as id: String) -> Bool {
            let path = canonicalPath(url)
            if let verdict = verdicts[path] { return verdict }
            let verdict = system.verifyProduct(url, id)
            if !verdict {
                Self.logger.notice("ignoring \(url.path, privacy: .public): it says it is \(id, privacy: .public) but is not signed as that Agent Relay product")
            }
            verdicts[path] = verdict
            return verdict
        }
        // A LaunchAgent's marker is only a claim: it counts once a signed
        // bundle of the same product is found below.
        var agents: [String: [URL]] = [:]
        for directory in launchAgentDirectories {
            for url in contents(of: directory) where url.pathExtension == "plist" {
                guard let id = Self.product(ofLaunchAgentAt: url), isOther(id) else { continue }
                agents[id, default: []].append(url)
            }
        }
        for app in system.runningApplications() {
            guard let url = app.bundleURL, !isOwnBundle(url), let id = Self.product(ofBundleAt: url),
                  app.bundleIdentifier == id, isOther(id), isSigned(url, as: id) else { continue }
            found[id, default: Found()].running.append(app)
        }
        func addBundle(_ url: URL, expecting expected: String? = nil) {
            guard url.pathExtension == "app", !isOwnBundle(url), !Self.isInTrash(url),
                  let id = Self.product(ofBundleAt: url), isOther(id), expected == nil || expected == id else { return }
            let path = canonicalPath(url)
            let known = found[id]?.bundles.contains(where: { canonicalPath($0) == path }) ?? false
            guard !known, isSigned(url, as: id) else { return }
            found[id, default: Found()].bundles.append(url)
        }
        for directory in applicationDirectories {
            for url in contents(of: directory) { addBundle(url) }
        }
        for app in found.values.flatMap(\.running) { if let url = app.bundleURL { addBundle(url) } }
        for id in Set(found.keys).union(agents.keys).union(rememberedIDs()).sorted() {
            for url in system.applicationURLs(id) { addBundle(url, expecting: id) }
        }
        for (id, urls) in agents {
            guard found[id] != nil else {
                Self.logger.notice("ignoring the LaunchAgents of \(id, privacy: .public): no app signed as that Agent Relay product was found")
                continue
            }
            found[id, default: Found()].plists.append(contentsOf: urls)
        }
        return found
    }

    private func rememberedIDs() -> [String] {
        ((system.readDefaults(bundleIdentifier)?[Self.replacedKey] as? [String]) ?? [])
            .filter { Self.isProductID($0) && isOther($0) }
    }

    /// Whether the bundle at the URL is a build with this bundle id that
    /// carries no marker of its own: Agent Relay Dev, or a profile built to
    /// coexist.
    static func isUnmarkedBuild(ofBundleAt url: URL, id: String) -> Bool {
        guard let info = propertyList(at: url.appendingPathComponent("Contents/Info.plist")) else { return false }
        return info["CFBundleIdentifier"] as? String == id && marker(in: info) == nil
    }

    /// Of these products this one replaced, those whose bundle id an
    /// unmarked build now has: one in the Applications folders or wherever
    /// LaunchServices knows a copy, outside the Trash, or one running, even
    /// from the Trash (its core is still in use). Such a
    /// build is not a product and its core has the replaced product's
    /// labels, so while it is there that core is not stopped and the
    /// replaced product's pending settings are not merged. It stays
    /// remembered: once the build is gone, a core of the replaced product
    /// that comes back is stopped again.
    private func unmarkedBuilds(of ids: Set<String>) -> Set<String> {
        guard !ids.isEmpty else { return [] }
        let installed = applicationDirectories.flatMap { contents(of: $0) }
        let running = system.runningApplications()
        return ids.filter { id in
            let stored = (installed + system.applicationURLs(id)).filter { !Self.isInTrash($0) }
            let urls = stored + running.filter { $0.bundleIdentifier == id }.compactMap(\.bundleURL)
            return urls.contains { url in
                url.pathExtension == "app" && Self.isUnmarkedBuild(ofBundleAt: url, id: id)
            }
        }
    }

    // MARK: Takeover

    /// The file every Agent Relay product locks for the length of a takeover.
    var lockURL: URL {
        supportDirectory.appendingPathComponent("AgentRelay/takeover.lock", isDirectory: false)
    }

    /// How long an app whose core could not be stopped gets to quit before
    /// it is forced: `pause` is called `quitSteps` times for `quitStep`.
    static let quitStep: TimeInterval = 0.25
    static let quitSteps = 20
    /// How long an app made to quit gets to exit before it counts as left
    /// running: `pause` is called up to `exitSteps` times for `quitStep`.
    static let exitSteps = 8
    /// How long a launch waits for another product's takeover to finish:
    /// longer than one takeover's quit grace and launchd wait.
    static let lockWait: TimeInterval = 30

    /// Replaces every other product found, then carries the person's data
    /// over. Returns the products it acted on (`takeOver`).
    @discardableResult
    func run() -> [String] { takeOver().acted }

    /// Replaces every other product found, then carries the person's data
    /// over. When another product holds the takeover lock, waits for it
    /// (up to `lockWait`), then reports whether that takeover replaced this
    /// app; this app replaces nothing if it was, or if the lock stays held.
    func takeOver() -> Outcome {
        let release: () -> Void
        if let free = system.lock(lockURL, 0) {
            release = free
        } else {
            Self.logger.notice("another Agent Relay product is taking over; waiting for it to finish")
            let acquired = system.lock(lockURL, Self.lockWait)
            if wasReplaced() {
                acquired?()
                return Outcome(acted: [], replaced: true)
            }
            guard let waited = acquired else {
                Self.logger.notice("the takeover lock is still held or unavailable; not replacing anything this launch")
                return Outcome()
            }
            release = waited
        }
        defer { release() }
        return Outcome(acted: replaceOthers(), replaced: false)
    }

    /// After waiting for another product's takeover: this app's bundle is
    /// gone from where it was opened (moved to the Trash). A takeover that
    /// could not trash it leaves it running with its data, so a core it
    /// booted out does not count: this app survived and takes over in turn.
    private func wasReplaced() -> Bool {
        guard FileManager.default.fileExists(atPath: bundleURL.path) else {
            Self.logger.notice("this app was moved to the Trash by another Agent Relay product")
            return true
        }
        return false
    }

    private func replaceOthers() -> [String] {
        let found = detect()
        let remembered = rememberedIDs()
        // A remembered product whose id an unmarked build now has is that
        // build's: it is not replaced and its settings stay pending.
        let unmarked = unmarkedBuilds(of: Set(remembered).subtracting(found.keys))
        for id in unmarked.sorted() {
            Self.logger.notice("leaving \(id, privacy: .public) alone: an unmarked build of it is installed or running")
        }
        // A product whose settings are still pending is replaced like any
        // other, so its core is booted out and whether it survives is known
        // before its defaults are merged.
        let ids = Set(found.keys).union(remembered).union(pendingSettingsSources()).subtracting(unmarked).sorted()
        var acted: [String] = []
        var settled: [String] = []
        var unsettled: [String] = []
        for id in ids {
            let outcome = replace(id, found[id] ?? Found())
            if outcome.acted { acted.append(id) }
            if outcome.survived {
                Self.logger.notice("\(id, privacy: .public) stays installed or running; its data and settings stay its own")
                unsettled.append(id)
            } else if outcome.coreUnloaded {
                settled.append(id)
            } else {
                unsettled.append(id)
            }
        }
        finishCarryingSettings(deferring: Set(unsettled).union(unmarked))
        // A product that stays installed or running, or whose core is still
        // loaded, may still use its data: nothing is carried over and no
        // marker is written, so a later launch decides.
        if let busy = unsettled.first(where: { hasDataDirectory($0) }) {
            Self.logger.notice("\(busy, privacy: .public) still runs, stays installed or has its core loaded; its data is settled at a later launch")
        } else {
            carryData(from: settled)
        }
        remember(found.keys.sorted())
        return acted
    }

    /// Adds products to the ones this product replaced (`replacedKey`),
    /// keeping those already there.
    private func remember(_ ids: [String]) {
        let remembered = rememberedIDs()
        guard !Set(ids).isSubset(of: remembered) else { return }
        var ours = system.readDefaults(bundleIdentifier) ?? [:]
        ours[Self.replacedKey] = Set(remembered).union(ids).sorted()
        system.writeDefaults(ours, bundleIdentifier)
    }

    /// Stops the core, trashes the bundles, quits the app and removes the
    /// LaunchAgents of one other product, in that order. `acted` is true
    /// when anything was there to act on; `coreUnloaded` when none of its
    /// core's labels is loaded afterwards; `survived` when a bundle of it
    /// could not be moved to the Trash, an app of it was left running, or
    /// an app of it made to quit had not exited `exitSteps` pauses later.
    private func replace(_ id: String, _ found: Found) -> (acted: Bool, coreUnloaded: Bool, survived: Bool) {
        let first = bootOut(id)
        var acted = first.attempted
        // Where the running apps are, taken before their bundles move.
        let runningPaths = found.running.map { $0.bundleURL.map(canonicalPath) }
        var trashed: Set<String> = []
        var survived = false
        var quitting: [RunningApp] = []
        for bundle in found.bundles {
            acted = true
            let path = canonicalPath(bundle)
            do {
                try system.trashItem(bundle)
                trashed.insert(path)
                Self.logger.notice("moved \(id, privacy: .public) at \(bundle.path, privacy: .public) to the Trash")
            } catch {
                survived = true
                Self.logger.error("could not move \(bundle.path, privacy: .public) to the Trash: \(error.localizedDescription, privacy: .public)")
            }
        }
        let coreStopped = !isAnyLoaded(id)
        for (app, path) in zip(found.running, runningPaths) where app.isRunning() {
            // An app whose bundle stays where it was is a product that
            // survives this takeover: quitting it would only start it and
            // this one quitting each other at every login.
            let inTrash = app.bundleURL.map(Self.isInTrash) ?? false
            guard inTrash || path.map({ trashed.contains($0) }) == true else {
                survived = true
                Self.logger.notice("leaving \(id, privacy: .public) running: its app could not be moved to the Trash")
                continue
            }
            if let started = app.launchDate, started > launchedAt {
                survived = true
                Self.logger.notice("leaving \(id, privacy: .public) running: it started after this app")
                continue
            }
            acted = true
            quit(app, id, coreStopped: coreStopped)
            quitting.append(app)
        }
        // Quitting can register its core again; stop that one too, and wait
        // for launchd to let it go (it gives a job five seconds), but only
        // when a core was stopped in this run: nothing loaded costs no wait.
        let second = bootOut(id)
        if second.attempted { acted = true }
        let coreUnloaded = (first.unloaded || second.unloaded) ? waitForCore(id) : !isAnyLoaded(id)
        for plist in found.plists {
            acted = true
            do {
                try system.removeItem(plist)
                Self.logger.notice("removed the LaunchAgent \(plist.lastPathComponent, privacy: .public)")
            } catch {
                Self.logger.error("could not remove the LaunchAgent \(plist.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
            }
        }
        // macOS offers no call that removes another app's login item; it
        // stays listed in Login Items until its bundle leaves the Trash.
        // An app that refused to quit, or has not exited yet, may still use
        // its data and settings: they stay its own until a later launch.
        for app in quitting where !hasExited(app) {
            survived = true
            Self.logger.error("\(id, privacy: .public) is still running after it was made to quit; its data and settings stay its own")
        }
        return (acted, coreUnloaded, survived)
    }

    /// True once the app has exited, waiting up to `exitSteps` pauses.
    private func hasExited(_ app: RunningApp) -> Bool {
        for _ in 0..<Self.exitSteps {
            guard app.isRunning() else { return true }
            system.pause(Self.quitStep)
        }
        return !app.isRunning()
    }

    /// Quits another product's app. With its core already stopped it is
    /// forced at once: a graceful quit of an Agent Relay app can ask the
    /// person whether to keep agents connected (`QuitConfirmationPanel`),
    /// a question about a core that is gone, in an app being replaced.
    /// Otherwise it is asked to quit and forced after `quitSteps` pauses.
    private func quit(_ app: RunningApp, _ id: String, coreStopped: Bool) {
        if coreStopped {
            Self.logger.notice("quitting another Agent Relay product, \(id, privacy: .public), its core already stopped")
            app.forceTerminate()
            return
        }
        Self.logger.notice("quitting another Agent Relay product, \(id, privacy: .public)")
        app.terminate()
        for _ in 0..<Self.quitSteps {
            guard app.isRunning() else { return }
            system.pause(Self.quitStep)
        }
        if app.isRunning() {
            Self.logger.notice("forcing \(id, privacy: .public) to quit")
            app.forceTerminate()
        }
    }

    private func isLoaded(_ label: String) -> Bool {
        system.launchctl(["print", "gui/\(getuid())/\(label)"]).success
    }

    private func isAnyLoaded(_ id: String) -> Bool {
        Self.labels(of: id).contains(where: isLoaded)
    }

    /// Unloads the other product's core jobs that launchd has loaded. Only
    /// its labels: this product's core is never touched. `attempted` when a
    /// label was loaded, `unloaded` when launchd accepted a bootout.
    private func bootOut(_ id: String) -> (attempted: Bool, unloaded: Bool) {
        var attempted = false
        var unloaded = false
        for label in Self.labels(of: id) where isLoaded(label) {
            attempted = true
            let result = system.launchctl(["bootout", "gui/\(getuid())/\(label)"])
            if result.success {
                unloaded = true
                Self.logger.notice("unloaded the core of another Agent Relay product, \(label, privacy: .public)")
            } else {
                Self.logger.error("could not unload \(label, privacy: .public): \(result.output, privacy: .public)")
            }
        }
        return (attempted, unloaded)
    }

    /// True once none of the product's core labels is loaded.
    private func waitForCore(_ id: String) -> Bool {
        for _ in 0..<120 {
            guard isAnyLoaded(id) else { return true }
            system.pause(0.05)
        }
        guard isAnyLoaded(id) else { return true }
        Self.logger.error("the core of \(id, privacy: .public) has not stopped yet")
        return false
    }

    // MARK: Data

    private var dataMarkerURL: URL {
        supportDirectory.appendingPathComponent(bundleIdentifier, isDirectory: true)
            .appendingPathComponent(Self.dataMarkerName)
    }

    /// A `.carried-over-from` file: the product named on its first line,
    /// and the products on its `settingsPending` lines, oldest first.
    static func readMarker(at url: URL) -> (origin: String, pending: [String])? {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        let lines = text.components(separatedBy: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        let prefix = settingsPending + " "
        let pending = lines.dropFirst().filter { $0.hasPrefix(prefix) }
            .map { String($0.dropFirst(prefix.count)).trimmingCharacters(in: .whitespaces) }
        return (lines.first ?? "", pending)
    }

    /// The text of a `.carried-over-from` file (`dataMarkerName`).
    static func markerText(origin: String, pending: [String]) -> String {
        ([origin] + pending.map { "\(settingsPending) \($0)" }).joined(separator: "\n") + "\n"
    }

    /// The products whose data was moved here but whose settings have not
    /// followed yet (the launch that moved it was interrupted, or a product
    /// was unsettled), oldest first. Only products this one remembers
    /// replacing (`replacedKey`) count: a marker naming any other domain
    /// is never honoured.
    func pendingSettingsSources() -> [String] {
        guard let marker = Self.readMarker(at: dataMarkerURL) else { return [] }
        let remembered = Set(rememberedIDs())
        return marker.pending.filter { remembered.contains($0) }
    }

    /// Carries over the settings of the products whose data was already
    /// moved, newest first, except a product still installed, running or
    /// with its core loaded, which stays pending while the others complete.
    /// `carryDefaults` only adds keys this product does not have yet, so
    /// this product's own values always win, and among the pending products
    /// the newest, merged first, wins a key both carry: it replaced the
    /// older ones and would have kept its own values. A pending product this
    /// one does not remember replacing is ignored and dropped.
    private func finishCarryingSettings(deferring unsettled: Set<String>) {
        guard let marker = Self.readMarker(at: dataMarkerURL), !marker.pending.isEmpty else { return }
        let remembered = Set(rememberedIDs())
        var deferred: Set<String> = []
        for source in marker.pending.reversed() {
            guard remembered.contains(source) else {
                Self.logger.error("ignoring the pending settings of \(source, privacy: .public): not a product this one replaced")
                continue
            }
            guard !unsettled.contains(source), !isAnyLoaded(source) else {
                Self.logger.notice("the settings of \(source, privacy: .public) follow at a later launch")
                deferred.insert(source)
                continue
            }
            carryDefaults(from: source)
        }
        let remaining = marker.pending.filter { deferred.contains($0) }
        do {
            try Data(Self.markerText(origin: marker.origin, pending: remaining).utf8).write(to: dataMarkerURL)
        } catch {
            Self.logger.error("could not record the carried-over settings: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Merges the other product's settings, then marks the carry-over
    /// complete: the marker keeps only the product's id.
    private func carrySettings(from source: String) {
        carryDefaults(from: source)
        do {
            try Data(Self.markerText(origin: source, pending: []).utf8).write(to: dataMarkerURL)
        } catch {
            Self.logger.error("could not record the settings of \(source, privacy: .public) as carried over: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Moves the most recently used other product's data directory to this
    /// product's, once, and only when this product has none yet, then its
    /// settings. The core migrates what it finds there when it starts, as
    /// after an update. The marker records the decision either way, so it
    /// is never made again; until the settings have followed it says so,
    /// and the next launch finishes them (`finishCarryingSettings`). The
    /// source is remembered as replaced before its data moves. Settings the
    /// source itself still had pending, in the marker its directory brings,
    /// stay pending ahead of its own (and are remembered too) when the
    /// source had recorded them as replaced; then every pending product's
    /// settings follow at the next launch, newest first.
    func carryData(from ids: [String]) {
        let fileManager = FileManager.default
        let destination = supportDirectory.appendingPathComponent(bundleIdentifier, isDirectory: true)
        let marker = dataMarkerURL
        guard !fileManager.fileExists(atPath: marker.path) else { return }
        let candidates = ids.filter { Self.isProductID($0) && isOther($0) }
        let sources = candidates.compactMap { id -> (id: String, url: URL, modified: Date)? in
            guard hasDataDirectory(id) else { return nil }
            let url = supportDirectory.appendingPathComponent(id, isDirectory: true)
            // Only the product's own directory in Application Support is
            // moved, never one a link there points to elsewhere.
            let resolved = url.resolvingSymlinksInPath()
            guard resolved.lastPathComponent == id,
                  canonicalPath(resolved.deletingLastPathComponent()) == canonicalPath(supportDirectory) else {
                Self.logger.error("not carrying the data of \(id, privacy: .public) over: it is not in Application Support")
                return nil
            }
            let modified = (try? fileManager.attributesOfItem(atPath: url.path)[.modificationDate] as? Date) ?? .distantPast
            return (id, url, modified)
        }
        guard let source = sources.max(by: { $0.modified < $1.modified }) else { return }
        let existing = (try? fileManager.contentsOfDirectory(atPath: destination.path))?
            .filter { $0 != ".DS_Store" } ?? []
        if !existing.isEmpty {
            Self.logger.notice("keeping this product's data; \(source.id, privacy: .public)'s stays where it is")
            try? Data("kept\n".utf8).write(to: marker)
            return
        }
        // The directory may bring the marker of the source's own carry-over,
        // naming products whose settings it had not merged yet. They stay
        // pending, but only those the source recorded as replaced.
        let theirReplaced = Set((system.readDefaults(source.id)?[Self.replacedKey] as? [String]) ?? [])
        var inherited: [String] = []
        for id in Self.readMarker(at: source.url.appendingPathComponent(Self.dataMarkerName))?.pending ?? [] {
            guard Self.isProductID(id), isOther(id), id != source.id, theirReplaced.contains(id),
                  !inherited.contains(id) else {
                Self.logger.error("ignoring the pending settings of \(id, privacy: .public) in the data of \(source.id, privacy: .public): not a product it replaced")
                continue
            }
            inherited.append(id)
        }
        let pending = inherited + [source.id]
        // Remembered before anything moves, so a pending product is always
        // one this product replaced (`pendingSettingsSources`).
        remember(pending)
        do {
            if fileManager.fileExists(atPath: destination.path) { try system.removeItem(destination) }
            try fileManager.createDirectory(at: supportDirectory, withIntermediateDirectories: true)
            try system.moveItem(source.url, destination)
            try Data(Self.markerText(origin: source.id, pending: pending).utf8).write(to: marker)
            Self.logger.notice("carried the data of \(source.id, privacy: .public) over")
        } catch {
            Self.logger.error("could not carry the data of \(source.id, privacy: .public) over: \(error.localizedDescription, privacy: .public)")
            return
        }
        guard inherited.isEmpty else {
            Self.logger.notice("the settings of \(source.id, privacy: .public) and the products it replaced follow at the next launch")
            return
        }
        carrySettings(from: source.id)
    }

    private func hasDataDirectory(_ id: String) -> Bool {
        guard Self.isProductID(id) else { return false }
        var isDirectory: ObjCBool = false
        let url = supportDirectory.appendingPathComponent(id, isDirectory: true)
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    /// The other product's settings this product does not have yet. Its
    /// core's login choice is renamed to this product's label; anything else
    /// keyed on its label, and what describes that build, stays behind.
    static func carriedDefaults(_ theirs: [String: Any], from other: String, to own: String) -> [String: Any] {
        let otherLabel = "\(other).core"
        var carried: [String: Any] = [:]
        for (key, value) in theirs where !uncarriedDefaults.contains(key) {
            if key == "\(otherLabel).startsAtLogin" {
                carried["\(own).core.startsAtLogin"] = value
            } else if key == otherLabel || key.hasPrefix(otherLabel + ".") {
                continue
            } else {
                carried[key] = value
            }
        }
        return carried
    }

    private func carryDefaults(from other: String) {
        guard let theirs = system.readDefaults(other), !theirs.isEmpty else { return }
        var ours = system.readDefaults(bundleIdentifier) ?? [:]
        for (key, value) in Self.carriedDefaults(theirs, from: other, to: bundleIdentifier) where ours[key] == nil {
            ours[key] = value
        }
        system.writeDefaults(ours, bundleIdentifier)
        system.removeDefaults(other)
        Self.logger.notice("carried the settings of \(other, privacy: .public) over")
    }
}
