import XCTest
@testable import AgentRelayiPad

final class VoicePersonaTests: XCTestCase {
    private func preferences() -> VoiceStylePreferences {
        let defaults = UserDefaults(suiteName: "VoicePersonaTests")!
        defaults.removePersistentDomain(forName: "VoicePersonaTests")
        return VoiceStylePreferences(defaults: defaults)
    }

    func testPersonasHaveStableIDsTheBrokerKnows() {
        XCTAssertEqual(VoicePersona.allCases.map(\.rawValue), ["standard", "pirate", "noir", "hype", "narrator"])
        XCTAssertEqual(VoicePersona.noir.title, "Noir detective")
    }

    func testSuggestedVoicesAreGPTLiveVoices() {
        for persona in VoicePersona.allCases {
            XCTAssertNotNil(VoiceLiveVoices.valid(persona.suggestedVoice), persona.rawValue)
        }
        XCTAssertEqual(VoiceLiveVoices.all.count, 23)
        XCTAssertTrue(VoiceLiveVoices.all.contains(VoiceLiveVoices.fallback))
        XCTAssertNil(VoiceLiveVoices.valid("robot"))
    }

    func testOnDeviceStyleStaysInARangeSpeechCanUse() {
        for persona in VoicePersona.allCases {
            XCTAssertTrue((0.8...1.2).contains(persona.onDeviceRate), persona.rawValue)
            XCTAssertTrue((0.75...1.25).contains(persona.onDevicePitch), persona.rawValue)
        }
    }

    func testVoiceFollowsThePersonaUntilChosen() {
        let style = preferences()
        XCTAssertEqual(style.persona, .standard)
        XCTAssertEqual(style.voice, "marin")
        style.persona = .pirate
        XCTAssertEqual(style.voice, VoicePersona.pirate.suggestedVoice)
        XCTAssertFalse(style.voiceChosen)
        style.voice = "willow"
        XCTAssertTrue(style.voiceChosen)
        style.persona = .narrator
        XCTAssertEqual(style.voice, "willow")
        style.voice = "not-a-voice"
        XCTAssertEqual(style.voice, "willow")
        style.clearVoice()
        XCTAssertEqual(style.voice, VoicePersona.narrator.suggestedVoice)
    }
}
