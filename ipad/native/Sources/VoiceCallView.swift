import AVFoundation
import os
import SwiftUI
import UIKit

/// One voice call with an agent. Each request is a DM to the agent's live
/// session and its replies are spoken: by GPT-Live when the person allows it
/// (the main path), otherwise with on-device speech (the fallback).
/// See docs/specs/ios-voice-call.md.
@MainActor final class VoiceCallController: ObservableObject {
    enum Phase: Equatable {
        case starting, consent, listening, hearing, confirming, sending, waiting, speaking, paused
        /// GPT-Live was closed to stop billing silence; the person can resume.
        case liveIdle
        /// The GPT-Live connection dropped and the call is reconnecting.
        case reconnecting
        case ended
    }
    enum Route: Equatable { case live, onDevice }
    enum Mode: String, CaseIterable, Identifiable {
        case openMic = "Open mic", pushToTalk = "Push to talk"
        var id: String { rawValue }
    }

    @Published private(set) var phase: Phase = .starting
    @Published var mode: Mode = .openMic { didSet { if mode != oldValue { resetListening() } } }
    @Published private(set) var muted = false {
        didSet {
            audio.setMuted(muted)
            live?.setMuted(muted)
            if muted { resetListening() }
        }
    }
    @Published private(set) var route: Route = .onDevice
    @Published private(set) var transcript = ""
    @Published private(set) var lastHeard = ""
    @Published private(set) var lastReply = ""
    @Published private(set) var pendingConfirmation: String?
    @Published private(set) var riskyWord = ""
    @Published private(set) var notice: String?
    @Published private(set) var level: Float = -160
    @Published private(set) var echoCancelling = true

    let agent: RelayAgent
    private let client: RelayClient
    private let audio = VoiceCallAudio()
    private let timeouts = VoiceCallTimeouts()
    private var detector = VoiceActivityDetector()
    /// Louder and longer than normal speech so room noise does not cut the agent off.
    private var bargeIn = VoiceActivityDetector(threshold: -32, startDuration: 0.25, hangover: 0.7)
    private var tracker: VoiceReplyTracker?
    /// The call's first conversation load finished (or failed); see `VoiceCallBaseline`.
    private var baselineAttempted = false
    private var waitingSince: Date?
    private var fillerPlayed = false
    private var lastActivity = Date()
    private var speakingEndedAt = Date.distantPast
    private var engineRunning = false
    private var watchdog: Task<Void, Never>?
    private var observers: [NSObjectProtocol] = []
    private let preferences = VoiceLivePreferences()
    let style = VoiceStylePreferences()
    // GPT-Live
    private var live: VoiceLiveConnection?
    private var liveSessionID = ""
    private var liveUsageSeconds: Double = 0
    private var liveTranscript = VoiceLiveTranscript()
    private var liveLastActivity = Date()
    private var liveClosingForIdle = false
    /// Delegations forwarded to the agent and not yet answered, oldest first.
    private var pendingDelegations: [String] = []
    private var lastDelegationID: String?
    private var confirmingDelegationID: String?
    /// Agent replies waiting for a voice path that can speak them.
    private var replyQueue: [String] = []
    /// Every GPT-Live session's usage, kept on this device until the broker accepts it.
    private let usageLedger = VoiceLiveUsageLedger()
    private var reportedSessions: Set<String> = []
    /// The broker's error code from the last GPT-Live start that failed.
    private var liveStartFailure: String?
    /// The notice that start showed, cleared if a retry then connects.
    private var liveFailureNotice: String?
    /// A GPT-Live session is being set up; only one at a time.
    private var liveSetupInFlight = false
    /// The current GPT-Live session sent `session.started`.
    private var liveReady = false
    private let liveIdle = VoiceLiveIdlePolicy()
    // Call lifetime
    private let system = VoiceCallSystem()
    private let rejoining: VoiceCallSnapshot?
    private var reconnectAttempt = 0
    private var knownMessageIDs: [String] = []
    /// The client's session when the call was made; a lookup after a sign-out or workspace switch is refused.
    private let sessionAtStart: Int
    /// Called once when the call ends.
    var onEnded: (() -> Void)?
    /// Called with the call's state while it is active, to rejoin after a relaunch.
    var onSnapshot: ((VoiceCallSnapshot) -> Void)?

    /// `rejoining` carries a call that was active before the app was relaunched.
    init(agent: RelayAgent, client: RelayClient, rejoining: VoiceCallSnapshot? = nil) {
        self.agent = agent
        self.client = client
        self.rejoining = rejoining
        // Before any await, so a workspace switch during the microphone prompt
        // cannot hand this call the new session.
        self.sessionAtStart = client.sessionGeneration
        for turn in rejoining?.history ?? [] {
            liveTranscript.add(turn.text + " ", startMs: 0, fromPerson: turn.fromPerson)
        }
    }

    /// Mutes or unmutes the microphone, in the app and in the system call UI.
    func setMuted(_ value: Bool) {
        guard value != muted else { return }
        muted = value
        system.setMuted(value)
    }

    var agentName: String { client.displayName(for: agent) }

    var status: String {
        switch phase {
        case .starting: return "Connecting…"
        case .consent: return "Agree to use GPT-Live"
        case .liveIdle: return "Paused to save GPT-Live minutes"
        case .reconnecting: return "Reconnecting…"
        case .listening where route == .live: return muted ? "Muted" : "Live"
        case .listening: return muted ? "Muted" : mode == .pushToTalk ? "Hold to talk" : "Listening"
        case .hearing: return "Hearing you"
        case .confirming: return "Check before sending"
        case .sending: return "Sending"
        case .waiting: return "Waiting for \(agentName)"
        case .speaking: return "\(agentName) is speaking"
        case .paused: return "Paused"
        case .ended: return "Call ended"
        }
    }

    /// Tags this call's log lines, so two calls in one run tell apart in Console.
    private let logID = String(UUID().uuidString.prefix(6))

    private func log(_ message: String, _ level: OSLogType = .info) {
        VoiceCallLog.logger.log(level: level, "[\(self.logID, privacy: .public) \(self.agent.name, privacy: .public)] \(message, privacy: .public)")
    }

    /// A step of the call's life, so Console (subsystem
    /// com.agentworkforce.relayipad, category voice-call) shows how far a call
    /// got and why it ended: filter on "stage=".
    private func stage(_ name: String, _ detail: String = "", _ level: OSLogType = .default) {
        log("stage=\(name)\(detail.isEmpty ? "" : " \(detail)")", level)
    }

    /// The call fell back to on-device voice because GPT-Live couldn't start.
    var fellBack: Bool { route == .onDevice && engineRunning && phase != .ended }

    private var activity: VoiceCallTimeouts.Activity {
        switch phase {
        case .listening, .liveIdle: return .resting
        case .paused: return .held
        default: return .busy
        }
    }

    // MARK: Lifecycle

    func start() async {
        guard phase == .starting else { return }
        // From the start, so a call left at the microphone prompt or the
        // consent question still ends instead of blocking later calls.
        lastActivity = Date()
        if watchdog == nil { watchdog = Task { [weak self] in await self?.runWatchdog() } }
        stage("starting", rejoining == nil ? "" : "rejoining")
        guard await AVAudioApplication.requestRecordPermission() else {
            return end(.microphoneDenied)
        }
        stage("microphone", "allowed")
        // Everything already in the conversation counts as heard; a rejoined
        // call still reads replies that arrived while the app was relaunching.
        // An agent never messaged from this device has no thread yet: that is
        // a known, empty conversation, not a failed read.
        let load = await loadConversation(startingCall: true)
        switch VoiceCallStartup.afterLoad(load, rejoined: rejoining?.knownMessageIDs, phaseIsStarting: phase == .starting) {
        case .cancelled:
            return stage("cancelled", "during the conversation load, in \(phase)")
        case .proceed(let heard, let known):
            if let heard { tracker = VoiceReplyTracker(existing: heard.map { VoiceTranscriptMessage(id: $0, text: "", mine: true) }) }
            // Without a baseline, the first load that succeeds becomes it (checkReplies).
            knownMessageIDs = known
        }
        stage("conversation", load.summary, load == .failed ? .error : .default)
        baselineAttempted = true
        if rejoining != nil { notice = "Rejoined your call with \(agentName)." }
        guard VoiceCallRoute.choose(consented: preferences.consented) != nil else {
            stage("consent", "asking")
            phase = .consent
            return
        }
        await begin()
    }

    /// The conversation as the call sees it, read without the inbox, in the
    /// session the call started in.
    private func loadConversation(startingCall: Bool = false) async -> VoiceConversationLoad {
        switch await client.directHistory(with: agent, startingCall: startingCall, session: sessionAtStart) {
        case .messages(let messages): return .messages(Self.transcriptMessages(messages))
        case .noThread: return .noThread
        case .failed: return .failed
        }
    }

    /// The person's answer to the GPT-Live consent question. Voice calls need
    /// GPT-Live, so declining ends the call before it starts.
    func answerConsent(agreed: Bool) {
        guard phase == .consent else { return }
        guard agreed else { return end(.consentDeclined) }
        stage("consent", "agreed")
        preferences.consented = true
        phase = .starting
        Task { await begin() }
    }

    /// Saves a new persona and voice. GPT-Live fixes the voice when a session
    /// starts, so a live call reconnects with the conversation so far; the
    /// on-device voice changes from the next sentence.
    func applyStyle(persona: VoicePersona, voice: String?) {
        let changed = persona != style.persona || (voice != nil && voice != style.voice) || (voice == nil && style.voiceChosen)
        style.persona = persona
        if let voice { style.voice = voice } else { style.clearVoice() }
        guard changed, route == .live, live != nil, phase != .ended, !liveSetupInFlight else { return }
        let closing = closeLive(reportingUsage: true, immediately: true)
        // The new session can't answer the old session's delegations.
        resetDelegationsForNewSession()
        phase = .starting
        notice = "Switching to \(persona.title.lowercased()) style…"
        Task {
            if await VoiceLiveHandover.replace(releasing: closing, then: { await self.startLive() },
                                               retryWhen: { self.liveStartFailure == "voice_session_limit" }) {
                // startLive() also returns true for a call that ended meanwhile; keep its notice.
                if phase != .ended { notice = nil }
                return
            }
            guard phase != .ended else { return }
            await startOnDevice()
        }
    }

    /// Whether a style change can take effect now: not while a GPT-Live
    /// session is being set up, which has already captured the old style.
    var canApplyStyle: Bool {
        phase != .starting && phase != .reconnecting && phase != .consent && phase != .ended
    }

    private func begin() async {
        observeSystem()
        if watchdog == nil { watchdog = Task { [weak self] in await self?.runWatchdog() } }
        if system.callID == nil {
            system.onEnd = { [weak self] reason in self?.end(.system(reason)) }
            system.onMute = { [weak self] value in
                guard let self, value != self.muted else { return }
                self.muted = value
            }
            system.onResume = { [weak self] in self?.resumeLive() }
            system.onActivate = { session in VoiceLiveConnection.audioSessionActivated(session) }
            // CallKit keeps the call alive on the lock screen and in the
            // background; without it the app's background audio mode does.
            let activated = await system.start(handle: agentName)
            stage("callkit", !system.isEnabled ? "unavailable" : activated ? "activated audio" : "didn't activate audio")
            guard phase != .ended else { return system.end() }
        }
        // GPT-Live, or the on-device fallback when it can't start.
        stage("gpt-live", "starting")
        if await startLive() { return stage(phase == .ended ? "cancelled" : "gpt-live", phase == .ended ? "during the GPT-Live start" : "connected") }
        guard phase != .ended else { return stage("cancelled", "during the GPT-Live start") }
        await startOnDevice()
    }

    private func startOnDevice() async {
        stage("on-device", "falling back")
        route = .onDevice
        guard await VoiceCallAudio.requestSpeechAuthorization() else {
            return end(.speechDenied)
        }
        guard !VoiceCallStartup.cancelled(phaseIsStarting: phase == .starting) else { return stage("cancelled", "during the on-device start, in \(phase)") }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .voiceChat,
                                    options: [.defaultToSpeaker, .allowBluetoothHFP, .allowBluetoothA2DP])
            // CallKit activates the session itself.
            if !system.audioActive { try session.setActive(true) }
            wireAudio()
            try audio.start()
        } catch {
            audio.stop()
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            return end(.audioFailed(error.localizedDescription))
        }
        engineRunning = true
        echoCancelling = audio.echoCancelling
        audio.play(.listening)
        stage("listening", "on-device")
        phase = .listening
        lastActivity = Date()
        // A GPT-Live call may have been on hold before falling back.
        system.setHeld(false)
        system.reportConnected()
        flushReplies()
    }

    /// Ends the call and waits until it has let go of the audio, so a call
    /// started next owns the audio session and the system call UI alone.
    func endReleasingAudio() async {
        end(.replaced)
        await system.waitUntilReleased()
        // The on-device engine stops 400 ms after the call ends.
        try? await Task.sleep(for: .milliseconds(500))
    }

    /// Ends the call for a cause; the call screen shows the cause's reason.
    func end(_ cause: VoiceCallEndCause) {
        guard phase != .ended else { return }
        stage("ended", "in \(phase) cause=\(cause.stage): \(cause.notice)")
        notice = cause.notice
        phase = .ended
        watchdog?.cancel()
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        observers = []
        closeLive(reportingUsage: true)
        let usedCallKit = system.isEnabled
        system.end()
        onEnded?()
        onEnded = nil
        guard engineRunning else {
            if !usedCallKit { try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation) }
            return
        }
        engineRunning = false
        audio.stopSpeaking()
        audio.cancelUtterance()
        audio.play(.ended)
        let audio = audio
        Task {
            try? await Task.sleep(for: .milliseconds(400))
            audio.stop()
            if !usedCallKit {
                try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            }
        }
    }

    /// Another call, Siri or an alarm took the audio. The call is not ended:
    /// GPT-Live is paused (so it doesn't bill) and resumes afterwards.
    func pause() {
        if route == .live {
            if live != nil, phase != .ended { closeLiveForIdle() }
            return
        }
        guard engineRunning, phase != .ended, phase != .paused else { return }
        audio.pause()
        detector.reset()
        transcript = ""
        phase = .paused
        lastActivity = Date()
    }

    func resume() {
        if route == .live { return resumeLive() }
        guard engineRunning, phase == .paused else { return }
        do {
            try AVAudioSession.sharedInstance().setActive(true)
            try audio.resume()
            phase = restingPhase
            lastActivity = Date()
        } catch {
            end(.resumeFailed)
        }
    }

    // MARK: Speaking to the agent

    func pressToTalk() {
        guard mode == .pushToTalk, !muted, canListen else { return }
        if audio.isSpeaking { stopAgentVoice() }
        beginHearing()
    }

    func releaseToTalk() {
        guard mode == .pushToTalk, phase == .hearing else { return }
        audio.endUtterance()
    }

    func confirmPending() {
        guard let text = pendingConfirmation else { return }
        pendingConfirmation = nil
        if route == .live, let id = confirmingDelegationID {
            confirmingDelegationID = nil
            phase = restingPhase
            forwardDelegation(id, request: text)
        } else {
            send(text)
        }
    }

    func discardPending() {
        pendingConfirmation = nil
        if route == .live, let id = confirmingDelegationID {
            confirmingDelegationID = nil
            live?.send(.commentary(delegationID: id, content: "The person cancelled that request on screen, so it was not sent to \(agentName)."))
        }
        phase = restingPhase
    }

    private var canListen: Bool {
        [.listening, .waiting, .speaking].contains(phase)
    }

    private var restingPhase: Phase {
        audio.isSpeaking ? .speaking : waitingSince != nil ? .waiting : .listening
    }

    private func wireAudio() {
        audio.onLevel = { [weak self] level, duration in self?.handleLevel(level, duration) }
        audio.onPartial = { [weak self] text in self?.transcript = text }
        audio.onFinal = { [weak self] text in self?.handleFinal(text) }
        audio.onSpeechFinished = { [weak self] in self?.speechFinished() }
    }

    private func handleLevel(_ level: Float, _ duration: TimeInterval) {
        if abs(level - self.level) >= 2 { self.level = level }
        guard mode == .openMic, !muted, canListen || phase == .hearing else { return }
        if audio.isSpeaking {
            // Without echo cancellation the microphone hears the agent, so the
            // call is half-duplex: no barge-in.
            guard echoCancelling else { return }
            if bargeIn.process(level: level, duration: duration) == .started {
                stopAgentVoice()
                // The person is already talking; the pre-roll holds the start.
                detector.markSpeaking()
                beginHearing()
            }
            return
        }
        if !echoCancelling, Date().timeIntervalSince(speakingEndedAt) < 0.6 { return }
        switch detector.process(level: level, duration: duration) {
        case .started: beginHearing()
        case .ended: audio.endUtterance()
        case nil: break
        }
    }

    private func beginHearing() {
        transcript = ""
        lastActivity = Date()
        audio.beginUtterance()
        phase = .hearing
    }

    private func handleFinal(_ raw: String) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        transcript = ""
        detector.reset()
        lastActivity = Date()
        guard phase == .hearing else { return }
        guard !text.isEmpty else {
            phase = restingPhase
            return
        }
        lastHeard = text
        switch VoiceCommand.parse(text) {
        case .endCall:
            return end(.spokenGoodbye)
        case .stopSpeaking:
            stopAgentVoice()
            phase = restingPhase
            return
        case .none:
            break
        }
        if let risk = VoiceActionGuard.risk(in: text) {
            riskyWord = risk
            pendingConfirmation = text
            phase = .confirming
            audio.play(.heard)
            return
        }
        send(text)
    }

    private func send(_ text: String) {
        audio.play(.heard)
        phase = .sending
        notice = nil
        Task {
            let sent = await deliver(text)
            guard phase != .ended else { return }
            if sent {
                if phase != .paused { phase = restingPhase }
                await checkReplies()
            } else {
                notice = "That didn't reach \(agentName). Check your connection and try again."
                if phase != .paused {
                    say("I couldn't send that.")
                    phase = restingPhase
                }
            }
        }
    }

    /// Sends a request to the agent as a DM with the voice header.
    private func deliver(_ text: String) async -> Bool {
        let sent = await client.send(VoiceSpeech.outgoing(text), to: agent)
        if sent {
            waitingSince = Date()
            fillerPlayed = false
        }
        return sent
    }

    private func resetListening() {
        detector.reset()
        guard phase == .hearing || phase == .confirming else { return }
        audio.cancelUtterance()
        pendingConfirmation = nil
        transcript = ""
        phase = restingPhase
    }

    // MARK: Hearing the agent

    private static func transcriptMessages(_ messages: [RelayMessage]) -> [VoiceTranscriptMessage] {
        messages.filter(\.hasServerID).map { VoiceTranscriptMessage(id: $0.id, text: $0.text, mine: $0.mine) }
    }

    private func checkReplies() async {
        // Replies that arrive while paused are read after the call resumes.
        guard baselineAttempted, phase != .ended, phase != .paused, phase != .consent else { return }
        let load = await loadConversation()
        guard phase != .ended, phase != .paused else { return }
        // A failed read leaves the baseline as it was; no thread yet is an empty one.
        guard let messages = load.messagesToTrack else { return }
        knownMessageIDs = messages.map(\.id)
        guard var tracker else {
            // The conversation couldn't load when the call started; what is
            // there now counts as heard, so nothing old is read aloud.
            self.tracker = VoiceReplyTracker(existing: messages)
            return
        }
        let replies = tracker.newReplies(in: messages)
        self.tracker = tracker
        guard !replies.isEmpty else { return }
        waitingSince = nil
        lastActivity = Date()
        for reply in replies {
            lastReply = reply.text
            replyQueue.append(reply.text)
        }
        flushReplies()
    }

    /// Speaks queued agent replies once a voice path can: GPT-Live after its
    /// session started, or the on-device engine. Until then they wait (during
    /// setup, a rejoin, a reconnect or a hold), so none are lost.
    private func flushReplies() {
        guard !replyQueue.isEmpty, phase != .ended else { return }
        if route == .live {
            guard let live, liveReady else {
                // A reply is worth reconnecting a held call for.
                if phase == .liveIdle { resumeLive() }
                return
            }
            liveLastActivity = Date()
            while let reply = replyQueue.first {
                let id = pendingDelegations.first ?? lastDelegationID
                guard live.send(.commentary(delegationID: id, content: VoiceLiveReply.commentary(reply, agentName: agentName))) else { return }
                replyQueue.removeFirst()
                if !pendingDelegations.isEmpty { pendingDelegations.removeFirst() }
            }
            if pendingDelegations.isEmpty, phase == .waiting { phase = .listening }
        } else if engineRunning, phase != .starting, phase != .paused {
            for reply in replyQueue { say(reply) }
            replyQueue = []
        }
    }

    private func say(_ text: String) {
        let sentences = VoiceSpeech.sentences(text)
        guard !sentences.isEmpty, engineRunning, phase != .paused, phase != .ended else { return }
        bargeIn.reset()
        audio.speak(sentences, rate: style.persona.onDeviceRate, pitch: style.persona.onDevicePitch)
        if phase != .hearing, phase != .confirming { phase = .speaking }
    }

    private func stopAgentVoice() {
        audio.stopSpeaking()
        bargeIn.reset()
        detector.reset()
        speakingEndedAt = Date()
        if phase == .speaking { phase = restingPhase }
    }

    private func speechFinished() {
        speakingEndedAt = Date()
        lastActivity = Date()
        bargeIn.reset()
        if phase == .speaking { phase = restingPhase }
    }

    private func runWatchdog() async {
        var tick = 0
        while !Task.isCancelled, phase != .ended {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, phase != .ended else { continue }
            // No call outlives its use: a silent, held or stuck call ends, so
            // tapping Call later starts a new one instead of showing this one.
            if let cause = timeouts.endCause(activity, idleFor: Date().timeIntervalSince(lastActivity)) {
                end(cause)
                continue
            }
            guard phase != .paused else { continue }
            tick += 1
            if let since = waitingSince {
                switch timeouts.waitAction(waitingFor: Date().timeIntervalSince(since), fillerPlayed: fillerPlayed) {
                case .filler:
                    fillerPlayed = true
                    // GPT-Live acknowledges delegations itself.
                    if phase == .waiting, route == .onDevice { say("Waiting for the agent.") }
                case .giveUp:
                    waitingSince = nil
                    notice = "No reply from \(agentName) yet. The call will read it if it arrives."
                    if phase == .waiting { phase = .listening }
                    for id in pendingDelegations {
                        live?.send(.commentary(delegationID: id, content: "\(agentName) hasn't replied yet. Its reply will be read if it arrives."))
                    }
                    pendingDelegations = []
                case .none:
                    break
                }
                await checkReplies()
            } else if tick % 3 == 0 {
                // Agents sometimes follow up with a second message.
                await checkReplies()
            }
            if tick % 5 == 0 { saveSnapshot() }
            if tick % 30 == 0 { await retryUsageReports() }
            if route == .live, live != nil, [.listening, .waiting].contains(phase),
               liveIdle.shouldClose(lastActivity: liveLastActivity, pendingDelegations: pendingDelegations.count) {
                closeLiveForIdle()
            }
        }
    }

    private func saveSnapshot() {
        // Not before the conversation has loaded once: a rejoin from a snapshot
        // without known messages would read the whole conversation aloud.
        guard phase != .ended, phase != .consent, tracker != nil else { return }
        let history = liveTranscript.history().map { VoiceCallSnapshot.Turn(fromPerson: $0.fromPerson, text: $0.text) }
        onSnapshot?(VoiceCallSnapshot(agentID: agent.id, agentName: agent.name, updatedAt: Date(),
                                      history: history, knownMessageIDs: knownMessageIDs))
    }

    private func observeSystem() {
        guard observers.isEmpty else { return }
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: RelayPushState.messageArrived, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in await self?.checkReplies() }
        })
        observers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            let type = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt).flatMap { AVAudioSession.InterruptionType(rawValue: $0) }
            let options = AVAudioSession.InterruptionOptions(rawValue: note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0)
            MainActor.assumeIsolated {
                if type == .began { self?.pause() }
                else if type == .ended, options.contains(.shouldResume) { self?.resume() }
            }
        })
        observers.append(center.addObserver(forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main) { [weak self] _ in
            // A route change (AirPods in or out) stops the engine.
            MainActor.assumeIsolated {
                guard let self, self.route == .onDevice, self.engineRunning, self.phase != .ended, self.phase != .paused else { return }
                do {
                    try self.audio.restart()
                    // The restart dropped any utterance in progress.
                    self.detector.reset()
                    self.transcript = ""
                    if self.phase == .hearing { self.phase = self.restingPhase }
                } catch {
                    self.end(.audioRouteLost)
                }
            }
        })
        observers.append(center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.end(.audioStopped) }
        })
    }
}

// MARK: GPT-Live

extension VoiceCallController {
    /// Starts (or resumes) a GPT-Live session through the Cloud voice broker.
    /// Returns false, with a notice, when the call should use on-device voice.
    fileprivate func startLive() async -> Bool {
        guard !liveSetupInFlight else { return true }
        liveSetupInFlight = true
        defer { liveSetupInFlight = false }
        liveStartFailure = nil
        let connection = VoiceLiveConnection()
        // The session the broker granted in this attempt, so a later failure still reports it.
        var grantedSessionID: String?
        connection.onEvent = { [weak self, weak connection] event in
            guard let self, let connection else { return }
            self.handleLive(event, from: connection)
        }
        connection.onDisconnect = { [weak self, weak connection] in
            guard let self, let connection, self.live === connection else { return }
            self.liveDisconnected()
        }
        do {
            let offer = try await connection.makeOffer()
            guard phase != .ended else {
                connection.tearDown()
                return true
            }
            let history = liveTranscript.history()
            let voice = style.voice
            let persona = style.persona
            // Longer than the broker's own worst case (ending overdue and
            // oldest calls, then OpenAI's 8 s), so a slow start isn't abandoned
            // while the broker still opens a session.
            log("Asking the broker for a GPT-Live session", .info)
            let grant = try await Self.withDeadline(20) { [client, agent] in
                try await client.createVoiceLiveSession(offer: offer, agent: agent, history: history,
                                                        voice: voice, persona: persona)
            }
            grantedSessionID = grant.sessionID
            log("Broker granted \(grant.sessionID)", .info)
            guard [.starting, .reconnecting].contains(phase) else {
                log("The call moved on during the start; reporting \(grant.sessionID)", .default)
                // The call moved on while the broker opened the session; free its slot.
                connection.tearDown()
                reportUsageInBackground(sessionID: grant.sessionID, seconds: 0)
                return phase == .ended
            }
            // Current before the answer is applied, so `session.started` isn't missed.
            live = connection
            liveReady = false
            liveSessionID = grant.sessionID
            connection.sessionID = grant.sessionID
            // Recorded before anything else can fail, so even a killed call reports it.
            usageLedger.open(grant.sessionID)
            liveUsageSeconds = 0
            liveClosingForIdle = false
            liveTranscript.startNewSession()
            try await connection.accept(answer: grant.answer)
            guard live === connection, phase != .ended else {
                // The call ended (or moved on) while the answer was applied.
                if live === connection { live = nil }
                connection.tearDown()
                reportUsageInBackground(sessionID: grant.sessionID, seconds: connection.usageSeconds)
                return true
            }
            liveLastActivity = Date()
            route = .live
            // A retried start succeeded: the first attempt's refusal no longer applies.
            if let shown = liveFailureNotice, notice == shown { notice = nil }
            liveFailureNotice = nil
            connection.setMuted(muted)
            phase = restingPhase
            lastActivity = Date()
            system.setHeld(false)
            system.reportConnected()
            // A session that never starts counts as a failed connection.
            Task { [weak self, weak connection] in
                try? await Task.sleep(for: .seconds(10))
                guard let self, let connection, self.live === connection, !self.liveReady else { return }
                self.liveDisconnected()
            }
            return true
        } catch {
            if live === connection { live = nil }
            connection.tearDown()
            let failure = error as? RelayAPIError
            // A usage report closes each session this start left open on the
            // broker; failed reports are retried every 30 s.
            let seconds = connection.usageSeconds
            for sessionID in VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: grantedSessionID,
                                                                            unconfirmed: failure?.voiceSessionID) {
                reportUsageInBackground(sessionID: sessionID, seconds: sessionID == grantedSessionID ? seconds : 0)
            }
            liveStartFailure = failure?.code
            log("GPT-Live start failed: status \(failure?.status ?? 0) code \(failure?.code ?? "none") \(error.localizedDescription)", .error)
            notice = VoiceCallRoute.fallbackReason(status: failure?.status, code: failure?.code.isEmpty == false ? failure?.code : nil)
            liveFailureNotice = notice
            return false
        }
    }

    /// Resumes a call whose GPT-Live session was closed while idle.
    func resumeLive() {
        guard phase == .liveIdle, !liveSetupInFlight else { return }
        var releasing: Task<Void, Never>?
        if let closing = live {
            // Still finishing the idle close, which now can't report it.
            live = nil
            closing.tearDown()
            releasing = reportUsageInBackground(sessionID: closing.sessionID, seconds: closing.usageSeconds)
        }
        notice = nil
        phase = .starting
        Task {
            if await VoiceLiveHandover.replace(releasing: releasing, then: { await self.startLive() },
                                               retryWhen: { self.liveStartFailure == "voice_session_limit" }) { return }
            guard phase != .ended else { return }
            await startOnDevice()
        }
    }

    fileprivate func closeLiveForIdle() {
        guard let live, !liveClosingForIdle else { return }
        liveClosingForIdle = true
        live.requestClose()
        phase = .liveIdle
        lastActivity = Date()
        // The system call stays up, shown as on hold; resuming it reconnects.
        system.setHeld(true)
        // Release the connection even if `session.closed` never arrives.
        Task { [weak self, weak live] in
            try? await Task.sleep(for: .seconds(3))
            guard let self, let live, self.live === live else { return }
            self.live = nil
            live.tearDown()
            await self.reportUsage(sessionID: live.sessionID, seconds: live.usageSeconds)
        }
    }

    /// Ends the GPT-Live session now; `session.closed` may not arrive.
    /// `immediately` stops WebRTC audio at once, before another voice path
    /// takes the microphone; otherwise GPT-Live gets a moment to report usage.
    /// Returns the close, which ends once the usage report finished.
    @discardableResult
    fileprivate func closeLive(reportingUsage: Bool, immediately: Bool = false) -> Task<Void, Never>? {
        guard let live else { return nil }
        self.live = nil
        liveReady = false
        live.onDisconnect = nil
        live.requestClose()
        if immediately { live.setMuted(true) }
        let sessionID = live.sessionID
        // Holds the controller strongly: closing the ended call's screen
        // releases it, and an unreported session keeps one of the account's
        // two GPT-Live slots and is charged up to 30 minutes. The background
        // task keeps the report going if the app leaves the foreground.
        let background = BackgroundWork(name: "Report GPT-Live usage")
        background.work = Task {
            try? await Task.sleep(for: .milliseconds(immediately ? 300 : 3_000))
            live.tearDown()
            if reportingUsage, !Task.isCancelled { await self.reportUsage(sessionID: sessionID, seconds: live.usageSeconds) }
            background.end()
        }
        return background.work
    }

    fileprivate func handleLive(_ event: VoiceLiveEvent, from connection: VoiceLiveConnection) {
        guard connection === live else {
            // An earlier session finishing (idle close, reconnect or end).
            if case .closed(let seconds) = event {
                let sessionID = connection.sessionID
                reportUsageInBackground(sessionID: sessionID, seconds: max(seconds ?? 0, connection.usageSeconds))
            } else if case .usage(let seconds) = event {
                connection.usageSeconds = max(connection.usageSeconds, seconds)
                usageLedger.update(connection.sessionID, seconds: connection.usageSeconds)
            }
            return
        }
        switch event {
        case .started:
            log("GPT-Live session started")
            liveReady = true
            liveLastActivity = Date()
            // Only a session that actually started resets the reconnect budget.
            reconnectAttempt = 0
            if phase == .reconnecting { phase = restingPhase }
            flushReplies()
        case .inputTranscript(let text, let startMs):
            liveTranscript.add(text, startMs: startMs, fromPerson: true)
            liveLastActivity = Date()
            lastActivity = Date()
            lastHeard = liveTranscript.history(limit: 1).last(where: \.fromPerson)?.text ?? lastHeard
        case .outputTranscript(let text, let startMs):
            liveTranscript.add(text, startMs: startMs, fromPerson: false)
            liveLastActivity = Date()
            lastActivity = Date()
        case .delegation(let id, let offsetMs):
            handleDelegation(id, offsetMs: offsetMs)
        case .usage(let seconds):
            liveUsageSeconds = max(liveUsageSeconds, seconds)
            connection.usageSeconds = liveUsageSeconds
            usageLedger.update(connection.sessionID, seconds: liveUsageSeconds)
        case .closed(let seconds):
            if let seconds { liveUsageSeconds = max(liveUsageSeconds, seconds) }
            if live != nil, !liveClosingForIdle, phase != .ended {
                // GPT-Live ended the session on its own.
                return liveDisconnected()
            }
            let sessionID = connection.sessionID
            let usage = liveUsageSeconds
            if liveClosingForIdle {
                connection.tearDown()
                self.live = nil
            }
            reportUsageInBackground(sessionID: sessionID, seconds: usage)
        case .error(_, let message):
            if !message.isEmpty { notice = message }
        case .other:
            break
        }
    }

    private func handleDelegation(_ id: String, offsetMs: Int) {
        let request = liveTranscript.request(at: offsetMs)
        guard !request.isEmpty else {
            live?.send(.commentary(delegationID: id, content: "The request wasn't transcribed. Ask the person to say it again."))
            return
        }
        lastHeard = request
        if let risk = VoiceActionGuard.risk(in: request) {
            riskyWord = risk
            pendingConfirmation = request
            confirmingDelegationID = id
            phase = .confirming
            live?.send(.commentary(delegationID: id, content: "This request could \(risk) something, so the person must confirm it on screen before it is sent to \(agentName). Ask them to check the screen."))
            return
        }
        forwardDelegation(id, request: request)
    }

    fileprivate func forwardDelegation(_ id: String, request: String) {
        pendingDelegations.append(id)
        lastDelegationID = id
        liveLastActivity = Date()
        live?.send(.thinking(delegationID: id, content: "Sent to \(agentName). Waiting for its reply."))
        Task {
            let sent = await deliver(request)
            guard phase != .ended else { return }
            if sent {
                // Don't override reconnecting, hold or setup.
                if [.sending, .listening, .waiting].contains(phase) { phase = restingPhase }
                await checkReplies()
            } else {
                pendingDelegations.removeAll { $0 == id }
                notice = "That didn't reach \(agentName). Check your connection and try again."
                live?.send(.commentary(delegationID: id, content: "The request couldn't be sent to \(agentName). Ask the person to try again."))
            }
        }
    }

    /// The GPT-Live connection dropped (network loss, ICE failure, or the
    /// session ended). Reconnect with a new session seeded with the call so
    /// far; after about 30 seconds of failures, carry on with on-device voice.
    fileprivate func liveDisconnected() {
        guard phase != .ended, let live, !liveClosingForIdle else { return }
        log("GPT-Live disconnected; reconnecting", .default)
        self.live = nil
        liveReady = false
        live.tearDown()
        let sessionID = live.sessionID
        let usage = live.usageSeconds
        let releasing = reportUsageInBackground(sessionID: sessionID, seconds: usage)
        resetDelegationsForNewSession()
        phase = .reconnecting
        // The retry budget carries over until a new session actually starts.
        Task { _ = await VoiceLiveHandover.replace(releasing: releasing, then: { await self.reconnectLive(); return true }) }
    }

    /// A new GPT-Live session can't answer the previous session's delegations;
    /// replies are still read, as general updates.
    fileprivate func resetDelegationsForNewSession() {
        pendingDelegations = []
        lastDelegationID = nil
        if confirmingDelegationID != nil {
            confirmingDelegationID = nil
            pendingConfirmation = nil
        }
    }

    private func reconnectLive() async {
        while phase == .reconnecting {
            guard let delay = VoiceLiveReconnect.delay(afterAttempt: reconnectAttempt) else { break }
            reconnectAttempt += 1
            try? await Task.sleep(for: .seconds(delay))
            guard phase == .reconnecting else { return }
            if await startLive() {
                if phase != .ended { notice = nil }
                // Stays `.reconnecting` until `session.started`; a new failure
                // continues with the remaining budget.
                return
            }
        }
        guard phase == .reconnecting else { return }
        notice = "GPT-Live couldn't reconnect, so the call fell back to on-device voice."
        phase = .starting
        reconnectAttempt = 0
        replyQueue.insert("Falling back to on-device voice.", at: 0)
        await startOnDevice()
    }

    /// Reports a session's usage as background work, so leaving the app right
    /// after a call doesn't hold the report (and the session's slot) back.
    @discardableResult
    fileprivate func reportUsageInBackground(sessionID: String, seconds: Double) -> Task<Void, Never> {
        let background = BackgroundWork(name: "Report GPT-Live usage")
        let work = Task {
            await self.reportUsage(sessionID: sessionID, seconds: seconds)
            background.end()
        }
        background.work = work
        return work
    }

    /// Reports a session's billed seconds; failed reports are kept and retried.
    fileprivate func reportUsage(sessionID: String, seconds: Double) async {
        guard !sessionID.isEmpty, !reportedSessions.contains(sessionID) else { return }
        let value = usageLedger.finish(sessionID, seconds: seconds)
        if await client.reportVoiceLiveUsage(sessionID: sessionID, seconds: value) {
            usageLedger.remove(sessionID)
            reportedSessions.insert(sessionID)
            log("Usage reported for \(sessionID): \(value) s", .info)
        } else {
            log("Usage report failed for \(sessionID); kept for retry", .error)
        }
    }

    fileprivate func retryUsageReports() async {
        await VoiceCallService.reportLeftoverUsage(client: client, ledger: usageLedger)
    }

    fileprivate static func withDeadline<T>(_ seconds: Double, _ operation: @escaping @MainActor () async throws -> T) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(for: .seconds(seconds))
                throw RelayAPIError(message: "GPT-Live took too long to start", status: 504, code: "voice_live_timeout")
            }
            defer { group.cancelAll() }
            guard let result = try await group.next() else { throw CancellationError() }
            return result
        }
    }
}

/// The call screen. The call itself belongs to `VoiceCallService`, so closing
/// this screen (or the app going to the background) keeps it going.
struct VoiceCallView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var call: VoiceCallController
    @State private var holding = false
    @State private var showStyle = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 22) {
                VStack(spacing: 6) {
                    Text(call.agentName).font(.title2.weight(.semibold))
                    Text(call.status).font(.subheadline).foregroundStyle(.secondary)
                        .accessibilityIdentifier("Voice call status")
                    if call.phase != .consent, call.phase != .starting, call.phase != .ended {
                        Text(call.route == .live ? "GPT-Live voice" : "On-device voice (fallback)")
                            .font(.caption).foregroundStyle(.secondary)
                            .accessibilityIdentifier("Voice call route")
                    }
                }.padding(.top, 12)
                if call.phase == .consent {
                    consent
                } else {
                    levelMeter
                    if let pending = call.pendingConfirmation {
                        confirmation(pending)
                    } else {
                        transcriptCard
                    }
                }
                if call.phase == .liveIdle {
                    Button("Resume") { call.resumeLive() }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("Resume voice call")
                }
                if let notice = call.notice {
                    Text(notice).font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }
                if !call.echoCancelling, call.phase != .ended {
                    Text("Echo cancellation is unavailable, so the call can't hear you while the agent speaks.")
                        .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }
                Spacer(minLength: 0)
                controls
            }
            .padding(20)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) { Text("Voice call").font(.headline) }
                ToolbarItem(placement: .cancellationAction) {
                    if call.phase == .ended {
                        Button("Done") { dismiss() }
                    } else {
                        Button { dismiss() } label: { Label("Minimize", systemImage: "chevron.down") }
                            .accessibilityIdentifier("Minimize voice call")
                    }
                }
                if call.phase != .ended, call.phase != .consent {
                    ToolbarItem(placement: .primaryAction) {
                        Menu {
                            Button("Voice and style…") { showStyle = true }
                        } label: {
                            Label("Voice options", systemImage: "ellipsis.circle")
                        }
                    }
                }
            }
        }
        .sheet(isPresented: $showStyle) { VoiceStyleSheet(call: call) }
    }

    /// Draft wording, pending review (docs/specs/ios-voice-call.md).
    private var consent: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(VoiceLiveConsent.title).font(.headline)
            Text(VoiceLiveConsent.disclosure)
                .font(.callout)
            Text(VoiceLiveConsent.fallback)
                .font(.callout).foregroundStyle(.secondary)
            HStack {
                Button("Not now") { call.answerConsent(agreed: false) }.buttonStyle(.bordered)
                Spacer()
                Button("Use GPT-Live") { call.answerConsent(agreed: true) }.buttonStyle(.borderedProminent)
            }
        }
        .padding(14)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12))
        .accessibilityIdentifier("Voice call consent")
    }

    private var levelMeter: some View {
        let normalized = CGFloat(max(0, min(1, (call.level + 60) / 50)))
        let active = call.phase == .hearing || call.phase == .speaking
        return ZStack {
            Circle().fill(Color.accentColor.opacity(0.12))
                .frame(width: 120 + 60 * (active ? normalized : 0))
            Image(systemName: call.phase == .speaking ? "waveform" : call.muted ? "mic.slash.fill" : "mic.fill")
                .font(.system(size: 38, weight: .medium))
                .foregroundStyle(Color.accentColor)
        }
        .frame(height: 190)
        .animation(.easeOut(duration: 0.12), value: normalized)
        .accessibilityHidden(true)
    }

    private var transcriptCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !call.transcript.isEmpty || !call.lastHeard.isEmpty {
                Label(call.transcript.isEmpty ? call.lastHeard : call.transcript, systemImage: "person.wave.2")
                    .font(.callout)
            }
            if !call.lastReply.isEmpty {
                Label(VoiceSpeech.spokenText(call.lastReply), systemImage: "sparkles")
                    .font(.callout).foregroundStyle(.secondary).lineLimit(6)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func confirmation(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("This sounds like it could \(call.riskyWord) something. Check it before sending.")
                .font(.subheadline.weight(.semibold))
            Text(text).font(.callout)
            HStack {
                Button("Discard", role: .destructive) { call.discardPending() }.buttonStyle(.bordered)
                Spacer()
                Button("Send to agent") { call.confirmPending() }.buttonStyle(.borderedProminent)
            }
        }
        .padding(14)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12))
        .accessibilityIdentifier("Voice call confirmation")
    }

    private var controls: some View {
        VStack(spacing: 16) {
            if call.route == .onDevice {
                Picker("Mode", selection: $call.mode) {
                    ForEach(VoiceCallController.Mode.allCases) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented)
            }
            if call.route == .onDevice, call.mode == .pushToTalk {
                Text(holding ? "Release to send" : "Hold to talk")
                    .font(.headline)
                    .frame(maxWidth: .infinity, minHeight: 56)
                    .background(Color.accentColor.opacity(holding ? 0.3 : 0.15), in: Capsule())
                    .gesture(DragGesture(minimumDistance: 0)
                        .onChanged { _ in
                            guard !holding else { return }
                            holding = true
                            call.pressToTalk()
                        }
                        .onEnded { _ in
                            holding = false
                            call.releaseToTalk()
                        })
                    .accessibilityAddTraits(.isButton)
                    .accessibilityLabel(holding ? "Send" : "Start talking")
                    .accessibilityAction {
                        // VoiceOver cannot hold, so activation toggles recording.
                        holding.toggle()
                        if holding { call.pressToTalk() } else { call.releaseToTalk() }
                    }
                    .disabled(call.phase == .ended)
            }
            HStack(spacing: 40) {
                Button { call.setMuted(!call.muted) } label: {
                    Label(call.muted ? "Unmute" : "Mute", systemImage: call.muted ? "mic.slash.fill" : "mic.fill")
                        .labelStyle(.iconOnly).font(.title2).frame(width: 60, height: 60)
                        .background(Color(.secondarySystemFill), in: Circle())
                }
                .disabled(call.phase == .ended)
                Button {
                    call.end(.hungUp)
                    dismiss()
                } label: {
                    Label("End call", systemImage: "phone.down.fill")
                        .labelStyle(.iconOnly).font(.title2).foregroundStyle(.white)
                        .frame(width: 60, height: 60).background(Color.red, in: Circle())
                }
            }.buttonStyle(.plain)
        }
    }
}

/// Shown above the workspace while a call continues behind a closed call screen.
struct VoiceCallBar: View {
    @ObservedObject var call: VoiceCallController
    let open: () -> Void
    /// Clears an ended call once its reason has been read.
    var dismiss: () -> Void = {}

    var body: some View {
        if call.phase == .ended { ended } else { bar }
    }

    /// A call that ended while its screen was closed still says why, here,
    /// until the person dismisses it.
    private var ended: some View {
        HStack(spacing: 10) {
            Image(systemName: "phone.down.fill")
            Text(VoiceCallBarPolicy.endedText(notice: call.notice)).lineLimit(4)
            Spacer(minLength: 0)
            Button("Dismiss", action: dismiss).fontWeight(.semibold)
        }
        .font(.subheadline)
        .foregroundStyle(.white)
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(Color(.darkGray))
        .accessibilityIdentifier("Voice call ended bar")
    }

    private var bar: some View {
        Button(action: open) {
            HStack(spacing: 10) {
                Image(systemName: "phone.fill")
                Text("\(call.agentName) · \(call.status)").lineLimit(1)
                Spacer(minLength: 0)
                Text("Return").fontWeight(.semibold)
            }
            .font(.subheadline)
            .foregroundStyle(.white)
            .padding(.horizontal, 16).padding(.vertical, 10)
            .background(Color.green)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("Voice call bar")
    }
}

/// A UIKit background task around one piece of async work. If iOS runs out of
/// background time, the work is cancelled and the task ended, so the app is
/// not terminated for holding it open. Ends at most once.
@MainActor private final class BackgroundWork {
    var work: Task<Void, Never>?
    private var identifier: UIBackgroundTaskIdentifier = .invalid

    init(name: String) {
        identifier = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            MainActor.assumeIsolated {
                self?.work?.cancel()
                self?.end()
            }
        }
    }

    func end() {
        guard identifier != .invalid else { return }
        UIApplication.shared.endBackgroundTask(identifier)
        identifier = .invalid
    }
}

/// Voice and persona for the call. GPT-Live voices apply to GPT-Live calls;
/// personas shape both paths (on-device voice approximates them).
struct VoiceStyleSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var call: VoiceCallController
    @State private var persona: VoicePersona = .standard
    @State private var voice: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ForEach(VoicePersona.allCases) { option in
                        Button {
                            persona = option
                        } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.title).foregroundStyle(.primary)
                                    Text(option.summary).font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if option == persona { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
                            }
                        }
                        .accessibilityAddTraits(option == persona ? .isSelected : [])
                    }
                } header: {
                    Text("Persona")
                } footer: {
                    Text("Personas change how the call sounds, not what your agent says. Destructive requests still need your confirmation on screen.")
                }
                if !call.canApplyStyle {
                    Section {
                        Text("You can apply a new style once the call has connected.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                }
                Section {
                    Picker("GPT-Live voice", selection: Binding(
                        get: { voice ?? "" },
                        set: { voice = $0.isEmpty ? nil : $0 }
                    )) {
                        Text("Persona's choice (\(VoiceLiveVoices.title(persona.suggestedVoice)))").tag("")
                        ForEach(VoiceLiveVoices.all, id: \.self) { Text(VoiceLiveVoices.title($0)).tag($0) }
                    }
                } header: {
                    Text("Voice")
                } footer: {
                    Text(call.route == .live
                         ? "Changing the voice or persona reconnects GPT-Live with the conversation so far."
                         : "GPT-Live voices apply to GPT-Live calls. On-device voice uses the persona's pace and pitch.")
                }
            }
            .navigationTitle("Voice and style")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Apply") {
                        call.applyStyle(persona: persona, voice: voice)
                        dismiss()
                    }
                    .disabled(!call.canApplyStyle)
                }
            }
            .onAppear {
                persona = call.style.persona
                voice = call.style.voiceChosen ? call.style.voice : nil
            }
        }
    }
}

