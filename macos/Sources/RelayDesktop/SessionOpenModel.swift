import Foundation
import Observation
import RelayDesktopCore

extension SessionOpenEngine {
    /// The bundled helper (the core binary the LaunchAgent names). A run
    /// without a bundle (`swift run`) may name one in
    /// `AGENT_RELAY_SESSION_HELPER`; a bundled helper always wins.
    static let live = SessionOpenEngine(executable: liveHelper())

    static func liveHelper(bundleURL: URL = Bundle.main.bundleURL,
                           environment: [String: String] = ProcessInfo.processInfo.environment) -> URL? {
        if let bundled = LaunchAgentService.bundledHelper(in: bundleURL) { return bundled }
        guard let path = environment["AGENT_RELAY_SESSION_HELPER"], path.hasPrefix("/"),
              FileManager.default.isExecutableFile(atPath: path) else { return nil }
        return URL(fileURLWithPath: path)
    }
}

/// One session's Open button: what the helper's plan says it does, and the
/// follow-up requests its answers ask for (`choose` → `selected`, `confirm` →
/// `confirmed`, `take_over` → `take_over`, a pending launch → `release`,
/// `can_resume_default` → `use_default_terminal`). Every word the button,
/// its reason and its questions show comes from the helper: the answer's
/// own, or its fixed words (`SessionOpen.Strings`, `.fallback` when no
/// answer could be had). This decides only which answer is shown and which
/// request is sent. GTK's `models::SessionOpens` is the counterpart.
@MainActor @Observable final class SessionOpenModel {
    /// A question the person answers before the next request is sent.
    enum Prompt: Equatable {
        /// `confirm`: the plan's question, then `confirmed`.
        case confirm(String)
        /// `take_over`: the plan's question, then `take_over`.
        case takeOver(String)
        /// A pending launch: ask whether it was closed, then `release`.
        case launchGone
    }

    /// How often a shown button is looked at again while the app is in
    /// front. One `session-open plan` costs about 0.07 s and 0.15 s of CPU
    /// on an Apple-silicon Mac; every 30 s that is about 0.5 % of one core
    /// while a session is shown, and nothing while it is not. A look is
    /// also taken when the row appears, when the app comes to the front,
    /// and after every open or release (their own answers). GTK uses the
    /// same interval (`models::session_open::PLAN_EVERY`).
    static let pollInterval: Duration = .seconds(30)

    let source: String
    let sessionId: String
    let directory: String?
    let appDataDir: String
    /// The person's terminal choice (a contract terminal id); nil defers to
    /// the system's default terminal. Relay has no such setting on macOS.
    let defaultTerminal: String?
    /// The person's "Open with: Terminal" for this provider.
    var preferTerminal: Bool
    @ObservationIgnored let engine: SessionOpenEngine

    private(set) var plan: SessionOpen.Plan?
    private(set) var opening = false
    private(set) var releasing = false
    private(set) var error: String?
    private(set) var notice: String?
    private(set) var readError: String?
    /// An open failed where resuming in the default terminal would work.
    private(set) var permitsDefaultTerminal = false
    var prompt: Prompt?

    init(source: String, sessionId: String, directory: String?, appDataDir: String,
         defaultTerminal: String? = nil, preferTerminal: Bool = false, engine: SessionOpenEngine) {
        self.source = source
        self.sessionId = sessionId
        self.directory = directory
        self.appDataDir = appDataDir
        self.defaultTerminal = defaultTerminal
        self.preferTerminal = preferTerminal
        self.engine = engine
    }

    func request(useDefaultTerminal: Bool = false, confirmed: Bool = false, takeOver: Bool = false,
                 selected: String? = nil) -> SessionOpen.Request {
        SessionOpen.Request(source: source, sessionId: sessionId, appDataDir: appDataDir,
                            directory: directory.flatMap { $0.isEmpty ? nil : $0 },
                            useDefaultTerminal: useDefaultTerminal, preferTerminal: preferTerminal,
                            defaultTerminal: defaultTerminal, confirmed: confirmed, takeOver: takeOver,
                            selected: selected)
    }

    /// The helper's fixed words: the last answer's, else this build's copy.
    var strings: SessionOpen.Strings { plan?.strings ?? .fallback }
    private var busy: Bool { opening || releasing }
    var title: String { opening ? strings.opening : plan?.title ?? strings.openSession }
    var enabled: Bool { !busy && plan?.actionable == true }
    var choices: [SessionOpen.Choice]? { plan?.action == .choose ? plan?.choices : nil }
    /// Where a click goes is a terminal: the button shows the terminal icon.
    var showsTerminal: Bool { plan?.destination == .terminal }
    var reason: String? { plan?.message }
    var offersLaunchGone: Bool {
        guard let plan else { return false }
        return plan.pendingLaunch && plan.reasonCode == "launch_pending"
    }
    var offersDefaultResume: Bool {
        guard let plan, plan.canResumeDefault, !offersLaunchGone else { return false }
        return plan.action == .unavailable || permitsDefaultTerminal
    }

    /// One read-only look; safe whenever the row is shown. Skipped while an
    /// open or release runs: its own answer is the next plan.
    func refresh() async {
        guard !busy else { return }
        do {
            let fresh = try await engine.plan(request())
            guard !Task.isCancelled, !busy else { return }
            plan = fresh
            readError = nil
        } catch {
            guard !Task.isCancelled else { return }
            readError = Self.sentence(error, strings)
        }
    }

    static func sentence(_ error: Error, _ strings: SessionOpen.Strings) -> String {
        (error as? SessionOpenEngine.Failure)?.sentence(strings) ?? strings.planFailed
    }

    /// The button: what the plan says, after a question when it asks one.
    func press() async {
        guard enabled, let plan else { return }
        switch plan.action {
        case .confirm: prompt = .confirm(plan.question ?? plan.message ?? "")
        case .takeOver: prompt = .takeOver(plan.question ?? strings.takeOverDetail)
        case .choose: return
        default: await open()
        }
    }

    func choose(_ id: String) async { await open(selected: id) }

    func confirmNotOpenElsewhere() async {
        prompt = nil
        await open(confirmed: true)
    }

    func confirmTakeOver() async {
        prompt = nil
        await open(takeOver: true)
    }

    func askLaunchGone() { prompt = .launchGone }

    /// The earlier launch was closed: the helper releases its reservation
    /// and answers a fresh plan. Nothing is opened; the person clicks Open
    /// next.
    func confirmLaunchGone() async {
        prompt = nil
        guard !busy else { return }
        releasing = true
        defer { releasing = false }
        error = nil
        notice = nil
        do {
            plan = try await engine.release(request())
            readError = nil
        } catch {
            self.error = Self.sentence(error, strings)
            releasing = false
            await refresh()
        }
    }

    func resumeInDefaultTerminal() async { await open(useDefaultTerminal: true) }

    func open(useDefaultTerminal: Bool = false, confirmed: Bool = false, takeOver: Bool = false,
              selected: String? = nil) async {
        guard !busy else { return }
        opening = true
        defer { opening = false }
        error = nil
        notice = nil
        permitsDefaultTerminal = false
        let request = request(useDefaultTerminal: useDefaultTerminal, confirmed: confirmed,
                              takeOver: takeOver, selected: selected)
        do {
            let result = try await engine.open(request)
            if let after = result.plan { plan = after }
            let message = result.message.isEmpty ? nil : result.message
            switch result.outcome {
            case .verified:
                // Selected exactly: the session is in front, nothing to say.
                notice = result.precision == .exactConversation ? nil : message
            case .dispatchedUnverified:
                notice = message
            case .refused, .failed, .unknown:
                error = message
                permitsDefaultTerminal = plan?.canResumeDefault ?? false
            }
            if result.plan == nil {
                opening = false
                await refresh()
            }
        } catch {
            // Never retried: after a timeout the outcome is unknown, so the
            // person sees that, the button is enabled again, and a fresh
            // look decides it.
            self.error = Self.sentence(error, strings)
            opening = false
            await refresh()
        }
    }
}
