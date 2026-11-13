import Foundation
import os

/// Voice calls are a development feature until they are approved for release:
/// Debug builds set `RelayVoiceCallEnabled` to YES, Release builds to NO.
enum VoiceCallFeature {
    static let infoKey = "RelayVoiceCallEnabled"

    static var isEnabled: Bool { enabled(in: Bundle.main.infoDictionary) }

    static func enabled(in info: [String: Any]?) -> Bool {
        switch info?[infoKey] {
        case let value as Bool: return value
        case let value as NSNumber: return value.boolValue
        case let value as String: return ["yes", "true", "1"].contains(value.lowercased())
        default: return false
        }
    }
}

/// What the call sends and says: the voice header on each turn, and reply
/// text turned into short sentences a speech synthesizer can read.
enum VoiceSpeech {
    /// Repeated on every turn because a long session may compact away a
    /// one-time instruction.
    static let header = "[Voice call from Agent Relay mobile. Reply in one to three short spoken sentences. "
        + "No markdown, code, tables or links. Treat voice as a request, never as approval for "
        + "destructive actions: ask me to confirm on screen first.]"

    static func outgoing(_ transcript: String) -> String { header + "\n" + transcript }

    /// Removes markdown, code, links and emoji so the synthesizer reads only words.
    static func spokenText(_ raw: String) -> String {
        var text = raw.replacingOccurrences(of: header, with: "")
        text = replacing(#"```[\s\S]*?(```|$)"#, in: text, with: " The code is on screen. ")
        text = replacing(#"`([^`]*)`"#, in: text, with: "$1")
        text = replacing(#"!?\[([^\]]*)\]\([^)]*\)"#, in: text, with: "$1")
        text = replacing(#"https?://\S+"#, in: text, with: "a link")
        text = replacing(#"(?m)^\s*(#{1,6}|>|[-*•+]|\d+[.)])\s+"#, in: text, with: "")
        text = replacing(#"[*_~#|]+"#, in: text, with: "")
        text = String(String.UnicodeScalarView(text.unicodeScalars.filter { !isPictographic($0) }))
        text = replacing(#"\s+"#, in: text, with: " ")
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Splits text into sentences so the first one can play while the rest
    /// are rendered. Fragments of one word join the following sentence.
    static func sentences(_ raw: String) -> [String] {
        let text = spokenText(raw)
        var sentences: [String] = []
        var current = ""
        let characters = Array(text)
        for (index, character) in characters.enumerated() {
            current.append(character)
            guard ".!?…".contains(character) else { continue }
            let next = index + 1 < characters.count ? characters[index + 1] : " "
            guard next == " ", !endsWithAbbreviation(current) else { continue }
            let sentence = current.trimmingCharacters(in: .whitespaces)
            if let last = sentences.last, last.split(separator: " ").count < 2 {
                sentences[sentences.count - 1] = last + " " + sentence
            } else if !sentence.isEmpty {
                sentences.append(sentence)
            }
            current = ""
        }
        let rest = current.trimmingCharacters(in: .whitespaces)
        if rest.contains(where: { $0.isLetter || $0.isNumber }) {
            if let last = sentences.last, last.split(separator: " ").count < 2 {
                sentences[sentences.count - 1] = last + " " + rest
            } else {
                sentences.append(rest)
            }
        }
        return sentences.filter { $0.contains(where: { $0.isLetter || $0.isNumber }) }
    }

    private static let abbreviations: Set<String> = ["e.g.", "i.e.", "etc.", "vs.", "mr.", "mrs.", "ms.", "dr.", "st.", "no."]

    private static func endsWithAbbreviation(_ text: String) -> Bool {
        guard let word = text.split(separator: " ").last?.lowercased() else { return false }
        return abbreviations.contains(word)
    }

    private static func isPictographic(_ scalar: Unicode.Scalar) -> Bool {
        if scalar.value == 0x200D || (0xFE00...0xFE0F).contains(scalar.value) { return true }
        // Digits, # and * are emoji too; only drop real pictographs.
        return scalar.properties.isEmojiPresentation || (scalar.properties.isEmoji && scalar.value >= 0x2100)
    }

    private static func replacing(_ pattern: String, in text: String, with template: String) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return text }
        let range = NSRange(text.startIndex..., in: text)
        return regex.stringByReplacingMatches(in: text, range: range, withTemplate: template)
    }
}

/// Short utterances that control the call instead of going to the agent.
/// Only a whole utterance counts, so "stop the server" still reaches the agent.
enum VoiceCommand: Equatable {
    case endCall, stopSpeaking, none

    static func parse(_ transcript: String) -> VoiceCommand {
        let words = VoiceWords.words(transcript).joined(separator: " ")
        if ["end call", "end the call", "hang up", "goodbye", "bye", "bye bye"].contains(words) { return .endCall }
        if ["stop", "wait", "hold on", "pause", "quiet", "be quiet", "shush", "hey", "ok stop", "okay stop"].contains(words) { return .stopSpeaking }
        return .none
    }
}

/// A guard, not the security boundary: a transcript that sounds like it could
/// delete, ship or spend something waits for an on-screen tap before it is sent.
enum VoiceActionGuard {
    private static let exact: Set<String> = [
        "rm", "pay", "pays", "paid", "paying", "kill", "kills", "killed", "wipe", "wipes", "wiped",
        "drop", "drops", "dropped", "push", "pushes", "pushed", "ship", "ships", "shipped",
    ]
    private static let stems = [
        "delet", "remov", "eras", "destroy", "truncat", "deploy", "releas", "publish", "merg",
        "force", "reset", "revert", "rollback", "terminat", "revok", "rotat", "uninstall", "purg",
        "overwrit", "rebas", "purchas", "refund", "transfer",
    ]
    private static let phrases = ["shut down", "roll back", "force push", "git push", "rotate keys"]

    /// The first risky word or phrase in the transcript, or nil.
    static func risk(in transcript: String) -> String? {
        let words = VoiceWords.words(transcript)
        let joined = " " + words.joined(separator: " ") + " "
        if let phrase = phrases.first(where: { joined.contains(" \($0) ") }) { return phrase }
        return words.first { word in exact.contains(word) || stems.contains { word.hasPrefix($0) } }
    }
}

enum VoiceWords {
    static func words(_ text: String) -> [String] {
        text.lowercased()
            .components(separatedBy: CharacterSet.letters.union(.decimalDigits).union(CharacterSet(charactersIn: "'")).inverted)
            .filter { !$0.isEmpty }
    }
}

/// Energy-based voice activity detection over echo-cancelled input levels.
struct VoiceActivityDetector {
    enum Event: Equatable { case started, ended }

    /// Level in dBFS above which a buffer counts as speech.
    var threshold: Float
    /// Speech must last this long before it counts, which filters clicks.
    var startDuration: TimeInterval
    /// Silence that ends an utterance.
    var hangover: TimeInterval
    private(set) var speaking = false
    private var above: TimeInterval = 0
    private var below: TimeInterval = 0

    init(threshold: Float = -42, startDuration: TimeInterval = 0.12, hangover: TimeInterval = 0.7) {
        self.threshold = threshold
        self.startDuration = startDuration
        self.hangover = hangover
    }

    mutating func process(level: Float, duration: TimeInterval) -> Event? {
        if level >= threshold {
            above += duration
            below = 0
            if !speaking, above >= startDuration {
                speaking = true
                return .started
            }
        } else {
            below += duration
            if !speaking { above = 0 }
            if speaking, below >= hangover {
                speaking = false
                above = 0
                return .ended
            }
        }
        return nil
    }

    /// Treats speech as already started, for barge-in, which detected it with
    /// its own thresholds.
    mutating func markSpeaking() {
        speaking = true
        above = 0
        below = 0
    }

    mutating func reset() {
        speaking = false
        above = 0
        below = 0
    }

    /// Root-mean-square level of the samples in dBFS (-160 for silence).
    static func level<C: Collection>(of samples: C) -> Float where C.Element == Float {
        guard !samples.isEmpty else { return -160 }
        let meanSquare = samples.reduce(Float(0)) { $0 + $1 * $1 } / Float(samples.count)
        guard meanSquare > 0 else { return -160 }
        return max(-160, 10 * log10(meanSquare))
    }
}

/// Whether a call may use CallKit. Apple does not allow CallKit in China
/// mainland (App Review guideline 5), so CallKit is never active when either
/// signal says China mainland:
/// - the device region (`Locale.current.region?.identifier`) is `CN`, or
/// - the App Store storefront (StoreKit 2 `Storefront.countryCode`, ISO 3166-1
///   alpha-3) is `CHN`.
/// Without CallKit the call runs as plain VoIP with the background audio mode
/// and no system call UI. An unknown storefront (no App Store account, the
/// Simulator, or a failed lookup) is not China, so the region decides. Hong
/// Kong (`HK`, `HKG`), Macau (`MO`, `MAC`) and Taiwan (`TW`, `TWN`) keep
/// CallKit.
enum VoiceCallPolicy {
    static func allowsCallKit(regionIdentifier: String?, storefrontCountryCode: String?) -> Bool {
        if let storefront = normalized(storefrontCountryCode), chinaMainland.contains(storefront) { return false }
        if let region = normalized(regionIdentifier), chinaMainland.contains(region) { return false }
        return true
    }

    /// China mainland as a region identifier (alpha-2) and as a storefront (alpha-3).
    private static let chinaMainland: Set<String> = ["CN", "CHN"]

    private static func normalized(_ code: String?) -> String? {
        guard let code = code?.trimmingCharacters(in: .whitespacesAndNewlines).uppercased(), !code.isEmpty else { return nil }
        return code
    }
}

/// Step-by-step call logging, readable in Console.app (subsystem
/// com.agentworkforce.relayipad, category voice-call) for a connected iPhone.
enum VoiceCallLog {
    static let logger = Logger(subsystem: "com.agentworkforce.relayipad", category: "voice-call")
}

/// How iOS ended a call through CallKit, and what the call screen says.
enum VoiceCallSystemEnd: String, Equatable {
    /// An end action: the system call UI, another call, or iOS itself.
    case ended = "ended"
    /// CallKit reset the app's provider, which ends every call.
    case reset = "reset"

    var notice: String {
        switch self {
        case .ended: return "iOS ended the call. If you didn't end it from the call screen, call again."
        case .reset: return "iOS reset the call. Call again to reconnect."
        }
    }

    /// Only an action for the call's own ID ends it: an action for an earlier
    /// call, or the one this call sent itself when hanging up, does not.
    static func isCurrent(_ id: UUID, callID: UUID?) -> Bool {
        callID == id
    }
}

/// Whether a CallKit start that waited for the storefront lookup may go on.
/// Ending the call bumps its generation, so a hang-up or sign-out during the
/// wait (when the call has no CallKit ID to end yet) stops the start.
enum VoiceCallKitStart {
    static func proceeds(started generation: Int, now current: Int) -> Bool {
        generation == current
    }
}

/// Which call a CallKit audio deactivation belongs to, and whether it means
/// the audio is off. CallKit sends one deactivation per activation, in order,
/// but without a call ID, and a released call's can arrive after the next
/// call's activation. So each activation is recorded with its call, each
/// deactivation goes to the oldest one still recorded, and WebRTC lets go of
/// the audio only when no newer call's audio is on. An ended call whose
/// deactivation never came is dropped once a newer call is recorded and
/// `staleAfter` has passed since it ended, so it can't take a later call's.
struct VoiceCallAudioActivations {
    let staleAfter: TimeInterval
    private(set) var calls: [UUID] = []
    /// When recorded calls ended, kept apart from the calls themselves.
    private(set) var endedAt: [UUID: Date] = [:]

    init(staleAfter: TimeInterval = 10) { self.staleAfter = staleAfter }

    /// The call whose audio is on now: the newest recorded.
    var current: UUID? { calls.last }

    mutating func activated(_ call: UUID) {
        calls.removeAll { $0 == call }
        calls.append(call)
        endedAt = endedAt.filter { calls.contains($0.key) }
    }

    mutating func ended(_ call: UUID, at now: Date = Date()) {
        guard calls.contains(call), endedAt[call] == nil else { return }
        endedAt[call] = now
    }

    /// The call this deactivation belongs to, and whether the audio is now
    /// off (WebRTC should let go of it). `isGone` says a call was released
    /// without ending, so it counts as ended long ago.
    mutating func deactivated(at now: Date = Date(), isGone: (UUID) -> Bool = { _ in false })
        -> (call: UUID?, audioOff: Bool) {
        while calls.count > 1, let oldest = calls.first,
              let ended = endedAt[oldest] ?? (isGone(oldest) ? .distantPast : nil),
              now.timeIntervalSince(ended) > staleAfter {
            calls.removeFirst()
            endedAt[oldest] = nil
        }
        guard !calls.isEmpty else { return (nil, true) }
        let call = calls.removeFirst()
        endedAt[call] = nil
        return (call, calls.isEmpty)
    }

    mutating func reset() {
        calls = []
        endedAt = [:]
    }
}

/// Timeouts for one call: a filler while the agent works, when to stop
/// waiting for a reply, and when a silent, held or stuck call ends.
struct VoiceCallTimeouts {
    var filler: TimeInterval = 4
    var giveUp: TimeInterval = 120
    var idle: TimeInterval = 300
    /// On hold after Siri, an alarm or another call took the audio.
    var held: TimeInterval = 15 * 60
    /// Any other state that made no progress, so no call outlives its use.
    var stuck: TimeInterval = 15 * 60

    enum WaitAction: Equatable { case none, filler, giveUp }
    /// What a call is doing, as far as ending it is concerned.
    enum Activity: Equatable { case resting, held, busy }

    func waitAction(waitingFor elapsed: TimeInterval, fillerPlayed: Bool) -> WaitAction {
        if elapsed >= giveUp { return .giveUp }
        if elapsed >= filler, !fillerPlayed { return .filler }
        return .none
    }

    func shouldEnd(idleFor elapsed: TimeInterval) -> Bool { elapsed >= idle }

    /// Why a call with no activity for `elapsed` ends now, or nil to keep it.
    func endCause(_ activity: Activity, idleFor elapsed: TimeInterval) -> VoiceCallEndCause? {
        switch activity {
        case .resting where elapsed >= idle: return .silence
        case .held where elapsed >= held: return .onHold
        case .busy where elapsed >= stuck: return .unresponsive
        default: return nil
        }
    }

}

/// Why a call ended. Every path that ends a call names its cause, and every
/// cause has a reason the call screen shows under "Call ended", so a call
/// never ends with nothing said. The stage word is what the log carries.
enum VoiceCallEndCause: Equatable {
    /// The person tapped End call on the call screen.
    case hungUp
    /// The person said "end call", "hang up", "goodbye" or the like.
    case spokenGoodbye
    /// Calling again (the same agent or another) replaced an on-device call, so GPT-Live could be asked again.
    case replaced
    /// iOS ended or reset the call through CallKit.
    case system(VoiceCallSystemEnd)
    /// The person signed out or changed workspace.
    case signedOut
    case microphoneDenied
    case speechDenied
    /// The person declined GPT-Live, and there is no call without it.
    case consentDeclined
    /// The on-device audio engine could not start.
    case audioFailed(String)
    /// The audio stopped while the call was running.
    case audioStopped
    /// The audio route (headphones, Bluetooth) went away.
    case audioRouteLost
    case silence
    case onHold
    case unresponsive
    /// A held GPT-Live call could not be resumed.
    case resumeFailed

    /// What the call screen shows under "Call ended".
    var notice: String {
        switch self {
        case .hungUp: return "You ended the call."
        case .spokenGoodbye: return "You ended the call by voice."
        case .replaced: return "This call was replaced by a new call."
        case .system(let end): return end.notice
        case .signedOut: return "Call ended because you signed out or changed workspace."
        case .microphoneDenied: return "Allow microphone access for Agent Relay in Settings to make voice calls."
        case .speechDenied: return "Allow speech recognition for Agent Relay in Settings to make voice calls."
        case .consentDeclined: return VoiceLiveConsent.declined
        case .audioFailed(let detail): return "The call's audio couldn't start: \(detail)"
        case .audioStopped: return "Audio stopped unexpectedly. Start the call again."
        case .audioRouteLost: return "The call lost its audio route. Start it again."
        case .silence: return "Call ended after 5 minutes of silence."
        case .onHold: return "Call ended after 15 minutes on hold."
        case .unresponsive: return "Call ended because it stopped responding. Call again to reconnect."
        case .resumeFailed: return "The call could not resume. Start it again."
        }
    }

    /// One word for Console, so the log says which path ended the call.
    var stage: String {
        switch self {
        case .hungUp: return "hung-up"
        case .spokenGoodbye: return "spoken-goodbye"
        case .replaced: return "replaced"
        case .system(let end): return "system-\(end.rawValue)"
        case .signedOut: return "signed-out"
        case .microphoneDenied: return "microphone-denied"
        case .speechDenied: return "speech-denied"
        case .consentDeclined: return "consent-declined"
        case .audioFailed: return "audio-failed"
        case .audioStopped: return "audio-stopped"
        case .audioRouteLost: return "audio-route-lost"
        case .silence: return "silence"
        case .onHold: return "on-hold"
        case .unresponsive: return "unresponsive"
        case .resumeFailed: return "resume-failed"
        }
    }

    /// Every cause, iOS's two included, for tests that check each has a reason. Kept by hand: a new case must be added here.
    static let all: [VoiceCallEndCause] = [
        .hungUp, .spokenGoodbye, .replaced, .system(.ended), .system(.reset), .signedOut, .microphoneDenied, .speechDenied,
        .consentDeclined, .audioFailed("engine"), .audioStopped, .audioRouteLost, .silence, .onHold, .unresponsive, .resumeFailed,
    ]
}

/// What the call's first read of the conversation found, told apart so a
/// cold start is handled right: no thread yet is a known, empty baseline
/// (the first reply will be read); a failed read is unknown (nothing is read
/// until a read succeeds, so an old conversation is not read aloud).
enum VoiceConversationLoad: Equatable {
    case messages([VoiceTranscriptMessage])
    /// The agent has no DM thread with this device yet; the first request starts one.
    case noThread
    /// The thread, or its messages, could not be read.
    case failed

    /// The messages a reply check tracks: the thread's, none for a thread
    /// that does not exist yet, and nil when the read failed, so the baseline
    /// is left as it was.
    var messagesToTrack: [VoiceTranscriptMessage]? {
        switch self {
        case .messages(let messages): return messages
        case .noThread: return []
        case .failed: return nil
        }
    }

    /// The message IDs the conversation holds, or nil when that is not known.
    var knownIDs: [String]? { messagesToTrack?.map(\.id) }

    /// What the log says about it: which case, not how much was said with whom.
    var summary: String {
        switch self {
        case .messages: return "conversation loaded"
        case .noThread: return "no conversation yet; the first request starts one"
        case .failed: return "conversation didn't load; replies are read once it does"
        }
    }
}

/// The DM thread between this device and an agent, found in the relay's
/// conversation list without the inbox having been opened: the one thread
/// whose participants are exactly this device and the agent.
enum VoiceDirectThread {
    /// What a lookup found: the thread (cached, or found in the list), no
    /// thread yet (the list was read and holds nothing with the agent), or a
    /// failed read (the list could not be read, it cannot say which thread,
    /// or this device does not know its own handle).
    enum Resolution: Equatable {
        case id(String)
        case noThread
        case failed
    }

    /// What the list holds for an agent.
    enum Match: Equatable {
        case one(String)
        /// Nothing with the agent in it.
        case none
        /// The agent is in the list, but not in exactly one clean 1:1 thread:
        /// two candidates, or only a thread with others in it. The list
        /// cannot say, and a guess could read the wrong conversation aloud.
        case unresolved
    }

    /// Resolves the thread: the cached id when there is one (no read), else
    /// from the list, which is nil when it could not be read. Without this
    /// device's own handle the list cannot be read safely: unknown, never a guess.
    static func resolve(cached: String?, threads: [[String: Any]]?, peer: String, mine: String) -> Resolution {
        if let cached, !cached.isEmpty { return .id(cached) }
        guard let threads, !mine.isEmpty else { return .failed }
        switch match(in: threads, peer: peer, mine: mine) {
        case .one(let id): return .id(id)
        case .none: return .noThread
        case .unresolved: return .failed
        }
    }

    /// Whether a lookup started in one session may still be used: the same
    /// session, still paired, not signing out. A lookup suspended across a
    /// sign-out or a workspace switch must neither fill the new session's
    /// cache nor send the old thread id to the new workspace.
    static func stillCurrent(started session: Int, now current: Int, paired: Bool, signingOut: Bool) -> Bool {
        session == current && paired && !signingOut
    }

    /// The 1:1 thread with `peer`: exactly one other participant, and exactly
    /// one such thread. The inbox keys a thread by its first other
    /// participant, so a thread that holds the agent beside others is reported
    /// as unresolved, never as "no thread": a call must not start from an
    /// empty baseline over a conversation the inbox may cache for the agent.
    static func match(in threads: [[String: Any]], peer: String, mine: String) -> Match {
        var pairs: [String] = []
        var holdsPeer = false
        for thread in threads {
            guard let id = thread["id"] as? String, !id.isEmpty else { continue }
            let participants = (thread["participants"] as? [[String: Any]] ?? []).compactMap { $0["agent_name"] as? String }
            let others = participants.filter { $0 != mine }
            if others.contains(peer) { holdsPeer = true }
            if others == [peer] { pairs.append(id) }
        }
        switch pairs.count {
        case 1: return .one(pairs[0])
        case 0: return holdsPeer ? .unresolved : .none
        default: return .unresolved
        }
    }

}

/// When the list last held no thread with an agent, so a call that has not
/// said anything yet does not re-read the list on every reply check. A miss
/// holds for `hold`; a call that starts forgets it, so a thread created
/// meanwhile is found; and a miss is not recorded within `afterSend` of a
/// request to the agent, whose first message is what creates the thread.
struct VoiceThreadMisses {
    static let hold: TimeInterval = 30
    static let afterSend: TimeInterval = 60

    /// What the list said last time, while it still holds.
    enum Hold: Equatable {
        /// Nothing with the agent: a known, empty conversation.
        case none
        /// The list was read but could not identify one thread: unknown.
        case unresolved
    }

    private(set) var missedAt: [String: (at: Date, hold: Hold)] = [:]
    private(set) var sentAt: [String: Date] = [:]

    /// Nothing recorded: after a reset, or before any lookup.
    var isEmpty: Bool { missedAt.isEmpty && sentAt.isEmpty }

    /// The answer that still holds for the agent, or nil when the list should be read now.
    func hold(_ agent: String, now: Date = Date()) -> Hold? {
        guard let missed = missedAt[agent] else { return nil }
        let elapsed = now.timeIntervalSince(missed.at)
        // A clock set back must not pin the hold open.
        return elapsed < 0 || elapsed >= Self.hold ? nil : missed.hold
    }

    /// The list was read and holds nothing with the agent. Not recorded
    /// within `afterSend` of a request to the agent, whose first message
    /// creates the thread.
    mutating func record(miss agent: String, now: Date = Date()) {
        if let sent = sentAt[agent], now.timeIntervalSince(sent) < Self.afterSend, now >= sent { return }
        missedAt[agent] = (now, .none)
    }

    /// The list was read and could not identify one thread: the answer is
    /// unknown, held so the list is not re-read on every reply check.
    mutating func record(unresolved agent: String, now: Date = Date()) {
        missedAt[agent] = (now, .unresolved)
    }

    mutating func found(_ agent: String) { missedAt[agent] = nil }

    mutating func sent(to agent: String, now: Date = Date()) {
        missedAt[agent] = nil
        sentAt[agent] = now
    }

    /// A call is starting: whatever the list said last time, it is asked again.
    mutating func forget(_ agent: String) { missedAt[agent] = nil }

    mutating func reset() {
        missedAt = [:]
        sentAt = [:]
    }
}

/// The call's startup steps run only while the call is still starting: a
/// hang-up, a system end or a replacement during a load or a connect cancels
/// what follows, and the stage log says so.
enum VoiceCallStartup {
    /// What the call does with its first conversation load once it is back.
    enum AfterLoad: Equatable {
        /// The call ended (or moved on) while the load was in flight: no
        /// baseline is taken from it and nothing else starts.
        case cancelled
        /// The call goes on: where reply tracking starts (nil: unknown until
        /// a load succeeds), and what the rejoin snapshot records.
        case proceed(heard: [String]?, known: [String])
    }

    static func cancelled(phaseIsStarting: Bool) -> Bool { !phaseIsStarting }

    /// The decision the controller takes after `start()`'s load returns.
    static func afterLoad(_ load: VoiceConversationLoad, rejoined: [String]?, phaseIsStarting: Bool) -> AfterLoad {
        guard !cancelled(phaseIsStarting: phaseIsStarting) else { return .cancelled }
        let loaded = load.knownIDs
        return .proceed(heard: VoiceCallBaseline.heardMessageIDs(loaded: loaded, rejoined: rejoined),
                        known: VoiceCallBaseline.knownMessageIDs(loaded: loaded, rejoined: rejoined) ?? [])
    }
}

/// Whether the workspace shows the call bar, and what an ended one says.
enum VoiceCallBarPolicy {
    /// A live call shows the bar whenever its screen is closed; an ended call
    /// only when its reason has not been seen (it ended while its screen was
    /// closed, or a workspace switch ended it), and never over the sign-in
    /// screen.
    static func shows(ended: Bool, screenPresented: Bool, paired: Bool, endedWhileHidden: Bool) -> Bool {
        guard !screenPresented, paired else { return false }
        return ended ? endedWhileHidden : true
    }

    /// Whether closing the call screen forgets an ended call: only one whose
    /// end the screen showed. A call that ended unseen, or one a workspace
    /// switch ended while the screen was open and closed, keeps its reason
    /// for the bar until that is dismissed.
    static func forgetsOnScreenClose(ended: Bool, endedWhileHidden: Bool) -> Bool { ended && !endedWhileHidden }

    /// The ended bar's text: the reason, led by "Call ended" unless it already says so.
    static func endedText(notice: String?) -> String {
        guard let notice, !notice.isEmpty else { return "Call ended · no reason recorded" }
        return notice.hasPrefix("Call ended") ? notice : "Call ended · \(notice)"
    }
}

/// A conversation message reduced to what the call needs.
struct VoiceTranscriptMessage: Equatable {
    let id: String
    let text: String
    let mine: Bool
}

/// Finds agent replies the call has not spoken yet. Everything already in the
/// conversation when the call starts counts as heard.
/// What a call counts as already in the conversation. Nil means unknown: the
/// conversation hasn't loaded (offline, or the relay read failed), so the call
/// reads no replies and saves no rejoin snapshot until a load succeeds, rather
/// than treating every message as new and reading them all aloud.
enum VoiceCallBaseline {
    /// Saved in the rejoin snapshot: the latest load, else what the call had
    /// seen before the relaunch.
    static func knownMessageIDs(loaded: [String]?, rejoined: [String]?) -> [String]? {
        loaded ?? rejoined
    }

    /// Where reply tracking starts: a rejoined call keeps what it had seen
    /// before the relaunch, so replies that arrived meanwhile are read.
    static func heardMessageIDs(loaded: [String]?, rejoined: [String]?) -> [String]? {
        rejoined ?? loaded
    }
}

struct VoiceReplyTracker {
    private var known: Set<String>

    init(existing: [VoiceTranscriptMessage]) {
        known = Set(existing.map(\.id))
    }

    mutating func newReplies(in messages: [VoiceTranscriptMessage]) -> [VoiceTranscriptMessage] {
        var replies: [VoiceTranscriptMessage] = []
        for message in messages where !known.contains(message.id) {
            known.insert(message.id)
            if !message.mine, !message.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                replies.append(message)
            }
        }
        return replies
    }
}

/// Short tones played through the call's own audio engine, so echo
/// cancellation removes them from the microphone as well.
enum VoiceCue {
    case listening, heard, ended

    private var notes: [(frequency: Double, duration: Double)] {
        switch self {
        case .listening: return [(660, 0.07), (880, 0.09)]
        case .heard: return [(880, 0.06)]
        case .ended: return [(660, 0.08), (440, 0.12)]
        }
    }

    /// Mono samples at the given rate with short fades so the tones do not click.
    func samples(sampleRate: Double, amplitude: Float = 0.18) -> [Float] {
        var output: [Float] = []
        for note in notes {
            let count = Int(note.duration * sampleRate)
            let fade = max(1, min(count / 4, Int(0.008 * sampleRate)))
            for index in 0..<count {
                let envelope = Float(min(1, Double(min(index, count - 1 - index)) / Double(fade)))
                let phase = 2 * Double.pi * note.frequency * Double(index) / sampleRate
                output.append(amplitude * envelope * Float(sin(phase)))
            }
            output.append(contentsOf: repeatElement(0, count: Int(0.02 * sampleRate)))
        }
        return output
    }
}

/// A speech synthesis voice installed on the device, as the fallback voice
/// picker sees it.
struct VoiceCandidate: Equatable {
    enum Quality: Int {
        case standard = 1, enhanced, premium
    }

    let identifier: String
    let language: String
    let quality: Quality
    /// Novelty and Personal Voice voices are never picked.
    var novelty = false
}

/// Picks the voice the on-device fallback reads replies with: the best quality
/// installed voice for the device's language (premium, then enhanced, then the
/// default compact voice), preferring the exact region when quality ties.
enum VoicePicker {
    static func best(_ candidates: [VoiceCandidate], for language: String) -> VoiceCandidate? {
        let wanted = normalized(language)
        let base = wanted.split(separator: "-").first.map(String.init) ?? wanted
        var best: (voice: VoiceCandidate, rank: (Int, Int))?
        for voice in candidates where !voice.novelty {
            let code = normalized(voice.language)
            guard code == base || code.hasPrefix(base + "-") else { continue }
            let rank = (voice.quality.rawValue, code == wanted ? 1 : 0)
            if best == nil || rank > best!.rank { best = (voice, rank) }
        }
        return best?.voice
    }

    private static func normalized(_ code: String) -> String {
        code.replacingOccurrences(of: "_", with: "-").lowercased()
    }
}
