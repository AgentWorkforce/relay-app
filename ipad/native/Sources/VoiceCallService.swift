import AVFoundation
import CallKit
import Foundation

/// Owns the active voice call for the whole app, independent of any view, so
/// closing the call screen, navigating away, backgrounding or locking the
/// device keeps the call. The call screen only shows and controls it.
@MainActor final class VoiceCallService: ObservableObject {
    static let shared = VoiceCallService()

    @Published private(set) var current: VoiceCallController?
    /// Whether the full call screen is showing; when false a call bar is.
    @Published var screenPresented = false
    /// The current call ended while its screen was closed, so its reason has
    /// not been seen; the call bar shows it until dismissed.
    @Published private(set) var endedWhileHidden = false

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Starts a call with the agent, or shows the call already in progress.
    /// A call that fell back to on-device voice is replaced, so calling again
    /// asks GPT-Live again instead of showing why the old call couldn't use it.
    func call(_ agent: RelayAgent, client: RelayClient, rejoining: Bool = false) {
        guard VoiceCallFeature.isEnabled else { return }
        var replacing: VoiceCallController?
        if let current, current.phase != .ended {
            guard current.fellBack, !rejoining else {
                VoiceCallLog.logger.info("Call: showing the call in progress with \(current.agent.name, privacy: .public) (asked for \(agent.name, privacy: .public))")
                screenPresented = true
                return
            }
            VoiceCallLog.logger.info("Call: replacing the on-device call with \(current.agent.name, privacy: .public)")
            replacing = current
        }
        VoiceCallLog.logger.info("Call: new call with \(agent.name, privacy: .public)\(rejoining ? " (rejoin)" : "", privacy: .public)")
        let snapshot = rejoining ? VoiceCallSnapshot.load(defaults) : nil
        let call = VoiceCallController(agent: agent, client: client, rejoining: snapshot)
        call.onEnded = { [weak self, weak call] in
            guard let self, let call, self.current === call else { return }
            VoiceCallSnapshot.clear(self.defaults)
            if !self.screenPresented { self.endedWhileHidden = true }
        }
        call.onSnapshot = { [weak self] snapshot in
            guard let self else { return }
            snapshot.save(self.defaults)
        }
        current = call
        endedWhileHidden = false
        screenPresented = true
        Task {
            if let replacing { await replacing.endReleasingAudio() }
            await call.start()
        }
    }

    /// The person dismissed the ended call's reason (the bar's Dismiss), or a
    /// sign-out left nothing to show: the ended call is forgotten.
    func dismissEnded() {
        endedWhileHidden = false
        if current?.phase == .ended { current = nil }
    }

    /// The call screen closed. An ended call whose end the screen showed is
    /// forgotten; one kept for the bar (ended unseen, or ended by a
    /// workspace switch that closed the screen) stays until dismissed.
    func screenClosed() {
        guard let current, VoiceCallBarPolicy.forgetsOnScreenClose(ended: current.phase == .ended, endedWhileHidden: endedWhileHidden) else { return }
        self.current = nil
    }

    /// Ends the current call for a cause that leaves nothing to resume (a
    /// sign-out or workspace switch) and closes its screen, so no empty call
    /// sheet outlives the session. With `keepReason` the ended call stays
    /// for the bar to show why (a workspace switch); without it the call is
    /// forgotten (a sign-out: nothing is kept over the sign-in screen).
    func endCurrent(_ cause: VoiceCallEndCause, keepReason: Bool = false) {
        current?.end(cause)
        screenPresented = false
        if keepReason, current != nil {
            endedWhileHidden = true
        } else {
            current = nil
            endedWhileHidden = false
        }
    }

    /// After a relaunch (a new build, an update or iOS ending the app), rejoins
    /// a call that was still active, if its agent is on the roster.
    func rejoinIfNeeded(client: RelayClient, roster: [RelayAgent], now: Date = Date()) {
        Task { await Self.reportLeftoverUsage(client: client) }
        guard current == nil, let snapshot = VoiceCallSnapshot.load(defaults) else { return }
        guard snapshot.isRecent(now: now),
              let agent = roster.first(where: { $0.id == snapshot.agentID || $0.name == snapshot.agentName }) else {
            if !snapshot.isRecent(now: now) { VoiceCallSnapshot.clear(defaults) }
            return
        }
        call(agent, client: client, rejoining: true)
    }
}

extension VoiceCallService {
    private static var reportsInFlight: Set<String> = []

    /// Reports GPT-Live sessions this device still holds: finished ones whose
    /// report failed, and ones an earlier run of the app left open. Each report
    /// ends its session on the broker and frees its slot.
    static func reportLeftoverUsage(client: RelayClient, ledger: VoiceLiveUsageLedger = VoiceLiveUsageLedger()) async {
        for (sessionID, seconds) in ledger.reportable where !reportsInFlight.contains(sessionID) {
            reportsInFlight.insert(sessionID)
            defer { reportsInFlight.remove(sessionID) }
            if await client.reportVoiceLiveUsage(sessionID: sessionID, seconds: seconds) {
                ledger.remove(sessionID)
            }
        }
    }
}

/// What is saved while a call is active so the app can rejoin it after the
/// process is replaced.
struct VoiceCallSnapshot: Codable, Equatable {
    static let key = "relay.voiceCall.active"
    /// Rejoin only calls that were active within this window.
    static let rejoinWindow: TimeInterval = 15 * 60

    var agentID: String
    var agentName: String
    var updatedAt: Date
    /// Recent call text, used to seed a resumed GPT-Live session.
    var history: [Turn]
    /// Messages already in the conversation, so replies that arrive while
    /// the app relaunches are still read.
    var knownMessageIDs: [String]?

    struct Turn: Codable, Equatable {
        var fromPerson: Bool
        var text: String
    }

    func isRecent(now: Date = Date()) -> Bool {
        now.timeIntervalSince(updatedAt) < Self.rejoinWindow && now >= updatedAt.addingTimeInterval(-60)
    }

    func save(_ defaults: UserDefaults) {
        if let data = try? JSONEncoder().encode(self) { defaults.set(data, forKey: Self.key) }
    }

    static func load(_ defaults: UserDefaults) -> VoiceCallSnapshot? {
        defaults.data(forKey: key).flatMap { try? JSONDecoder().decode(VoiceCallSnapshot.self, from: $0) }
    }

    static func clear(_ defaults: UserDefaults) {
        defaults.removeObject(forKey: key)
    }
}

/// Reconnect timing for a dropped GPT-Live connection: quick retries, then
/// on-device voice, about 30 seconds in total.
enum VoiceLiveReconnect {
    static let delays: [TimeInterval] = [0.5, 1.5, 3, 6, 10, 10]

    static func delay(afterAttempt attempt: Int) -> TimeInterval? {
        attempt < delays.count ? delays[attempt] : nil
    }
}

/// The system call UI (lock screen, Dynamic Island, the green call indicator)
/// and call audio priority through CallKit, for one call. CallKit activates
/// the audio session; the call starts its audio once `onActivate` runs.
///
/// CallKit is not used where Apple does not allow it (China).
@MainActor final class VoiceCallSystem {
    var onActivate: ((AVAudioSession) -> Void)?
    /// iOS ended or reset this call: from the system call UI, another call, or itself.
    var onEnd: ((VoiceCallSystemEnd) -> Void)?
    var onMute: ((Bool) -> Void)?
    /// The person resumed a held call from the system UI.
    var onResume: (() -> Void)?

    private let callKit = VoiceCallKit.shared
    private(set) var callID: UUID?
    private var activation: CheckedContinuation<Bool, Never>?

    static var isAllowed: Bool { VoiceCallKit.isAllowed }

    var isEnabled: Bool { callKit.provider != nil }
    /// CallKit activated the audio for this call, not for an earlier one.
    var audioActive: Bool { callKit.ownsAudio(self) }

    /// Starts an outgoing call in the system UI and waits (briefly) for CallKit
    /// to activate the audio session. Returns whether CallKit activated it.
    func start(handle name: String) async -> Bool {
        guard callKit.provider != nil else { return false }
        let id = UUID()
        callID = id
        callKit.register(self, for: id)
        VoiceCallLog.logger.info("CallKit start \(id.uuidString, privacy: .public)")
        let action = CXStartCallAction(call: id, handle: CXHandle(type: .generic, value: name))
        do {
            try await callKit.controller.request(CXTransaction(action: action))
        } catch {
            VoiceCallLog.logger.error("CallKit refused the start: \(error.localizedDescription, privacy: .public)")
            if callID == id { callID = nil }
            callKit.unregister(id)
            return false
        }
        if callKit.ownsAudio(self) { return true }
        return await withCheckedContinuation { continuation in
            activation = continuation
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(3))
                self?.finishActivation(false)
            }
        }
    }

    func reportConnected() {
        guard let provider = callKit.provider, let callID else { return }
        provider.reportOutgoingCall(with: callID, connectedAt: Date())
    }

    func setHeld(_ held: Bool) {
        guard let callID else { return }
        callKit.controller.request(CXTransaction(action: CXSetHeldCallAction(call: callID, onHold: held))) { _ in }
    }

    func setMuted(_ muted: Bool) {
        guard let callID else { return }
        callKit.controller.request(CXTransaction(action: CXSetMutedCallAction(call: callID, muted: muted))) { _ in }
    }

    /// Ends the system call; CallKit then deactivates the audio session. The
    /// end action CallKit sends back no longer matches `callID`, so it is
    /// not taken for iOS ending the call.
    func end() {
        guard let callID else { return }
        self.callID = nil
        callKit.noteEnded(callID)
        VoiceCallLog.logger.info("CallKit end \(callID.uuidString, privacy: .public)")
        callKit.controller.request(CXTransaction(action: CXEndCallAction(call: callID))) { error in
            if error != nil {
                Task { @MainActor in VoiceCallKit.shared.provider?.reportCall(with: callID, endedAt: Date(), reason: .remoteEnded) }
            }
        }
    }

    /// After `end()`: waits (briefly) for CallKit to deactivate the call's
    /// audio, then stops taking audio callbacks, so a late one can't switch
    /// off the audio of the call that replaces this one.
    func waitUntilReleased() async {
        await callKit.waitForAudioRelease()
        onActivate = nil
    }

    // Called by VoiceCallKit for this call's actions.

    fileprivate func endedBySystem(_ id: UUID, _ reason: VoiceCallSystemEnd) {
        guard VoiceCallSystemEnd.isCurrent(id, callID: callID) else {
            VoiceCallLog.logger.info("CallKit \(reason.rawValue, privacy: .public) ignored for \(id.uuidString, privacy: .public)")
            return
        }
        VoiceCallLog.logger.notice("CallKit \(reason.rawValue, privacy: .public) the call \(id.uuidString, privacy: .public)")
        callID = nil
        callKit.noteEnded(id)
        finishActivation(false)
        onEnd?(reason)
    }

    fileprivate func setMutedBySystem(_ muted: Bool) { onMute?(muted) }
    fileprivate func resumedBySystem() { onResume?() }

    fileprivate func activated(_ session: AVAudioSession) {
        onActivate?(session)
        finishActivation(true)
    }


    private func finishActivation(_ active: Bool) {
        let continuation = activation
        activation = nil
        continuation?.resume(returning: active)
    }
}

/// The app's one CallKit provider. Apple expects a single provider per app;
/// one per call let an action or reset meant for an earlier call reach a new
/// one. Each action goes to the call that owns its UUID.
@MainActor final class VoiceCallKit: NSObject {
    static let shared = VoiceCallKit()

    static var isAllowed: Bool {
        Locale.current.region?.identifier != "CN"
    }

    let provider: CXProvider?
    let controller = CXCallController()
    private(set) var audioActive = false
    private var calls: [UUID: WeakCall] = [:]
    /// The call started last; CallKit connects one call at a time.
    private var newestCallID: UUID?
    /// Each activation's call, so each deactivation reaches the right one.
    private var activations = VoiceCallAudioActivations()
    /// The call whose audio is on now.
    private var audioOwner: VoiceCallSystem? { activations.current.flatMap(call) }
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []

    private struct WeakCall { weak var system: VoiceCallSystem? }

    override init() {
        if Self.isAllowed {
            let configuration = CXProviderConfiguration()
            configuration.supportsVideo = false
            configuration.maximumCallsPerCallGroup = 1
            configuration.maximumCallGroups = 1
            configuration.supportedHandleTypes = [.generic]
            configuration.includesCallsInRecents = false
            provider = CXProvider(configuration: configuration)
        } else {
            provider = nil
        }
        super.init()
        provider?.setDelegate(self, queue: .main)
    }

    func register(_ system: VoiceCallSystem, for id: UUID) {
        calls = calls.filter { $0.value.system != nil }
        calls[id] = WeakCall(system: system)
        newestCallID = id
    }

    func unregister(_ id: UUID) { calls[id] = nil }

    /// A call ended; its audio deactivation may still be to come.
    func noteEnded(_ id: UUID) { activations.ended(id) }

    /// Whether the active call audio belongs to this call. The audio flag is
    /// app-wide now that there is one provider, so a call can't go by it alone.
    func ownsAudio(_ system: VoiceCallSystem) -> Bool { audioActive && audioOwner === system }

    private func call(_ id: UUID) -> VoiceCallSystem? { calls[id]?.system }

    /// Waits up to 2 s for CallKit to deactivate the call audio.
    func waitForAudioRelease() async {
        guard provider != nil, audioActive else { return }
        await withCheckedContinuation { continuation in
            releaseWaiters.append(continuation)
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(2))
                guard let self, !self.releaseWaiters.isEmpty else { return }
                VoiceCallLog.logger.notice("CallKit didn't deactivate audio within 2 s")
                self.finishRelease()
            }
        }
    }

    private func finishRelease() {
        let waiters = releaseWaiters
        releaseWaiters = []
        waiters.forEach { $0.resume() }
    }
}

extension VoiceCallKit: CXProviderDelegate {
    nonisolated func providerDidReset(_ provider: CXProvider) {
        MainActor.assumeIsolated {
            VoiceCallLog.logger.notice("CallKit reset the provider")
            let all = calls
            calls = [:]
            audioActive = false
            activations.reset()
            for (id, entry) in all { entry.system?.endedBySystem(id, .reset) }
            finishRelease()
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        MainActor.assumeIsolated {
            // CallKit activates the session with the category set here.
            try? AVAudioSession.sharedInstance().setCategory(
                .playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothHFP, .allowBluetoothA2DP])
            provider.reportOutgoingCall(with: action.callUUID, startedConnectingAt: Date())
            action.fulfill()
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        MainActor.assumeIsolated {
            action.fulfill()
            // Kept registered (weakly): its audio deactivation is still to come.
            call(action.callUUID)?.endedBySystem(action.callUUID, .ended)
            noteEnded(action.callUUID)
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
        MainActor.assumeIsolated {
            action.fulfill()
            if let system = call(action.callUUID), system.callID == action.callUUID {
                system.setMutedBySystem(action.isMuted)
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetHeldCallAction) {
        MainActor.assumeIsolated {
            action.fulfill()
            if !action.isOnHold, let system = call(action.callUUID), system.callID == action.callUUID {
                system.resumedBySystem()
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
        MainActor.assumeIsolated {
            audioActive = true
            // The call CallKit is connecting, if it hasn't ended meanwhile.
            let owner = newestCallID.flatMap { id in call(id)?.callID == id ? id : nil }
            if let owner { activations.activated(owner) }
            VoiceCallLog.logger.info("CallKit activated audio")
            owner.flatMap(call)?.activated(audioSession)
        }
    }

    nonisolated func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
        MainActor.assumeIsolated {
            let (_, audioOff) = activations.deactivated { self.call($0) == nil }
            audioActive = !audioOff
            VoiceCallLog.logger.info("CallKit deactivated audio\(audioOff ? "" : " of an earlier call; the current call keeps it", privacy: .public)")
            // App-wide, like the audio session: only when no newer call's audio is on.
            if audioOff { VoiceLiveConnection.audioSessionDeactivated() }
            finishRelease()
        }
    }
}
