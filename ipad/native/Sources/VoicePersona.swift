import Foundation

/// How the call sounds: a speaking style for the voice layer. Personas change
/// tone, pace and wording only; the agent's answers and the safety rules stay
/// the same. The Cloud voice broker holds the GPT-Live text for each id, so the
/// app sends only the id. On-device voice approximates them with rate and pitch.
enum VoicePersona: String, CaseIterable, Identifiable, Codable {
    case standard
    case pirate
    case noir
    case hype
    case narrator

    var id: String { rawValue }

    var title: String {
        switch self {
        case .standard: return "Standard"
        case .pirate: return "Pirate"
        case .noir: return "Noir detective"
        case .hype: return "Hype-man"
        case .narrator: return "Calm narrator"
        }
    }

    var summary: String {
        switch self {
        case .standard: return "Clear and friendly."
        case .pirate: return "Salty sea-dog slang, still accurate."
        case .noir: return "Hard-boiled, moody, rain on the window."
        case .hype: return "Big energy, celebrates every green build."
        case .narrator: return "Slow, soothing, documentary calm."
        }
    }

    /// A voice that suits the persona, used until the person picks one.
    var suggestedVoice: String {
        switch self {
        case .standard: return "marin"
        case .pirate: return "ash"
        case .noir: return "cedar"
        case .hype: return "verse"
        case .narrator: return "sage"
        }
    }

    /// On-device approximation: multipliers for `AVSpeechUtterance` rate and pitch.
    var onDeviceRate: Float {
        switch self {
        case .standard, .pirate: return 1.0
        case .noir: return 0.9
        case .hype: return 1.12
        case .narrator: return 0.85
        }
    }

    var onDevicePitch: Float {
        switch self {
        case .standard: return 1.0
        case .pirate: return 0.85
        case .noir: return 0.8
        case .hype: return 1.2
        case .narrator: return 0.95
        }
    }
}

/// GPT-Live's built-in voices, from the `session.audio.output.voice` reference
/// (read October 28, 2026). `marin` is GPT-Live's default.
enum VoiceLiveVoices {
    static let all = [
        "alloy", "ash", "ballad", "beacon", "bossa", "cedar", "cinder", "coral", "delta", "echo", "gleam",
        "marin", "meridian", "quartz", "ripple", "sage", "shida", "shimmer", "stone", "tempo", "verse",
        "vesper", "willow",
    ]
    static let fallback = "marin"

    static func valid(_ name: String?) -> String? {
        guard let name, all.contains(name) else { return nil }
        return name
    }

    static func title(_ name: String) -> String { name.prefix(1).uppercased() + name.dropFirst() }
}

/// The person's chosen voice and persona, stored on this device.
struct VoiceStylePreferences {
    static let personaKey = "relay.voiceCall.persona"
    static let voiceKey = "relay.voiceCall.voice"
    let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) { self.defaults = defaults }

    var persona: VoicePersona {
        get { defaults.string(forKey: Self.personaKey).flatMap(VoicePersona.init(rawValue:)) ?? .standard }
        nonmutating set { defaults.set(newValue.rawValue, forKey: Self.personaKey) }
    }

    /// The chosen GPT-Live voice, or the persona's suggestion if none was chosen.
    var voice: String {
        get { VoiceLiveVoices.valid(defaults.string(forKey: Self.voiceKey)) ?? persona.suggestedVoice }
        nonmutating set {
            if let valid = VoiceLiveVoices.valid(newValue) { defaults.set(valid, forKey: Self.voiceKey) }
        }
    }

    /// Whether the person picked a voice themselves (otherwise it follows the persona).
    var voiceChosen: Bool { VoiceLiveVoices.valid(defaults.string(forKey: Self.voiceKey)) != nil }

    func clearVoice() { defaults.removeObject(forKey: Self.voiceKey) }
}
