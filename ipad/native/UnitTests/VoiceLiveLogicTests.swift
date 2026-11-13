import XCTest
@testable import AgentRelayiPad

final class VoiceLiveEventTests: XCTestCase {
    private func parse(_ json: String) -> VoiceLiveEvent? { VoiceLiveEvent.parse(Data(json.utf8)) }

    func testParsesTheEventsTheCallUses() {
        XCTAssertEqual(parse(#"{"type":"session.started","session":{"id":"live_1"}}"#), .started)
        XCTAssertEqual(parse(#"{"type":"session.closed","usage":{"seconds":92}}"#), .closed(seconds: 92))
        XCTAssertEqual(parse(#"{"type":"session.closed"}"#), .closed(seconds: nil))
        XCTAssertEqual(parse(#"{"type":"session.usage.updated","usage":{"seconds":12}}"#), .usage(seconds: 12))
        XCTAssertEqual(parse(#"{"type":"session.input_transcript.delta","delta":"What is","start_ms":1000,"end_ms":1200}"#),
                       .inputTranscript("What is", startMs: 1000))
        XCTAssertEqual(parse(#"{"type":"session.output_transcript.delta","delta":"Sure","start_ms":1500}"#),
                       .outputTranscript("Sure", startMs: 1500))
        XCTAssertEqual(parse(#"{"type":"session.delegation.created","offset_ms":2000,"delegation":{"id":"item_1","type":"delegation","target":"client"}}"#),
                       .delegation(id: "item_1", offsetMs: 2000))
        XCTAssertEqual(parse(#"{"type":"error","error":{"code":"immutable_field_update","message":"No."}}"#),
                       .error(code: "immutable_field_update", message: "No."))
        XCTAssertEqual(parse(#"{"type":"response.event"}"#), .other("response.event"))
    }

    func testIgnoresMalformedAndNonClientDelegations() {
        XCTAssertNil(parse("not json"))
        XCTAssertNil(parse(#"{"no":"type"}"#))
        XCTAssertEqual(parse(#"{"type":"session.delegation.created","delegation":{"id":"r1","target":"responses"}}"#),
                       .other("session.delegation.created"))
    }
}

final class VoiceLiveCommandTests: XCTestCase {
    private func object(_ command: VoiceLiveCommand) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: command.data(eventID: "e1"))) as? [String: Any] ?? [:]
    }

    func testAppendsCarryTheDelegationAndContent() {
        let commentary = object(.commentary(delegationID: "item_1", content: "It shipped."))
        XCTAssertEqual(commentary["type"] as? String, "session.commentary.append")
        XCTAssertEqual(commentary["delegation_id"] as? String, "item_1")
        XCTAssertEqual(commentary["content"] as? String, "It shipped.")
        XCTAssertEqual(commentary["event_id"] as? String, "e1")

        let thinking = object(.thinking(delegationID: nil, content: ""))
        XCTAssertEqual(thinking["type"] as? String, "session.thinking.append")
        XCTAssertTrue(thinking["delegation_id"] is NSNull)
    }

    func testClose() {
        XCTAssertEqual(object(.close)["type"] as? String, "session.close")
    }
}

final class VoiceLiveTranscriptTests: XCTestCase {
    func testRequestIsWhatThePersonSaidSinceThePreviousDelegation() {
        var transcript = VoiceLiveTranscript()
        transcript.add("What's failing ", startMs: 100, fromPerson: true)
        transcript.add("in CI?", startMs: 400, fromPerson: true)
        XCTAssertEqual(transcript.request(at: 900), "What's failing in CI?")
        transcript.add("Checking.", startMs: 1000, fromPerson: false)
        transcript.add("And the iOS build too.", startMs: 2000, fromPerson: true)
        XCTAssertEqual(transcript.request(at: 2500), "And the iOS build too.")
    }

    func testShortRepliesCarryWhatTheAssistantJustSaid() {
        var transcript = VoiceLiveTranscript()
        transcript.add("Fix the flaky test.", startMs: 100, fromPerson: true)
        _ = transcript.request(at: 500)
        transcript.add("Should I also update ", startMs: 1000, fromPerson: false)
        transcript.add("the snapshot?", startMs: 1200, fromPerson: false)
        transcript.add("Yes ", startMs: 2000, fromPerson: true)
        transcript.add("do that.", startMs: 2100, fromPerson: true)
        XCTAssertEqual(transcript.request(at: 2500), "Yes do that.\n(Replying to: \"Should I also update the snapshot?\")")
    }

    func testDelegationBeforeNewWordsUsesTheLastRequest() {
        var transcript = VoiceLiveTranscript()
        transcript.add("Summarize the PR.", startMs: 100, fromPerson: true)
        _ = transcript.request(at: 500)
        XCTAssertEqual(transcript.request(at: 600), "Summarize the PR.")
        XCTAssertEqual(VoiceLiveTranscript().history().count, 0)
    }

    func testANewSessionNeverResendsEarlierRequests() {
        var transcript = VoiceLiveTranscript()
        transcript.add("Deploy staging.", startMs: 5_000, fromPerson: true)
        XCTAssertEqual(transcript.request(at: 6_000), "Deploy staging.")
        transcript.startNewSession()
        // Offsets restart at zero in the new session.
        transcript.add("What's the status?", startMs: 200, fromPerson: true)
        XCTAssertEqual(transcript.request(at: 500), "What's the status?")
        XCTAssertEqual(transcript.history().first?.text, "Deploy staging.What's the status?")
    }

    func testHistoryGroupsTurns() {
        var transcript = VoiceLiveTranscript()
        transcript.add("Hi ", startMs: 0, fromPerson: true)
        transcript.add("there.", startMs: 100, fromPerson: true)
        transcript.add("Hello.", startMs: 300, fromPerson: false)
        let history = transcript.history()
        XCTAssertEqual(history.map(\.fromPerson), [true, false])
        XCTAssertEqual(history.map(\.text), ["Hi there.", "Hello."])
    }
}

final class VoiceLiveReplyTests: XCTestCase {
    func testShortRepliesAreCleanedAndAttributed() {
        XCTAssertEqual(VoiceLiveReply.commentary("**Done.** Tests pass ✅", agentName: "builder"),
                       "builder replied: Done. Tests pass")
    }

    func testLongRepliesStopAtASentenceAndPointToTheChat() {
        let reply = Array(repeating: "This sentence describes one more change in detail.", count: 80).joined(separator: " ")
        let text = VoiceLiveReply.commentary(reply, agentName: "builder")
        XCTAssertLessThanOrEqual(VoiceLiveReply.estimatedTokens(text), VoiceLiveReply.tokenLimit)
        XCTAssertTrue(text.hasPrefix("builder replied: This sentence"))
        XCTAssertTrue(text.hasSuffix("detail. The full reply is in the chat."))
    }

    func testOneHugeSentenceIsCut() {
        let text = VoiceLiveReply.commentary(String(repeating: "word ", count: 1_000), agentName: "a")
        XCTAssertLessThanOrEqual(VoiceLiveReply.estimatedTokens(text), VoiceLiveReply.tokenLimit)
        XCTAssertTrue(text.hasSuffix("… The full reply is in the chat."))
    }

    func testNonEnglishRepliesStayUnderTheTokenLimit() {
        let reply = String(repeating: "构建已经完成，所有测试都通过了。", count: 60)
        let text = VoiceLiveReply.commentary(reply, agentName: "builder")
        XCTAssertLessThanOrEqual(VoiceLiveReply.estimatedTokens(text), VoiceLiveReply.tokenLimit)
        XCTAssertLessThan(text.count, 450)
        XCTAssertTrue(text.hasSuffix("The full reply is in the chat."))
    }
}

final class VoiceCallRouteTests: XCTestCase {
    func testCallsUseGPTLiveOnceConsentedAndNeverStartWithout() {
        XCTAssertEqual(VoiceCallRoute.choose(consented: true), .live)
        XCTAssertNil(VoiceCallRoute.choose(consented: false))
    }

    func testFallbackReasons() {
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: 429, code: "voice_quota_exceeded").contains("this month's GPT-Live minutes"))
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: 429, code: "voice_session_limit").contains("Too many GPT-Live calls"))
        XCTAssertFalse(VoiceCallRoute.fallbackReason(status: 429, code: nil).contains("minutes"))
        // Only the broker's call limit blames open calls; a 429 from a token
        // refresh or the edge doesn't.
        XCTAssertFalse(VoiceCallRoute.fallbackReason(status: 429, code: nil).contains("Too many GPT-Live calls"))
        XCTAssertFalse(VoiceCallRoute.fallbackReason(status: 429, code: "rate_limited").contains("Too many GPT-Live calls"))
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: 429, code: nil).contains("busy"))
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: 503, code: "voice_live_unavailable").contains("isn't available"))
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: 502, code: "voice_live_upstream").contains("couldn't connect"))
        XCTAssertTrue(VoiceCallRoute.fallbackReason(status: nil, code: nil).contains("couldn't connect"))
        for status in [429, 503, 502] {
            XCTAssertTrue(VoiceCallRoute.fallbackReason(status: status, code: nil).hasSuffix("falls back to on-device voice."))
        }
    }

    func testDecliningExplainsThatCallsNeedGPTLive() {
        XCTAssertTrue(VoiceLiveConsent.declined.contains("Voice calls need GPT-Live"))
        XCTAssertTrue(VoiceLiveConsent.fallback.contains("falls back to on-device voice"))
    }

    func testIdleSessionsCloseOnlyWithNothingPending() {
        let policy = VoiceLiveIdlePolicy(idleAfter: 60)
        let now = Date()
        XCTAssertFalse(policy.shouldClose(lastActivity: now.addingTimeInterval(-30), pendingDelegations: 0, now: now))
        XCTAssertTrue(policy.shouldClose(lastActivity: now.addingTimeInterval(-60), pendingDelegations: 0, now: now))
        XCTAssertFalse(policy.shouldClose(lastActivity: now.addingTimeInterval(-600), pendingDelegations: 1, now: now))
    }

    func testPreferencesPersist() {
        let defaults = UserDefaults(suiteName: "VoiceLivePreferencesTests")!
        defaults.removePersistentDomain(forName: "VoiceLivePreferencesTests")
        let preferences = VoiceLivePreferences(defaults: defaults)
        XCTAssertFalse(preferences.consented)
        preferences.consented = true
        XCTAssertTrue(VoiceLivePreferences(defaults: defaults).consented)
    }

    func testEarlierOnDeviceChoiceIsRetired() {
        let name = "VoiceLivePreferencesTests.retired"
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        // Someone who agreed to GPT-Live, then chose on-device voice in an earlier build.
        defaults.set(true, forKey: "relay.voiceCall.liveConsent")
        defaults.set(true, forKey: "relay.voiceCall.preferOnDevice")
        let preferences = VoiceLivePreferences(defaults: defaults)
        XCTAssertNil(defaults.object(forKey: VoiceLivePreferences.retiredOnDeviceKey))
        XCTAssertEqual(VoiceCallRoute.choose(consented: preferences.consented), .live)

        // Someone who chose on-device voice without agreeing is asked first.
        defaults.removePersistentDomain(forName: name)
        defaults.set(true, forKey: "relay.voiceCall.preferOnDevice")
        let unconsented = VoiceLivePreferences(defaults: defaults)
        XCTAssertNil(defaults.object(forKey: VoiceLivePreferences.retiredOnDeviceKey))
        XCTAssertNil(VoiceCallRoute.choose(consented: unconsented.consented))
    }
}

final class VoiceCallSnapshotTests: XCTestCase {
    func testSnapshotsRoundTripAndExpire() {
        let defaults = UserDefaults(suiteName: "VoiceCallSnapshotTests")!
        defaults.removePersistentDomain(forName: "VoiceCallSnapshotTests")
        let now = Date()
        let snapshot = VoiceCallSnapshot(agentID: "relay:builder", agentName: "builder", updatedAt: now,
                                         history: [.init(fromPerson: true, text: "What's failing?")],
                                         knownMessageIDs: ["m1", "m2"])
        snapshot.save(defaults)
        XCTAssertEqual(VoiceCallSnapshot.load(defaults), snapshot)
        XCTAssertTrue(snapshot.isRecent(now: now.addingTimeInterval(60)))
        XCTAssertFalse(snapshot.isRecent(now: now.addingTimeInterval(VoiceCallSnapshot.rejoinWindow)))
        XCTAssertFalse(snapshot.isRecent(now: now.addingTimeInterval(-120)))
        VoiceCallSnapshot.clear(defaults)
        XCTAssertNil(VoiceCallSnapshot.load(defaults))
    }
}

final class VoiceLiveReconnectTests: XCTestCase {
    func testRetriesForAboutThirtySecondsThenStops() {
        let total = VoiceLiveReconnect.delays.reduce(0, +)
        XCTAssertGreaterThanOrEqual(total, 25)
        XCTAssertLessThanOrEqual(total, 40)
        XCTAssertEqual(VoiceLiveReconnect.delay(afterAttempt: 0), 0.5)
        XCTAssertNil(VoiceLiveReconnect.delay(afterAttempt: VoiceLiveReconnect.delays.count))
    }
}

final class VoiceLiveUsageLedgerTests: XCTestCase {
    private var defaults: UserDefaults!

    override func setUp() {
        defaults = UserDefaults(suiteName: "VoiceLiveUsageLedgerTests")
        defaults.removePersistentDomain(forName: "VoiceLiveUsageLedgerTests")
    }

    func testAnOpenSessionIsKeptButNotReportedDuringItsOwnRun() {
        let ledger = VoiceLiveUsageLedger(defaults: defaults, launchID: "run-1")
        ledger.open("live_1")
        ledger.update("live_1", seconds: 42)
        XCTAssertEqual(ledger.entries["live_1"]?.seconds, 42)
        XCTAssertTrue(ledger.reportable.isEmpty)
    }

    func testASessionLeftOpenByAKilledRunIsReportedOnTheNextLaunch() {
        let killed = VoiceLiveUsageLedger(defaults: defaults, launchID: "run-1")
        killed.open("live_1")
        killed.update("live_1", seconds: 90)
        let next = VoiceLiveUsageLedger(defaults: defaults, launchID: "run-2")
        XCTAssertEqual(next.reportable, ["live_1": 90])
        next.remove("live_1")
        XCTAssertTrue(next.entries.isEmpty)
        XCTAssertNil(defaults.data(forKey: VoiceLiveUsageLedger.key))
    }

    func testFinishedSessionsAreReportedWithTheirHighestUsage() {
        let ledger = VoiceLiveUsageLedger(defaults: defaults, launchID: "run-1")
        ledger.open("live_1")
        ledger.update("live_1", seconds: 60)
        XCTAssertEqual(ledger.finish("live_1", seconds: 30), 60)
        ledger.update("live_1", seconds: 20)
        XCTAssertEqual(ledger.reportable, ["live_1": 60])
        // A failed start the broker opened is finished without being opened first.
        XCTAssertEqual(ledger.finish("live_2", seconds: 0), 0)
        XCTAssertEqual(ledger.reportable, ["live_1": 60, "live_2": 0])
    }

    func testAFailedStartReportsEverySessionItLeftOpen() {
        // The WebRTC answer failed after the broker granted a session.
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: "live_1", unconfirmed: nil), ["live_1"])
        // The broker couldn't confirm closing a session it opened.
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: nil, unconfirmed: "live_2"), ["live_2"])
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: "live_1", unconfirmed: "live_1"), ["live_1"])
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: "live_1", unconfirmed: "live_2"), ["live_1", "live_2"])
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: nil, unconfirmed: nil), [])
        XCTAssertEqual(VoiceLiveUsageLedger.sessionsLeftByFailedStart(granted: "", unconfirmed: nil), [])
    }

    @MainActor
    func testAReplacedSessionIsReportedBeforeTheNextOneStarts() async {
        var events: [String] = []
        let release = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(50))
            events.append("reported old")
        }
        let started = await VoiceLiveHandover.replace(releasing: release) {
            events.append("started new")
            return true
        }
        XCTAssertTrue(started)
        XCTAssertEqual(events, ["reported old", "started new"])
        // Nothing to release: the new session starts at once.
        let alone = await VoiceLiveHandover.replace(releasing: nil) { false }
        XCTAssertFalse(alone)
    }

    @MainActor
    func testAReportThatHangsDoesNotHoldUpTheNextSession() async {
        // A report on a dead network: the call moves on after the wait.
        let hung = Task { @MainActor in _ = try? await Task.sleep(for: .seconds(60)) }
        let began = ContinuousClock.now
        let started = await VoiceLiveHandover.replace(releasing: hung, waitingAtMost: .milliseconds(50)) { true }
        XCTAssertTrue(started)
        XCTAssertLessThan(ContinuousClock.now - began, .seconds(5))
        hung.cancel()
    }

    @MainActor
    func testARefusalWhileTheOldReportIsLateRetriesOnceItLands() async {
        var events: [String] = []
        // The old report lands only after the first start was refused.
        let (landed, landReport) = AsyncStream<Void>.makeStream()
        let late = Task { @MainActor in
            for await _ in landed { break }
            events.append("reported old")
        }
        var attempts = 0
        let started = await VoiceLiveHandover.replace(releasing: late, waitingAtMost: .milliseconds(50), then: {
            attempts += 1
            events.append("start \(attempts)")
            if attempts == 1 { landReport.yield() }
            return attempts > 1
        }, retryWhen: { true })
        XCTAssertTrue(started)
        XCTAssertEqual(events, ["start 1", "reported old", "start 2"])
    }

    @MainActor
    func testARetryStillWaitsAtMostTwiceTheLimitForAHungReport() async {
        let hung = Task { @MainActor in _ = try? await Task.sleep(for: .seconds(60)) }
        var attempts = 0
        let began = ContinuousClock.now
        let started = await VoiceLiveHandover.replace(releasing: hung, waitingAtMost: .milliseconds(50),
                                                      then: { attempts += 1; return false }, retryWhen: { true })
        XCTAssertFalse(started)
        XCTAssertEqual(attempts, 1)
        XCTAssertLessThan(ContinuousClock.now - began, .seconds(5))
        hung.cancel()
    }

    @MainActor
    func testNoRetryWhenTheOldReportLandedOrTheRefusalWasSomethingElse() async {
        var attempts = 0
        let done = Task { @MainActor in }
        await done.value
        let afterReport = await VoiceLiveHandover.replace(releasing: done, then: { attempts += 1; return false },
                                                          retryWhen: { true })
        XCTAssertFalse(afterReport)
        XCTAssertEqual(attempts, 1)
        attempts = 0
        let late = Task { @MainActor in _ = try? await Task.sleep(for: .milliseconds(100)) }
        let otherRefusal = await VoiceLiveHandover.replace(releasing: late, waitingAtMost: .milliseconds(10),
                                                           then: { attempts += 1; return false }, retryWhen: { false })
        XCTAssertFalse(otherRefusal)
        XCTAssertEqual(attempts, 1)
    }

    func testEmptySessionIDsAreIgnored() {
        let ledger = VoiceLiveUsageLedger(defaults: defaults, launchID: "run-1")
        ledger.open("")
        ledger.finish("", seconds: 5)
        XCTAssertTrue(ledger.entries.isEmpty)
    }
}
