import XCTest
@testable import AgentRelayiPad

final class VoicePickerTests: XCTestCase {
    private func voice(_ id: String, _ language: String, _ quality: VoiceCandidate.Quality, novelty: Bool = false) -> VoiceCandidate {
        VoiceCandidate(identifier: id, language: language, quality: quality, novelty: novelty)
    }

    func testPrefersPremiumThenEnhancedThenDefault() {
        let compact = voice("compact", "en-US", .standard)
        let enhanced = voice("enhanced", "en-US", .enhanced)
        let premium = voice("premium", "en-US", .premium)
        XCTAssertEqual(VoicePicker.best([compact, enhanced, premium], for: "en-US"), premium)
        XCTAssertEqual(VoicePicker.best([compact, enhanced], for: "en-US"), enhanced)
        XCTAssertEqual(VoicePicker.best([compact], for: "en-US"), compact)
    }

    func testOnlyVoicesForTheCurrentLanguage() {
        let french = voice("fr", "fr-FR", .premium)
        let english = voice("en", "en-US", .standard)
        XCTAssertEqual(VoicePicker.best([french, english], for: "en-US"), english)
        XCTAssertNil(VoicePicker.best([french], for: "en-US"))
        XCTAssertNil(VoicePicker.best([], for: "en-US"))
        // "en" must not match a language that merely starts with the same letters.
        XCTAssertNil(VoicePicker.best([voice("x", "enx-US", .premium)], for: "en-US"))
    }

    func testQualityOutranksRegionAndRegionBreaksTies() {
        let british = voice("gb", "en-GB", .premium)
        let american = voice("us", "en-US", .enhanced)
        XCTAssertEqual(VoicePicker.best([american, british], for: "en-US"), british)
        let britishEnhanced = voice("gb", "en-GB", .enhanced)
        XCTAssertEqual(VoicePicker.best([britishEnhanced, american], for: "en_us"), american)
    }

    func testSkipsNoveltyAndPersonalVoices() {
        let novelty = voice("bells", "en-US", .premium, novelty: true)
        let normal = voice("samantha", "en-US", .standard)
        XCTAssertEqual(VoicePicker.best([novelty, normal], for: "en-US"), normal)
    }
}

final class VoiceCallFeatureTests: XCTestCase {
    func testFlagReadsInfoValues() {
        XCTAssertTrue(VoiceCallFeature.enabled(in: [VoiceCallFeature.infoKey: "YES"]))
        XCTAssertTrue(VoiceCallFeature.enabled(in: [VoiceCallFeature.infoKey: true]))
        XCTAssertFalse(VoiceCallFeature.enabled(in: [VoiceCallFeature.infoKey: "NO"]))
        XCTAssertFalse(VoiceCallFeature.enabled(in: [VoiceCallFeature.infoKey: ""]))
        XCTAssertFalse(VoiceCallFeature.enabled(in: [:]))
        XCTAssertFalse(VoiceCallFeature.enabled(in: nil))
    }

    /// Unit tests run the Debug build; the TestFlight workflow checks that the
    /// Release archive ships `RelayVoiceCallEnabled=YES` too.
    func testThisBuildEnablesVoiceCalls() {
        XCTAssertTrue(VoiceCallFeature.isEnabled)
    }
}

final class VoiceSpeechTests: XCTestCase {
    func testOutgoingTurnCarriesTheVoiceHeader() {
        let text = VoiceSpeech.outgoing("what's failing in CI")
        XCTAssertTrue(text.hasPrefix("[Voice call from Agent Relay mobile."))
        XCTAssertTrue(text.hasSuffix("\nwhat's failing in CI"))
        XCTAssertTrue(VoiceSpeech.header.contains("never as approval for destructive actions"))
    }

    func testSpokenTextDropsMarkdownLinksAndEmoji() {
        let spoken = VoiceSpeech.spokenText("## Done ✅\n- **Tests** pass in `swift test` 🎉\nSee [the PR](https://github.com/x/y/pull/1) or https://example.com/a.")
        XCTAssertEqual(spoken, "Done Tests pass in swift test See the PR or a link")
    }

    func testSpokenTextReplacesCodeBlocks() {
        let spoken = VoiceSpeech.spokenText("Here is the fix.\n```swift\nlet x = 1\n```\nRun it again.")
        XCTAssertEqual(spoken, "Here is the fix. The code is on screen. Run it again.")
    }

    func testSpokenTextKeepsDigits() {
        XCTAssertEqual(VoiceSpeech.spokenText("Build 35 passed #1 of 2"), "Build 35 passed 1 of 2")
    }

    func testSentencesSplitOnEndingsButNotNumbersOrAbbreviations() {
        XCTAssertEqual(
            VoiceSpeech.sentences("The build took 3.5 minutes. Tests passed, e.g. the voice ones! Want me to merge it?"),
            ["The build took 3.5 minutes.", "Tests passed, e.g. the voice ones!", "Want me to merge it?"]
        )
    }

    func testOneWordFragmentsJoinTheNextSentence() {
        XCTAssertEqual(VoiceSpeech.sentences("Sure. I'll check the logs now."), ["Sure. I'll check the logs now."])
        XCTAssertEqual(VoiceSpeech.sentences("Okay."), ["Okay."])
    }

    func testTrailingTextWithoutPunctuationIsKept() {
        XCTAssertEqual(VoiceSpeech.sentences("First part. second part without a stop"),
                       ["First part.", "second part without a stop"])
    }

    func testSentencesIgnorePunctuationOnlyText() {
        XCTAssertEqual(VoiceSpeech.sentences("--- ... |"), [])
        XCTAssertEqual(VoiceSpeech.sentences("🎉"), [])
    }
}

final class VoiceCommandTests: XCTestCase {
    func testWholeUtterancesControlTheCall() {
        XCTAssertEqual(VoiceCommand.parse("End call."), .endCall)
        XCTAssertEqual(VoiceCommand.parse("Hang up"), .endCall)
        XCTAssertEqual(VoiceCommand.parse("Stop!"), .stopSpeaking)
        XCTAssertEqual(VoiceCommand.parse("hold on"), .stopSpeaking)
    }

    func testRequestsThatMentionCommandsGoToTheAgent() {
        XCTAssertEqual(VoiceCommand.parse("stop the dev server"), .none)
        XCTAssertEqual(VoiceCommand.parse("wait for CI and then tell me"), .none)
    }
}

final class VoiceActionGuardTests: XCTestCase {
    func testDestructiveRequestsNeedConfirmation() {
        XCTAssertEqual(VoiceActionGuard.risk(in: "Delete the old branches"), "delete")
        XCTAssertEqual(VoiceActionGuard.risk(in: "deploy it to production"), "deploy")
        XCTAssertEqual(VoiceActionGuard.risk(in: "merge the PR"), "merge")
        XCTAssertEqual(VoiceActionGuard.risk(in: "please rm the build folder"), "rm")
        XCTAssertEqual(VoiceActionGuard.risk(in: "shut down the server"), "shut down")
        XCTAssertEqual(VoiceActionGuard.risk(in: "Force-push main"), "force push")
        XCTAssertEqual(VoiceActionGuard.risk(in: "rotate keys for staging"), "rotate keys")
    }

    func testQuestionsAndReadsPassThrough() {
        XCTAssertNil(VoiceActionGuard.risk(in: "What's failing in CI?"))
        XCTAssertNil(VoiceActionGuard.risk(in: "Summarize the payload format"))
        XCTAssertNil(VoiceActionGuard.risk(in: "Explain the dropdown component"))
    }
}

final class VoiceActivityDetectorTests: XCTestCase {
    func testSpeechStartsAfterTheStartDurationAndEndsAfterTheHangover() {
        var detector = VoiceActivityDetector(threshold: -40, startDuration: 0.1, hangover: 0.3)
        XCTAssertNil(detector.process(level: -20, duration: 0.05))
        XCTAssertEqual(detector.process(level: -20, duration: 0.05), .started)
        XCTAssertNil(detector.process(level: -20, duration: 0.05))
        XCTAssertNil(detector.process(level: -60, duration: 0.2))
        XCTAssertEqual(detector.process(level: -60, duration: 0.1), .ended)
        XCTAssertFalse(detector.speaking)
    }

    func testShortClicksAreIgnored() {
        var detector = VoiceActivityDetector(threshold: -40, startDuration: 0.12, hangover: 0.3)
        XCTAssertNil(detector.process(level: -10, duration: 0.05))
        XCTAssertNil(detector.process(level: -70, duration: 0.05))
        XCTAssertNil(detector.process(level: -10, duration: 0.05))
        XCTAssertFalse(detector.speaking)
    }

    func testPausesShorterThanTheHangoverKeepTheUtterance() {
        var detector = VoiceActivityDetector(threshold: -40, startDuration: 0.05, hangover: 0.5)
        XCTAssertEqual(detector.process(level: -20, duration: 0.1), .started)
        XCTAssertNil(detector.process(level: -60, duration: 0.4))
        XCTAssertNil(detector.process(level: -20, duration: 0.1))
        XCTAssertNil(detector.process(level: -60, duration: 0.4))
        XCTAssertEqual(detector.process(level: -60, duration: 0.1), .ended)
    }

    func testBargeInMarksSpeechStartedSoTheUtteranceCanEnd() {
        var detector = VoiceActivityDetector(threshold: -40, startDuration: 0.12, hangover: 0.3)
        detector.markSpeaking()
        XCTAssertTrue(detector.speaking)
        XCTAssertNil(detector.process(level: -20, duration: 0.05))
        XCTAssertEqual(detector.process(level: -60, duration: 0.3), .ended)
    }

    func testLevel() {
        XCTAssertEqual(VoiceActivityDetector.level(of: [Float]()), -160)
        XCTAssertEqual(VoiceActivityDetector.level(of: [Float](repeating: 0, count: 10)), -160)
        XCTAssertEqual(VoiceActivityDetector.level(of: [Float](repeating: 1, count: 10)), 0, accuracy: 0.001)
        XCTAssertEqual(VoiceActivityDetector.level(of: [Float](repeating: 0.1, count: 10)), -20, accuracy: 0.001)
    }
}

final class VoiceCallTimeoutsTests: XCTestCase {
    func testWaitingPlaysOneFillerThenGivesUp() {
        let timeouts = VoiceCallTimeouts(filler: 4, giveUp: 120, idle: 300)
        XCTAssertEqual(timeouts.waitAction(waitingFor: 2, fillerPlayed: false), .none)
        XCTAssertEqual(timeouts.waitAction(waitingFor: 4, fillerPlayed: false), .filler)
        XCTAssertEqual(timeouts.waitAction(waitingFor: 30, fillerPlayed: true), .none)
        XCTAssertEqual(timeouts.waitAction(waitingFor: 120, fillerPlayed: true), .giveUp)
        XCTAssertFalse(timeouts.shouldEnd(idleFor: 299))
        XCTAssertTrue(timeouts.shouldEnd(idleFor: 300))
    }

    func testNoCallOutlivesItsUse() {
        let timeouts = VoiceCallTimeouts()
        XCTAssertNil(timeouts.endCause(.resting, idleFor: 299))
        XCTAssertEqual(timeouts.endCause(.resting, idleFor: 300), .silence)
        XCTAssertEqual(VoiceCallEndCause.silence.notice, "Call ended after 5 minutes of silence.")
        // A held call (Siri, an alarm, another call) used to stay open forever.
        XCTAssertNil(timeouts.endCause(.held, idleFor: 14 * 60))
        XCTAssertEqual(timeouts.endCause(.held, idleFor: 15 * 60), .onHold)
        XCTAssertEqual(VoiceCallEndCause.onHold.notice, "Call ended after 15 minutes on hold.")
        XCTAssertNil(timeouts.endCause(.busy, idleFor: 14 * 60))
        XCTAssertEqual(timeouts.endCause(.busy, idleFor: 15 * 60), .unresponsive)
    }
}

final class VoiceCallSystemEndTests: XCTestCase {
    func testOnlyAnActionForTheCallsOwnIDEndsIt() {
        let call = UUID()
        XCTAssertTrue(VoiceCallSystemEnd.isCurrent(call, callID: call))
        // An earlier call's action, or the one this call sent when hanging up.
        XCTAssertFalse(VoiceCallSystemEnd.isCurrent(UUID(), callID: call))
        XCTAssertFalse(VoiceCallSystemEnd.isCurrent(call, callID: nil))
    }

    func testEveryWayIOSEndsACallSaysSo() {
        XCTAssertTrue(VoiceCallSystemEnd.ended.notice.hasPrefix("iOS ended the call."))
        XCTAssertTrue(VoiceCallSystemEnd.reset.notice.hasPrefix("iOS reset the call."))
        XCTAssertNotEqual(VoiceCallSystemEnd.ended.notice, VoiceCallSystemEnd.reset.notice)
    }
}

final class VoiceCallAudioActivationsTests: XCTestCase {
    private let a = UUID(), b = UUID(), c = UUID()

    func testALateDeactivationForAnEndedCallLeavesTheNewCallsAudioOn() {
        var audio = VoiceCallAudioActivations()
        let now = Date()
        audio.activated(a)
        audio.ended(a, at: now)
        audio.activated(b)
        let late = audio.deactivated(at: now.addingTimeInterval(3))
        XCTAssertEqual(late.call, a)
        XCTAssertFalse(late.audioOff)
        XCTAssertEqual(audio.current, b)
        audio.ended(b, at: now.addingTimeInterval(20))
        let own = audio.deactivated(at: now.addingTimeInterval(21))
        XCTAssertEqual(own.call, b)
        XCTAssertTrue(own.audioOff)
        XCTAssertTrue(audio.endedAt.isEmpty)
    }

    func testAnOrdinaryHangUpTurnsTheAudioOff() {
        var audio = VoiceCallAudioActivations()
        audio.activated(a)
        audio.ended(a)
        XCTAssertTrue(audio.deactivated().audioOff)
        // A deactivation with nothing recorded is the audio going off too.
        XCTAssertTrue(audio.deactivated().audioOff)
    }

    func testAChainOfReplacedCallsKeepsTheLiveCallsAudio() {
        var audio = VoiceCallAudioActivations()
        let now = Date()
        audio.activated(a); audio.ended(a, at: now)
        audio.activated(b); audio.ended(b, at: now)
        audio.activated(c)
        XCTAssertEqual(audio.deactivated(at: now.addingTimeInterval(1)).call, a)
        let second = audio.deactivated(at: now.addingTimeInterval(2))
        XCTAssertEqual(second.call, b)
        XCTAssertFalse(second.audioOff)
        XCTAssertEqual(audio.current, c)
    }

    func testANeverArrivingDeactivationDoesntTakeALaterCallsOwn() {
        var audio = VoiceCallAudioActivations(staleAfter: 10)
        let now = Date()
        audio.activated(a); audio.ended(a, at: now)
        audio.activated(b)
        audio.ended(b, at: now.addingTimeInterval(60))
        let own = audio.deactivated(at: now.addingTimeInterval(61))
        XCTAssertEqual(own.call, b)
        XCTAssertTrue(own.audioOff)
        XCTAssertTrue(audio.endedAt.isEmpty)
    }

    func testACallReleasedWithoutEndingCountsAsEndedLongAgo() {
        var audio = VoiceCallAudioActivations()
        audio.activated(a)
        audio.activated(b)
        let own = audio.deactivated { $0 == self.a }
        XCTAssertEqual(own.call, b)
        XCTAssertTrue(own.audioOff)
    }

    func testARecentEndTimeBeatsTheCallBeingGone() {
        // How the service sees it: A ended and its object is already released.
        var audio = VoiceCallAudioActivations()
        let now = Date()
        audio.activated(a)
        audio.ended(a, at: now)
        audio.activated(b)
        let late = audio.deactivated(at: now.addingTimeInterval(2)) { $0 == self.a }
        XCTAssertEqual(late.call, a)
        XCTAssertFalse(late.audioOff)
        XCTAssertEqual(audio.current, b)
    }

    func testEndTimesOnlyForRecordedCallsAndResetForgetsAll() {
        var audio = VoiceCallAudioActivations()
        audio.ended(a)
        XCTAssertTrue(audio.endedAt.isEmpty)
        audio.activated(a)
        audio.ended(a)
        audio.reset()
        XCTAssertNil(audio.current)
        XCTAssertTrue(audio.endedAt.isEmpty)
    }
}

final class VoiceCallBaselineTests: XCTestCase {
    func testAFailedLoadLeavesTheBaselineUnknown() {
        // Not empty: an empty baseline would read the whole conversation aloud.
        XCTAssertNil(VoiceCallBaseline.knownMessageIDs(loaded: nil, rejoined: nil))
        XCTAssertNil(VoiceCallBaseline.heardMessageIDs(loaded: nil, rejoined: nil))
    }

    func testAnEmptyConversationIsAKnownBaseline() {
        XCTAssertEqual(VoiceCallBaseline.knownMessageIDs(loaded: [], rejoined: nil), [])
        XCTAssertEqual(VoiceCallBaseline.heardMessageIDs(loaded: [], rejoined: nil), [])
    }

    func testARejoinedCallKeepsWhatItHadHeard() {
        // Replies that arrived during the relaunch are still read.
        XCTAssertEqual(VoiceCallBaseline.heardMessageIDs(loaded: ["m1", "m2"], rejoined: ["m1"]), ["m1"])
        XCTAssertEqual(VoiceCallBaseline.knownMessageIDs(loaded: ["m1", "m2"], rejoined: ["m1"]), ["m1", "m2"])
        // A failed load doesn't overwrite the snapshot's record with nothing.
        XCTAssertEqual(VoiceCallBaseline.knownMessageIDs(loaded: nil, rejoined: ["m1"]), ["m1"])
        XCTAssertEqual(VoiceCallBaseline.heardMessageIDs(loaded: nil, rejoined: ["m1"]), ["m1"])
    }
}

final class VoiceCallEndCauseTests: XCTestCase {
    func testEveryWayACallEndsHasAVisibleReasonAndAStage() {
        for cause in VoiceCallEndCause.all {
            XCTAssertFalse(cause.notice.isEmpty, "\(cause)")
            XCTAssertNotEqual(cause.notice, "Call ended", "\(cause) must say why")
            XCTAssertFalse(cause.stage.contains(" "), "\(cause) stage is one word for Console")
        }
        XCTAssertEqual(VoiceCallEndCause.hungUp.notice, "You ended the call.")
        XCTAssertEqual(VoiceCallEndCause.system(.ended).notice, VoiceCallSystemEnd.ended.notice)
        XCTAssertEqual(VoiceCallEndCause.system(.reset).stage, "system-reset")
        XCTAssertEqual(VoiceCallEndCause.consentDeclined.notice, VoiceLiveConsent.declined)
        XCTAssertTrue(VoiceCallEndCause.audioFailed("engine busy").notice.contains("engine busy"))
    }

    func testSpokenEndIsNotCalledAGoodbye() {
        XCTAssertEqual(VoiceCallEndCause.spokenGoodbye.notice, "You ended the call by voice.", "'end call' and 'hang up' are not goodbyes")
    }
}

final class VoiceColdCallTests: XCTestCase {
    private func message(_ id: String, _ text: String, mine: Bool = false) -> VoiceTranscriptMessage {
        VoiceTranscriptMessage(id: id, text: text, mine: mine)
    }

    private let threads: [[String: Any]] = [
        ["id": "t-grok", "participants": [["agent_name": "me-mobile"], ["agent_name": "devopsgrok"]]],
        ["id": "t-claude", "participants": [["agent_name": "claude-cli"], ["agent_name": "me-mobile"]]],
        ["id": "", "participants": [["agent_name": "me-mobile"], ["agent_name": "empty-id"]]],
        ["id": "t-group", "participants": [["agent_name": "me-mobile"], ["agent_name": "grouped"], ["agent_name": "someone-else"]]],
        ["id": "t-twin-a", "participants": [["agent_name": "me-mobile"], ["agent_name": "twin"]]],
        ["id": "t-twin-b", "participants": [["agent_name": "twin"], ["agent_name": "me-mobile"]]],
    ]

    func testAColdCallToAnAgentWithNoThreadStartsFromAnEmptyBaselineAndReadsTheFirstReply() {
        // No thread anywhere: the relay was asked, so "no thread" is known, not a failed read.
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "idle-agent", mine: "me-mobile"), .none)
        let load = VoiceConversationLoad.noThread
        XCTAssertEqual(load.knownIDs, [], "a known, empty conversation")
        XCTAssertEqual(VoiceCallBaseline.heardMessageIDs(loaded: load.knownIDs, rejoined: nil), [])
        XCTAssertEqual(VoiceCallBaseline.knownMessageIDs(loaded: load.knownIDs, rejoined: nil), [])
        // The first spoken request creates the thread; its reply is new and is read.
        var tracker = VoiceReplyTracker(existing: [])
        let afterFirstDM = [message("m1", "[Voice call] what's the status?", mine: true), message("m2", "All green.")]
        XCTAssertEqual(tracker.newReplies(in: afterFirstDM).map(\.id), ["m2"])
        XCTAssertEqual(tracker.newReplies(in: afterFirstDM), [], "read once")
    }

    func testAnExistingThreadIsFoundWithoutTheInboxAndItsHistoryCountsAsHeard() {
        // The inbox never filled the cache, but the relay's conversation list has the thread.
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "devopsgrok", mine: "me-mobile"), .one("t-grok"))
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "claude-cli", mine: "me-mobile"), .one("t-claude"))
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "me-mobile", mine: "me-mobile"), .none, "this device is never its own peer")
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "empty-id", mine: "me-mobile"), .none, "a thread without an id is no thread")
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "grouped", mine: "me-mobile"), .unresolved, "a thread with others in it is not no thread either: the inbox may cache it")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: nil, threads: threads, peer: "grouped", mine: "me-mobile"), .failed)
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "twin", mine: "me-mobile"), .unresolved, "two candidates mean the list cannot say")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: nil, threads: threads, peer: "twin", mine: "me-mobile"), .failed, "and that is unknown, not no thread")
        XCTAssertEqual(VoiceDirectThread.match(in: threads, peer: "idle-agent", mine: "me-mobile"), .none)
        XCTAssertEqual(VoiceDirectThread.resolve(cached: nil, threads: threads, peer: "devopsgrok", mine: ""), .failed, "without this device's own handle the list is unknowable, never a guess")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: "t-warm", threads: nil, peer: "devopsgrok", mine: ""), .id("t-warm"), "a cached thread needs no handle")
        let load = VoiceConversationLoad.messages([message("old1", "earlier reply"), message("old2", "my old question", mine: true)])
        XCTAssertEqual(load.knownIDs, ["old1", "old2"])
        var tracker = VoiceReplyTracker(existing: load.knownIDs!.map { message($0, "", mine: true) })
        XCTAssertEqual(tracker.newReplies(in: [message("old1", "earlier reply"), message("old2", "my old question", mine: true)]), [], "nothing old is read aloud")
        XCTAssertEqual(tracker.newReplies(in: [message("old1", "earlier reply"), message("new", "fresh reply")]).map(\.id), ["new"])
    }

    func testTheLookupResolvesFromTheCacheWithoutReadingAndFromTheListOtherwise() {
        XCTAssertEqual(VoiceDirectThread.resolve(cached: "t-warm", threads: nil, peer: "devopsgrok", mine: "me-mobile"), .id("t-warm"), "a warm cache needs no list")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: nil, threads: nil, peer: "devopsgrok", mine: "me-mobile"), .failed, "a cold cache and no list is a failed read")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: "", threads: threads, peer: "devopsgrok", mine: "me-mobile"), .id("t-grok"), "found in the list")
        XCTAssertEqual(VoiceDirectThread.resolve(cached: nil, threads: threads, peer: "idle-agent", mine: "me-mobile"), .noThread, "the list was read and holds none")
        XCTAssertNotEqual(VoiceDirectThread.Resolution.noThread, .failed)
    }

    func testANoThreadAnswerHoldsBetweenChecksButNotAcrossACallStartASentRequestOrASignOut() {
        let now = Date()
        var misses = VoiceThreadMisses()
        XCTAssertNil(misses.hold("bob", now: now))
        misses.record(miss: "bob", now: now)
        XCTAssertEqual(misses.hold("bob", now: now.addingTimeInterval(3)), VoiceThreadMisses.Hold.none, "held between reply checks")
        XCTAssertNotNil(misses.hold("bob", now: now.addingTimeInterval(29)))
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(30)), "and asked again after the hold")
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(-60)), "a clock set back does not pin the hold open")
        XCTAssertNil(misses.hold("alice", now: now), "per agent")
        // A later call to the same agent asks afresh: a thread created meanwhile is found.
        misses.forget("bob")
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(5)))
        // A request sent to the agent creates the thread: no miss is held for a minute after it.
        misses.record(miss: "bob", now: now)
        misses.sent(to: "bob", now: now.addingTimeInterval(1))
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(2)), "a send clears the miss")
        misses.record(miss: "bob", now: now.addingTimeInterval(3))
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(4)), "and a miss just after a send is not recorded, so the first reply is not delayed")
        misses.record(miss: "bob", now: now.addingTimeInterval(62))
        XCTAssertNotNil(misses.hold("bob", now: now.addingTimeInterval(63)), "a minute after the send a miss holds again")
        misses.found("bob")
        XCTAssertNil(misses.hold("bob", now: now.addingTimeInterval(64)))
        misses.record(miss: "carol", now: now)
        misses.reset()
        XCTAssertTrue(misses.isEmpty, "a sign-out or workspace switch forgets every miss")
        // A clock set back further than the after-send window does not suppress a miss.
        misses.sent(to: "bob", now: now)
        misses.record(miss: "bob", now: now.addingTimeInterval(-120))
        XCTAssertNotNil(misses.hold("bob", now: now.addingTimeInterval(-119)))
        // A list that could not identify one thread holds as unknown, not as no thread.
        var unresolved = VoiceThreadMisses()
        unresolved.record(unresolved: "twin", now: now)
        XCTAssertEqual(unresolved.hold("twin", now: now.addingTimeInterval(5)), .unresolved, "the list is not re-read every check")
        XCTAssertNil(unresolved.hold("twin", now: now.addingTimeInterval(30)))
        var missed = VoiceThreadMisses()
        missed.record(miss: "bob", now: now)
        XCTAssertEqual(missed.hold("bob", now: now), VoiceThreadMisses.Hold.none, "a known, empty conversation holds as such")
        XCTAssertNotEqual(missed.hold("bob", now: now), unresolved.hold("twin", now: now))
    }

    func testACancelDuringTheConversationLoadTakesNoBaselineAndStartsNothing() {
        // The decision the controller takes when its first load comes back, which is where a
        // hang-up, a system end or a replacement during the load is noticed. The controller itself
        // (AVFoundation, CallKit) is not driven here.
        let load = VoiceConversationLoad.messages([message("old", "earlier reply")])
        XCTAssertEqual(VoiceCallStartup.afterLoad(load, rejoined: nil, phaseIsStarting: false), .cancelled, "the call ended while loading: nothing of the load is kept")
        XCTAssertEqual(VoiceCallStartup.afterLoad(.noThread, rejoined: nil, phaseIsStarting: false), .cancelled)
        XCTAssertEqual(VoiceCallStartup.afterLoad(load, rejoined: nil, phaseIsStarting: true), .proceed(heard: ["old"], known: ["old"]))
        XCTAssertEqual(VoiceCallStartup.afterLoad(.noThread, rejoined: nil, phaseIsStarting: true), .proceed(heard: [], known: []), "a cold call starts from a known, empty baseline")
        XCTAssertEqual(VoiceCallStartup.afterLoad(.failed, rejoined: nil, phaseIsStarting: true), .proceed(heard: nil, known: []), "a failed read leaves the baseline unknown")
        XCTAssertEqual(VoiceCallStartup.afterLoad(load, rejoined: ["seen"], phaseIsStarting: true), .proceed(heard: ["seen"], known: ["old"]), "a rejoined call keeps what it had heard")
        XCTAssertTrue(VoiceCallStartup.cancelled(phaseIsStarting: false))
    }

    func testTheCallBarShowsALiveCallAndACallThatEndedUnseenButNeverOverSignIn() {
        XCTAssertTrue(VoiceCallBarPolicy.shows(ended: false, screenPresented: false, paired: true, endedWhileHidden: false))
        XCTAssertFalse(VoiceCallBarPolicy.shows(ended: false, screenPresented: true, paired: true, endedWhileHidden: false), "the screen shows it instead")
        XCTAssertTrue(VoiceCallBarPolicy.shows(ended: true, screenPresented: false, paired: true, endedWhileHidden: true), "ended unseen: the reason has not been seen")
        XCTAssertFalse(VoiceCallBarPolicy.shows(ended: true, screenPresented: false, paired: true, endedWhileHidden: false), "ended on screen: closing the screen does not flash it")
        XCTAssertFalse(VoiceCallBarPolicy.shows(ended: true, screenPresented: false, paired: false, endedWhileHidden: true), "never over the sign-in screen")
        XCTAssertEqual(VoiceCallBarPolicy.endedText(notice: "You ended the call."), "Call ended · You ended the call.")
        XCTAssertEqual(VoiceCallBarPolicy.endedText(notice: "Call ended after 5 minutes of silence."), "Call ended after 5 minutes of silence.", "not said twice")
        XCTAssertEqual(VoiceCallBarPolicy.endedText(notice: nil), "Call ended · no reason recorded")
        // Closing the screen forgets only a call whose end the screen showed.
        XCTAssertTrue(VoiceCallBarPolicy.forgetsOnScreenClose(ended: true, endedWhileHidden: false), "hung up on screen, screen closed: nothing left to show")
        XCTAssertFalse(VoiceCallBarPolicy.forgetsOnScreenClose(ended: true, endedWhileHidden: true), "a workspace switch closed the screen: the bar still owes the reason")
        XCTAssertFalse(VoiceCallBarPolicy.forgetsOnScreenClose(ended: false, endedWhileHidden: false), "a live call minimised stays")
    }

    func testAReplyCheckTracksTheThreadsMessagesNoneForNoThreadAndLeavesTheBaselineOnAFailedRead() {
        XCTAssertEqual(VoiceConversationLoad.messages([message("m1", "hi")]).messagesToTrack, [message("m1", "hi")])
        XCTAssertEqual(VoiceConversationLoad.noThread.messagesToTrack, [])
        XCTAssertNil(VoiceConversationLoad.failed.messagesToTrack)
        XCTAssertEqual(VoiceConversationLoad.messages([message("m1", "hi")]).summary, "conversation loaded", "the log never says how much was said")
    }

    func testALookupSuspendedAcrossASignOutOrWorkspaceSwitchIsDiscarded() {
        // Started in session 3: usable only while session 3 is still the paired, signed-in one.
        XCTAssertTrue(VoiceDirectThread.stillCurrent(started: 3, now: 3, paired: true, signingOut: false))
        XCTAssertFalse(VoiceDirectThread.stillCurrent(started: 3, now: 4, paired: true, signingOut: false), "a sign-out or workspace switch bumped the session")
        XCTAssertFalse(VoiceDirectThread.stillCurrent(started: 3, now: 3, paired: true, signingOut: true), "signing out right now")
        XCTAssertFalse(VoiceDirectThread.stillCurrent(started: 3, now: 3, paired: false, signingOut: false), "no longer paired")
    }

    func testAFailedReadIsNotMistakenForAnEmptyConversation() {
        let load = VoiceConversationLoad.failed
        XCTAssertNil(load.knownIDs)
        XCTAssertNil(VoiceCallBaseline.heardMessageIDs(loaded: load.knownIDs, rejoined: nil), "no baseline: nothing is read until a read succeeds")
        XCTAssertNotEqual(VoiceConversationLoad.failed, VoiceConversationLoad.noThread)
        XCTAssertTrue(VoiceConversationLoad.failed.summary.contains("didn't load"))
        XCTAssertTrue(VoiceConversationLoad.noThread.summary.contains("no conversation yet"))
    }

    func testAHangUpCauseHasAReasonAndAnUnreadConversationLeavesNoBaseline() {
        // The pieces a startup hang-up is built from, not the controller itself (it
        // needs AVFoundation and CallKit): the reason the screen shows, and that a
        // conversation never read leaves no baseline. The controller's own
        // startup-cancel path has no automated coverage yet.
        let cause = VoiceCallEndCause.hungUp
        XCTAssertEqual(cause.notice, "You ended the call.")
        XCTAssertEqual(cause.stage, "hung-up")
        XCTAssertNil(VoiceCallBaseline.knownMessageIDs(loaded: nil, rejoined: nil))
        // iOS ending a call that was still connecting says so too, never a bare "Call ended".
        XCTAssertTrue(VoiceCallEndCause.system(.ended).notice.hasPrefix("iOS ended the call"))
    }
}

@MainActor final class VoiceCallServiceTests: XCTestCase {
    func testEndingTheCurrentCallForASignOutClosesItsScreenAndForgetsIt() {
        let defaults = UserDefaults(suiteName: "VoiceCallServiceTests")!
        defaults.removePersistentDomain(forName: "VoiceCallServiceTests")
        let service = VoiceCallService(defaults: defaults)
        service.screenPresented = true
        service.endCurrent(.signedOut)
        XCTAssertFalse(service.screenPresented, "no empty call sheet is left presented")
        XCTAssertNil(service.current)
        XCTAssertFalse(service.endedWhileHidden)
        // With no call at all, keeping the reason keeps nothing, and it is a safe no-op.
        service.screenPresented = true
        service.endCurrent(.signedOut, keepReason: true)
        XCTAssertFalse(service.screenPresented)
        XCTAssertNil(service.current)
        XCTAssertFalse(service.endedWhileHidden, "no call, so no unseen reason")
        // Closing a screen with no call is a no-op too.
        service.screenClosed()
        XCTAssertNil(service.current)
    }
}

final class VoiceReplyTrackerTests: XCTestCase {
    private func message(_ id: String, _ text: String, mine: Bool = false) -> VoiceTranscriptMessage {
        VoiceTranscriptMessage(id: id, text: text, mine: mine)
    }

    func testOnlyNewAgentMessagesAreSpokenOnce() {
        var tracker = VoiceReplyTracker(existing: [message("1", "old reply")])
        let history = [message("1", "old reply"), message("2", "my question", mine: true), message("3", "new reply")]
        XCTAssertEqual(tracker.newReplies(in: history).map(\.id), ["3"])
        XCTAssertEqual(tracker.newReplies(in: history), [])
        XCTAssertEqual(tracker.newReplies(in: history + [message("4", "   ")]), [])
        XCTAssertEqual(tracker.newReplies(in: history + [message("5", "and another")]).map(\.id), ["5"])
    }
}

final class VoiceCueTests: XCTestCase {
    func testCuesAreShortQuietAndStartSilent() {
        for cue in [VoiceCue.listening, .heard, .ended] {
            let samples = cue.samples(sampleRate: 24_000)
            XCTAssertGreaterThan(samples.count, 1_000)
            XCTAssertLessThan(Double(samples.count) / 24_000, 0.4)
            XCTAssertLessThanOrEqual(samples.map(abs).max() ?? 1, 0.18)
            XCTAssertEqual(samples.first ?? 1, 0, accuracy: 0.0001)
        }
    }
}
