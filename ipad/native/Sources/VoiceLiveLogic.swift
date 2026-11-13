import Foundation

/// Events GPT-Live sends on the WebRTC data channel that the call uses.
/// See docs/specs/ios-voice-call.md and OpenAI's GPT-Live guides.
enum VoiceLiveEvent: Equatable {
    case started
    case closed(seconds: Double?)
    case usage(seconds: Double)
    case inputTranscript(String, startMs: Int)
    case outputTranscript(String, startMs: Int)
    /// A client delegation: GPT-Live asks the application to do the work.
    case delegation(id: String, offsetMs: Int)
    case error(code: String, message: String)
    case other(String)

    static func parse(_ data: Data) -> VoiceLiveEvent? {
        guard let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = event["type"] as? String else { return nil }
        func int(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? 0 }
        func seconds(_ value: Any?) -> Double? { ((value as? [String: Any])?["seconds"] as? NSNumber)?.doubleValue }
        switch type {
        case "session.started":
            return .started
        case "session.closed":
            return .closed(seconds: seconds(event["usage"]))
        case "session.usage.updated":
            return seconds(event["usage"]).map { .usage(seconds: $0) } ?? .other(type)
        case "session.input_transcript.delta":
            return .inputTranscript(event["delta"] as? String ?? "", startMs: int(event["start_ms"]))
        case "session.output_transcript.delta":
            return .outputTranscript(event["delta"] as? String ?? "", startMs: int(event["start_ms"]))
        case "session.delegation.created":
            let delegation = event["delegation"] as? [String: Any] ?? [:]
            guard let id = delegation["id"] as? String, !id.isEmpty,
                  (delegation["target"] as? String ?? "client") == "client" else { return .other(type) }
            return .delegation(id: id, offsetMs: int(event["offset_ms"]))
        case "error":
            let error = event["error"] as? [String: Any] ?? [:]
            return .error(code: error["code"] as? String ?? "", message: error["message"] as? String ?? "")
        default:
            return .other(type)
        }
    }
}

/// Events the call sends to GPT-Live.
enum VoiceLiveCommand {
    /// Context GPT-Live may use later without saying it now.
    case thinking(delegationID: String?, content: String)
    /// A result GPT-Live should say aloud, in its own words.
    case commentary(delegationID: String?, content: String)
    case close

    func data(eventID: String = "ar_\(UUID().uuidString)") -> Data {
        var body: [String: Any] = ["event_id": eventID]
        switch self {
        case .thinking(let id, let content), .commentary(let id, let content):
            if case .thinking = self { body["type"] = "session.thinking.append" } else { body["type"] = "session.commentary.append" }
            body["delegation_id"] = id ?? NSNull()
            body["content"] = content
        case .close:
            body["type"] = "session.close"
        }
        return (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data()
    }
}

/// The spoken conversation as GPT-Live transcribes it. Delegation events carry
/// no request text, so the call works out the request from what the person
/// said since the previous delegation.
struct VoiceLiveTranscript {
    struct Fragment: Equatable {
        let text: String
        let startMs: Int
        let fromPerson: Bool
        /// Which GPT-Live session transcribed it; offsets restart in each one.
        var session = 0
    }

    private(set) var fragments: [Fragment] = []
    private var delegatedUntilMs = -1
    private var session = 0

    mutating func add(_ text: String, startMs: Int, fromPerson: Bool) {
        guard !text.isEmpty else { return }
        fragments.append(Fragment(text: text, startMs: startMs, fromPerson: fromPerson, session: session))
        if fragments.count > 2_000 { fragments.removeFirst(fragments.count - 2_000) }
    }

    /// A new GPT-Live session (resume or reconnect) starts its offsets at zero.
    /// Earlier text stays in the history but is never re-sent as a request.
    mutating func startNewSession() {
        session += 1
        delegatedUntilMs = -1
    }

    /// What the person asked for since the previous delegation. A short reply
    /// such as "yes, do that" carries what the assistant said just before it.
    mutating func request(at offsetMs: Int) -> String {
        let start = delegatedUntilMs
        delegatedUntilMs = max(delegatedUntilMs, offsetMs)
        let current = fragments.filter { $0.session == session }
        var said = Self.joined(current.filter { $0.fromPerson && $0.startMs > start && $0.startMs <= offsetMs })
        if said.isEmpty {
            // Delegated before any new words were transcribed: use the last thing said.
            said = Self.joined(Array(current.filter(\.fromPerson).suffix(20)))
        }
        guard !said.isEmpty else { return "" }
        if VoiceWords.words(said).count <= 4,
           let firstOfRequest = current.firstIndex(where: { $0.fromPerson && $0.startMs > start && $0.startMs <= offsetMs })
            ?? current.lastIndex(where: { $0.fromPerson && $0.startMs <= offsetMs }) {
            let before = current[..<firstOfRequest]
            let assistant = Self.joined(Array(before.reversed().prefix { !$0.fromPerson }.reversed()))
            if !assistant.isEmpty { return "\(said)\n(Replying to: \"\(Self.tail(assistant, 300))\")" }
        }
        return said
    }

    /// Recent conversation as text, for a session that resumes this one.
    func history(limit: Int = 20) -> [(fromPerson: Bool, text: String)] {
        var turns: [(fromPerson: Bool, text: String)] = []
        for fragment in fragments {
            if let last = turns.last, last.fromPerson == fragment.fromPerson {
                turns[turns.count - 1].text += fragment.text
            } else {
                turns.append((fragment.fromPerson, fragment.text))
            }
        }
        return turns.suffix(limit).map { ($0.fromPerson, Self.tail($0.text.trimmingCharacters(in: .whitespacesAndNewlines), 2_000)) }
            .filter { !$0.text.isEmpty }
    }

    private static func joined(_ fragments: [Fragment]) -> String {
        fragments.map(\.text).joined().trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func tail(_ text: String, _ limit: Int) -> String {
        text.count <= limit ? text : "…" + text.suffix(limit)
    }
}

enum VoiceLiveReply {
    /// GPT-Live accepts up to 500 tokens per append; stay well below it.
    static let tokenLimit = 400

    /// A conservative token estimate: about 3 ASCII characters per token, and
    /// one token per other character (CJK and emoji often take one or more).
    static func estimatedTokens(_ text: String) -> Int {
        var ascii = 0, other = 0
        for scalar in text.unicodeScalars { if scalar.isASCII { ascii += 1 } else { other += 1 } }
        return (ascii + 2) / 3 + other
    }

    /// The agent's reply prepared for GPT-Live to say: markdown, code and links
    /// removed, and long replies cut at a sentence with a pointer to the chat.
    static func commentary(_ reply: String, agentName: String) -> String {
        let text = VoiceSpeech.spokenText(reply)
        let prefix = "\(agentName) replied: "
        guard estimatedTokens(prefix + text) > tokenLimit else { return prefix + text }
        let suffix = " The full reply is in the chat."
        let budget = tokenLimit - estimatedTokens(prefix + suffix)
        var kept = ""
        for sentence in VoiceSpeech.sentences(text) {
            let next = kept.isEmpty ? sentence : kept + " " + sentence
            guard estimatedTokens(next) <= budget else { break }
            kept = next
        }
        if kept.isEmpty {
            for character in text {
                guard estimatedTokens(kept + String(character)) < budget else { break }
                kept.append(character)
            }
            kept += "…"
        }
        return prefix + kept + suffix
    }
}

/// Which path a call uses: GPT-Live voice, or on-device speech.
enum VoiceCallRoute: Equatable {
    case live
    case onDevice(reason: String)

    /// Calls always use GPT-Live once the person has agreed to it; on-device
    /// voice is only the fallback when GPT-Live can't run (see `fallbackReason`).
    /// Without consent there is no call: `nil`.
    static func choose(consented: Bool) -> VoiceCallRoute? {
        consented ? .live : nil
    }

    /// What the person is told when the broker refuses or fails to start a session.
    static func fallbackReason(status: Int?, code: String?) -> String {
        // The broker answers 429 for two different refusals, so the code decides.
        // A 429 without one (a token refresh or the edge) is not about calls.
        switch (status, code) {
        case (_, "voice_quota_exceeded"?):
            return "You've used this month's GPT-Live minutes, so this call falls back to on-device voice."
        case (_, "voice_session_limit"?):
            return "Too many GPT-Live calls are still open on your account, so this call falls back to on-device voice."
        case (429?, _):
            return "Agent Relay is busy right now, so this call falls back to on-device voice."
        case (503?, _), (_, "voice_live_unavailable"?), (404?, _):
            return "GPT-Live voice isn't available, so this call falls back to on-device voice."
        default:
            return "GPT-Live voice couldn't connect, so this call falls back to on-device voice."
        }
    }
}

/// GPT-Live sessions whose usage the broker hasn't accepted yet. A session
/// holds one of the account's two GPT-Live slots until its usage is reported,
/// so every session is recorded the moment it is granted and kept on this
/// device until a report is accepted, even if the app is killed mid-call.
struct VoiceLiveUsageLedger {
    static let key = "relay.voiceCall.unreportedUsage"
    /// This run of the app; sessions from an earlier run can't still be open in it.
    static let launchID = UUID().uuidString

    struct Entry: Codable, Equatable {
        var seconds: Double
        var launchID: String
        /// The call let go of the session, so its usage is final.
        var finished: Bool
    }

    let defaults: UserDefaults
    let launchID: String

    init(defaults: UserDefaults = .standard, launchID: String = VoiceLiveUsageLedger.launchID) {
        self.defaults = defaults
        self.launchID = launchID
    }

    var entries: [String: Entry] {
        guard let data = defaults.data(forKey: Self.key),
              let entries = try? JSONDecoder().decode([String: Entry].self, from: data) else { return [:] }
        return entries
    }

    private func save(_ entries: [String: Entry]) {
        if entries.isEmpty { return defaults.removeObject(forKey: Self.key) }
        if let data = try? JSONEncoder().encode(entries) { defaults.set(data, forKey: Self.key) }
    }

    /// A session the broker just granted, still in use.
    func open(_ sessionID: String) {
        guard !sessionID.isEmpty else { return }
        var all = entries
        if all[sessionID] == nil { all[sessionID] = Entry(seconds: 0, launchID: launchID, finished: false) }
        save(all)
    }

    /// The session's billed seconds so far, so a killed call still reports them.
    func update(_ sessionID: String, seconds: Double) {
        guard var entry = entries[sessionID], seconds > entry.seconds else { return }
        entry.seconds = seconds
        var all = entries
        all[sessionID] = entry
        save(all)
    }

    /// The call let go of the session; returns the seconds to report.
    @discardableResult
    func finish(_ sessionID: String, seconds: Double) -> Double {
        guard !sessionID.isEmpty else { return seconds }
        var all = entries
        var entry = all[sessionID] ?? Entry(seconds: 0, launchID: launchID, finished: true)
        entry.seconds = max(entry.seconds, seconds)
        entry.finished = true
        all[sessionID] = entry
        save(all)
        return entry.seconds
    }

    func remove(_ sessionID: String) {
        var all = entries
        guard all.removeValue(forKey: sessionID) != nil else { return }
        save(all)
    }

    /// Sessions a failed start left open on the broker: the one it granted
    /// before the WebRTC answer failed, and the one it named when it couldn't
    /// confirm closing a session it opened.
    static func sessionsLeftByFailedStart(granted: String?, unconfirmed: String?) -> [String] {
        var sessions: [String] = []
        for id in [granted, unconfirmed] {
            if let id, !id.isEmpty, !sessions.contains(id) { sessions.append(id) }
        }
        return sessions
    }

    /// Sessions to report now: finished ones, and any an earlier run of the
    /// app left open (it was killed, crashed or updated during the call).
    var reportable: [String: Double] {
        entries.filter { $0.value.finished || $0.value.launchID != launchID }.mapValues(\.seconds)
    }
}

/// An account may hold two GPT-Live sessions, so a call that replaces its
/// session (a new voice or persona, resuming, reconnecting) lets the old one
/// go first: its usage report closes it on the broker and frees its slot.
/// The wait is short: on a dead network the report can take 25 s, and the
/// ledger retries a report that didn't land, so the call moves on.
enum VoiceLiveHandover {
    static let wait: Duration = .seconds(3)

    /// `retryWhen` is asked after a failed start that didn't wait for the
    /// old session's report: when it says the broker refused because both
    /// slots were full, the start is tried once more if the report lands
    /// within another `limit`, so the whole handover waits at most twice it.
    @MainActor
    static func replace(releasing old: Task<Void, Never>?, waitingAtMost limit: Duration = wait,
                        then start: @MainActor () async -> Bool,
                        retryWhen slotsWereFull: @MainActor () -> Bool = { false }) async -> Bool {
        var released = true
        if let old { released = await finished(old, within: limit) }
        if await start() { return true }
        guard !released, let old, slotsWereFull(), await finished(old, within: limit) else { return false }
        return await start()
    }

    /// Whether the task finished within the limit; it keeps going either way.
    /// (A task group would wait for both, and awaiting a task can't be cancelled.)
    @MainActor
    private static func finished(_ task: Task<Void, Never>, within limit: Duration) async -> Bool {
        await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            let resume = ResumeOnce(continuation)
            let timer = Task { @MainActor in
                try? await Task.sleep(for: limit)
                resume(false)
            }
            Task { @MainActor in
                await task.value
                timer.cancel()
                resume(true)
            }
        }
    }

    @MainActor private final class ResumeOnce {
        private var continuation: CheckedContinuation<Bool, Never>?
        init(_ continuation: CheckedContinuation<Bool, Never>) { self.continuation = continuation }
        func callAsFunction(_ value: Bool) {
            continuation?.resume(returning: value)
            continuation = nil
        }
    }
}

/// GPT-Live bills silence, so an idle live session is closed and resumed later.
struct VoiceLiveIdlePolicy {
    var idleAfter: TimeInterval = 60

    func shouldClose(lastActivity: Date, pendingDelegations: Int, now: Date = Date()) -> Bool {
        pendingDelegations == 0 && now.timeIntervalSince(lastActivity) >= idleAfter
    }
}

/// Whether the person agreed to send call audio to OpenAI. Stored on this device.
struct VoiceLivePreferences {
    static let consentKey = "relay.voiceCall.liveConsent"
    /// Earlier builds let people prefer on-device voice. Calls now always use
    /// GPT-Live, so the old choice is removed and ignored.
    static let retiredOnDeviceKey = "relay.voiceCall.preferOnDevice"
    let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        defaults.removeObject(forKey: Self.retiredOnDeviceKey)
    }

    var consented: Bool {
        get { defaults.bool(forKey: Self.consentKey) }
        nonmutating set { defaults.set(newValue, forKey: Self.consentKey) }
    }
}

/// The one-time GPT-Live consent question asked before the first call.
/// Draft wording, pending review (docs/specs/ios-voice-call.md).
enum VoiceLiveConsent {
    static let title = "Use GPT-Live for voice calls?"
    static let disclosure = "GPT-Live from OpenAI makes calls sound natural: you can talk over it, and it answers right away while your agent works. During a GPT-Live call, your voice and the call transcript are sent to OpenAI. Your requests still go to your agent as messages. Agent Relay asks OpenAI not to store the call."
    static let fallback = "If GPT-Live isn't available, the call falls back to on-device voice, which keeps your audio on this device."
    /// Shown when the person declines: there is no call without GPT-Live.
    static let declined = "Voice calls need GPT-Live, so the call didn't start. Call again if you change your mind."
}
