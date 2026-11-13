import Foundation
import os

/// Cold-start timing, readable as signpost intervals in Instruments and as log
/// lines in Console.app (subsystem com.agentworkforce.relayipad, category
/// startup). It records step names and durations only: never an account,
/// workspace, token, agent name or message.
@MainActor enum StartupTrace {
    static let logger = Logger(subsystem: "com.agentworkforce.relayipad", category: "startup")
    private static let signposter = OSSignposter(subsystem: "com.agentworkforce.relayipad", category: "startup")
    private static var launchedAt: TimeInterval?
    private static var rosterShown = false

    struct Step {
        let name: StaticString
        let state: OSSignpostIntervalState
        let start: TimeInterval
    }

    /// Called once from the app delegate; later steps are timed from here.
    static func launched() {
        guard launchedAt == nil else { return }
        launchedAt = ProcessInfo.processInfo.systemUptime
        signposter.emitEvent("launched")
        logger.info("launched")
    }

    static func begin(_ name: StaticString) -> Step {
        Step(name: name, state: signposter.beginInterval(name, id: signposter.makeSignpostID()),
             start: ProcessInfo.processInfo.systemUptime)
    }

    static func end(_ step: Step) {
        signposter.endInterval(step.name, step.state)
        let now = ProcessInfo.processInfo.systemUptime
        let label = String(describing: step.name)
        let took = now - step.start
        let elapsed = sinceLaunch(now)
        logger.info("\(label, privacy: .public) took \(took, format: .fixed(precision: 3), privacy: .public)s, \(elapsed, format: .fixed(precision: 3), privacy: .public)s after launch")
    }

    /// The first roster of the process reached the lists.
    static func firstRosterShown() {
        guard !rosterShown else { return }
        rosterShown = true
        signposter.emitEvent("firstRosterShown")
        let elapsed = sinceLaunch(ProcessInfo.processInfo.systemUptime)
        logger.info("first roster shown \(elapsed, format: .fixed(precision: 3), privacy: .public)s after launch")
    }

    private static func sinceLaunch(_ now: TimeInterval) -> Double {
        launchedAt.map { now - $0 } ?? -1
    }
}
