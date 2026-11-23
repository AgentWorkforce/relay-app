import XCTest
@testable import AgentRelayiPad

/// Synthetic questions in the relay core's format; no real content.
final class QuestionCardTests: XCTestCase {
    private let ref = "q0a1b2c3d"
    private let agent = "planner"

    private func question(_ blocks: [String], several: Bool = false, ref: String? = nil, agent: String? = nil,
                          minutes: String = "30") -> String {
        let ref = ref ?? self.ref
        let reply = several
            ? "Reply: answer \(ref) <n|text>; <n|text> (one per question, in order). Expires in \(minutes) min; the agent continues without it."
            : "Reply: answer \(ref) <n|text>. Expires in \(minutes) min; the agent continues without it."
        return (["[Question \(ref) from @\(agent ?? self.agent) on test-mini]", "Working on: Tidying the build",
                 "session s-fixture"] + blocks + [reply]).joined(separator: "\n")
    }

    private func parse(_ text: String, sender: String? = nil, mine: Bool = false) -> RelayQuestion? {
        RelayQuestionParser.parse(text, sender: sender ?? agent, mine: mine, chatAgent: agent)
    }

    // MARK: Parser

    func testEveryHintLine() throws {
        let cases: [(String, RelayQuestion.Selection, Bool, Bool)] = [
            ("free text", .none, true, false),
            ("Choose one or more (comma-separated numbers), or answer in free text.", .multiple, true, true),
            ("Choose one or more (comma-separated numbers).", .multiple, false, true),
            ("Choose a number, or answer in free text.", .single, true, true),
            ("Choose a number.", .single, false, true),
        ]
        for (hint, selection, freeText, hasOptions) in cases {
            let block = ["Which target should the build use?"] + (hasOptions ? ["1. Debug", "2. Release"] : []) + [hint]
            let parsed = try XCTUnwrap(parse(question(block)), hint)
            XCTAssertEqual(parsed.items.count, 1)
            XCTAssertEqual(parsed.items[0].selection, selection, hint)
            XCTAssertEqual(parsed.items[0].allowsFreeText, freeText, hint)
            XCTAssertEqual(parsed.items[0].options.count, hasOptions ? 2 : 0, hint)
        }
    }

    func testOneQuestionWithHeaderAndDescriptions() throws {
        let parsed = try XCTUnwrap(parse(question(["Build target: Which one?", "1. Debug", "2. Release — slower, smaller",
                                                   "Choose a number."])))
        XCTAssertEqual(parsed.ref, ref)
        XCTAssertEqual(parsed.agent, agent)
        XCTAssertEqual(parsed.host, "test-mini")
        XCTAssertEqual(parsed.workingOn, "Tidying the build")
        XCTAssertEqual(parsed.expiresInMinutes, 30)
        let item = parsed.items[0]
        XCTAssertEqual(item.header, "Build target")
        XCTAssertEqual(item.body, "Which one?")
        XCTAssertEqual(item.options, [.init(number: 1, label: "Debug", description: nil),
                                      .init(number: 2, label: "Release", description: "slower, smaller")])
    }

    func testOneQuestionWithoutHeader() throws {
        let parsed = try XCTUnwrap(parse(question(["Which one should run first?", "1. Lint", "2. Tests", "Choose a number."])))
        XCTAssertNil(parsed.items[0].header)
        XCTAssertEqual(parsed.items[0].body, "Which one should run first?")
    }

    func testSeveralQuestions() throws {
        let parsed = try XCTUnwrap(parse(question([
            "(1/3) Target: Which build?", "1. Debug", "2. Release", "Choose a number.",
            "(2/3) Which checks?", "1. Lint", "2. Tests", "3. Docs", "Choose one or more (comma-separated numbers).",
            "(3/3) Notes for the release?", "free text",
        ], several: true)))
        XCTAssertEqual(parsed.items.map(\.body), ["Which build?", "Which checks?", "Notes for the release?"])
        XCTAssertEqual(parsed.items.map(\.selection), [.single, .multiple, .none])
    }

    func testNotACard() {
        let good = question(["Which one?", "1. A", "2. B", "Choose a number."])
        XCTAssertNotNil(parse(good))
        XCTAssertNil(parse(good, sender: "someone-else"), "wrong sender")
        XCTAssertNil(parse(good, mine: true), "mine")
        XCTAssertNil(RelayQuestionParser.parse(good, sender: "", mine: false, chatAgent: ""), "no chat agent")
        XCTAssertNil(parse(question(["Which one?", "1. A", "Choose a number."], agent: "other-agent")),
                     "line 1 names another agent")
        for bad in ["q0a1b2c3", "x0a1b2c3d", "q0a1b2c3z", "q0a1b2c3d4"] {
            XCTAssertNil(parse(question(["Which one?", "1. A", "Choose a number."], ref: bad)), bad)
        }
        XCTAssertNil(parse(question(["Which one?", "1. A", "Pick whatever you like."])), "unknown hint")
        XCTAssertNil(parse(question(["Which one?", "1. A", "2. B"])), "no hint")
        XCTAssertNil(parse(question(["Which one?", "1. A", "3. C", "Choose a number."])), "options out of order")
        XCTAssertNil(parse(question(["Which one?", "1. A", "Choose a number."], minutes: "soon")), "bad expiry")
        XCTAssertNil(parse(question(["Which one?", "free text", "Second?", "free text"])), "several without the several reply")
        XCTAssertNil(parse(question(["(1/2) Which one?", "free text", "Second?", "free text"], several: true)),
                     "several without every prefix")
        XCTAssertNil(parse(question(["Which one?", "Choose a number."])), "a choice hint with no options")
        XCTAssertNil(parse(question(["Which one?", "1. A", "free text"])), "free text hint with options")
        XCTAssertNil(parse("Fix the build"))
        XCTAssertNil(parse(""))
    }

    func testOversizeAndOddInput() {
        let many = (1...11).flatMap { ["(\($0)/11) Question \($0)?", "free text"] }
        XCTAssertNil(parse(question(many, several: true)), "more than 10 questions")
        let ten = (1...10).flatMap { ["(\($0)/10) Question \($0)?", "free text"] }
        XCTAssertNotNil(parse(question(ten, several: true)), "10 questions")
        let options = (1...51).map { "\($0). Option \($0)" }
        XCTAssertNil(parse(question(["Which one?"] + options + ["Choose a number."])), "more than 50 options")
        XCTAssertNotNil(parse(question(["Which one?"] + Array(options.prefix(50)) + ["Choose a number."])))
        XCTAssertNil(parse(question(["Which one?", String(repeating: "x", count: 50_000), "free text"])), "too long")
        XCTAssertNil(parse(String(repeating: "\n", count: 5_000)))
        XCTAssertNil(parse("[Question \(ref) from @\(agent) on ]\nfree text"))
    }

    // MARK: Answers

    private func line(_ text: String, _ responses: [RelayQuestionAnswer.Response]) throws -> Result<String, RelayQuestionAnswer.Problem> {
        RelayQuestionAnswer.line(for: try XCTUnwrap(parse(text)), responses: responses)
    }

    func testAnswerForEachShape() throws {
        let single = question(["Which one?", "1. A", "2. B", "Choose a number, or answer in free text."])
        XCTAssertEqual(try line(single, [.choices([2])]), .success("answer \(ref) 2"))
        XCTAssertEqual(try line(single, [.text("  Neither,\nask again  ")]), .success("answer \(ref) Neither, ask again"))
        XCTAssertEqual(try line(single, [.choices([1, 2])]), .failure(.invalidChoice(question: 1)))
        XCTAssertEqual(try line(single, [.choices([3])]), .failure(.invalidChoice(question: 1)))
        XCTAssertEqual(try line(single, [.text("2")]), .failure(.looksLikeChoice(question: 1)))
        XCTAssertEqual(try line(single, [.text("   ")]), .failure(.unanswered(question: 1)))
        XCTAssertEqual(try line(single, [.choices([])]), .failure(.unanswered(question: 1)))
        XCTAssertEqual(try line(single, []), .failure(.wrongCount))
        // One question: a ";" stays.
        XCTAssertEqual(try line(single, [.text("A; then B")]), .success("answer \(ref) A; then B"))

        let numbersOnly = question(["Which one?", "1. A", "2. B", "Choose a number."])
        XCTAssertEqual(try line(numbersOnly, [.text("A")]), .failure(.freeTextNotAllowed(question: 1)))

        let multi = question(["Which ones?", "1. A", "2. B", "3. C", "Choose one or more (comma-separated numbers)."])
        XCTAssertEqual(try line(multi, [.choices([3, 1, 3])]), .success("answer \(ref) 1,3"))

        let freeOnly = question(["Anything else?", "free text"])
        XCTAssertEqual(try line(freeOnly, [.text("42")]), .success("answer \(ref) 42"), "no options to confuse it with")

        let several = question(["(1/2) Which one?", "1. A", "2. B", "Choose a number.",
                                "(2/2) Notes?", "free text"], several: true)
        XCTAssertEqual(try line(several, [.choices([2]), .text("ship it; then tag")]),
                       .success("answer \(ref) 2; ship it, then tag"), "a ; in free text becomes ,")

        XCTAssertEqual(try line(freeOnly, [.text(String(repeating: "x", count: 4_090))]), .failure(.tooLong))
    }

    // MARK: Follow-ups and state

    func testEveryFollowUp() {
        let cases: [(String, RelayQuestionFollowUp)] = [
            ("Answer to question \(ref) delivered to @planner.", .delivered),
            ("Answer to question \(ref) is queued for @planner: it reaches the session at its next turn.", .queued),
            ("The answer to question \(ref) was not delivered: the session ended", .notDelivered(reason: "the session ended")),
            ("Question \(ref) expired; @planner continued without it.", .expired),
            ("Question \(ref) was already answered.", .alreadyAnswered),
            ("Question \(ref) was already answered in the session.", .answeredInSession),
            ("Question \(ref) is still open: the answer did not match.", .stillOpen(reason: "the answer did not match")),
            ("Question \(ref) is not open: @planner was not told it was sent, and asked it itself.", .notOpen),
        ]
        for (text, expected) in cases {
            let parsed = RelayQuestionFollowUp.parse(text)
            XCTAssertEqual(parsed?.ref, ref, text)
            XCTAssertEqual(parsed?.followUp, expected, text)
        }
        XCTAssertNil(RelayQuestionFollowUp.parse("Question \(ref) was moved somewhere new."))
        XCTAssertNil(RelayQuestionFollowUp.parse("Answer to question \(ref) went sideways."))
        XCTAssertNil(RelayQuestionFollowUp.parse("Question qzzzz expired"))
    }

    private func state(_ after: [RelayQuestionState.Message], pending: [RelayQuestionState.PendingAnswer] = [],
                       accepted: RelayQuestionState.AcceptedAnswer? = nil,
                       sentAt: Date? = Date(timeIntervalSince1970: 1_000), now: Date = Date(timeIntervalSince1970: 1_060)) throws -> RelayQuestionState {
        let text = question(["Which one?", "1. A", "2. B", "Choose a number."])
        let parsed = try XCTUnwrap(parse(text))
        let messages = [RelayQuestionState.Message(text: text, mine: false, sender: agent)] + after
        return RelayQuestionState.state(for: parsed, at: 0, in: messages, chatAgent: agent, pending: pending,
                                        accepted: accepted, sentAt: sentAt, now: now)
    }

    private func fromAgent(_ text: String, at seconds: TimeInterval? = nil, id: String = "") -> RelayQuestionState.Message {
        .init(text: text, mine: false, sender: agent, createdAt: seconds.map { Date(timeIntervalSince1970: $0) }, id: id)
    }
    private func mine(_ text: String, at seconds: TimeInterval? = nil, id: String = "") -> RelayQuestionState.Message {
        .init(text: text, mine: true, sender: "me", createdAt: seconds.map { Date(timeIntervalSince1970: $0) }, id: id)
    }
    private func acceptedAnswer(tappedAt seconds: TimeInterval, id: String? = nil,
                                earlier: Set<String>? = nil) -> RelayQuestionState.AcceptedAnswer {
        .init(sentAt: Date(timeIntervalSince1970: seconds), messageID: id, earlierRowIDs: earlier)
    }

    func testStateFromTheChat() throws {
        XCTAssertEqual(try state([]), .open)
        XCTAssertEqual(try state([mine("answer \(ref) 2")]), .sent)
        XCTAssertEqual(try state([mine("ANSWER \(ref.uppercased()) 1")]), .sent, "any case")
        XCTAssertEqual(try state([mine("answer \(ref) 2"), fromAgent("Answer to question \(ref) delivered to @planner.")]), .delivered)
        XCTAssertEqual(try state([fromAgent("Answer to question \(ref) is queued for @planner: next turn.")]), .queued)
        let notDelivered = try state([mine("answer \(ref) 2"), fromAgent("The answer to question \(ref) was not delivered: busy")])
        XCTAssertEqual(notDelivered, .notDelivered(reason: "busy"))
        XCTAssertTrue(notDelivered.answerable, "the card can be answered again")
        XCTAssertEqual(try state([fromAgent("Question \(ref) expired; @planner continued without it.")]), .expired)
        XCTAssertEqual(try state([fromAgent("Question \(ref) was already answered.")]), .alreadyAnswered)
        XCTAssertEqual(try state([fromAgent("Question \(ref) was already answered in the session.")]), .answeredInSession)
        XCTAssertEqual(try state([fromAgent("Question \(ref) is still open: try again.")]), .stillOpen(reason: "try again"))
        XCTAssertEqual(try state([fromAgent("Question \(ref) is not open: @planner asked it itself.")]), .notOpen)
    }

    func testUnknownOrForeignFollowUpsChangeNothing() throws {
        XCTAssertEqual(try state([fromAgent("Question \(ref) took a new path.")]), .open)
        XCTAssertEqual(try state([fromAgent("Question q99999999 was already answered.")]), .open, "another ref")
        XCTAssertEqual(try state([.init(text: "Question \(ref) was already answered.", mine: false, sender: "someone-else")]),
                       .open, "another sender")
        XCTAssertEqual(try state([mine("answer q99999999 2")]), .open, "an answer to another question")
    }

    func testPendingAndExpiry() throws {
        XCTAssertEqual(try state([], pending: [.init(text: "answer \(ref) 2", failed: false)]), .sending)
        XCTAssertEqual(try state([], pending: [.init(text: "answer \(ref) 2", failed: true)]), .sendFailed)
        XCTAssertEqual(try state([], pending: [.init(text: "hello", failed: false)]), .open)
        // 30 minutes after it was sent, plus the clock margin, the card stops sending.
        let expiry: TimeInterval = 1_000 + 30 * 60
        XCTAssertEqual(try state([], now: Date(timeIntervalSince1970: expiry + RelayQuestionState.clockSkew)), .expired)
        XCTAssertEqual(try state([], now: Date(timeIntervalSince1970: expiry + RelayQuestionState.clockSkew - 1)), .open,
                       "a device clock a little ahead does not block an answer the core would take")
        XCTAssertEqual(RelayQuestionState.clockSkew, 120)
        // Without a send time the minutes are shown as written and nothing expires by clock.
        XCTAssertEqual(try state([], sentAt: nil, now: .distantFuture), .open)
        // An answered card stays answered after the expiry time.
        XCTAssertEqual(try state([mine("answer \(ref) 2")], now: .distantFuture), .sent)
    }

    func testAnAcceptedAnswerStaysSentWithoutItsHistoryRow() throws {
        // Accepted, its bubble settled, history never showed the row. No id
        // and no history at the send: only the first tap's time identifies it.
        let accepted = acceptedAnswer(tappedAt: 1_010)
        let settled = try state([], accepted: accepted)
        XCTAssertEqual(settled, .sent)
        XCTAssertFalse(settled.answerable)
        // Still sent after the expiry time: it was answered.
        XCTAssertEqual(try state([], accepted: accepted, now: .distantFuture), .sent)
        // An old "not delivered" from before this answer does not reopen it.
        XCTAssertEqual(try state([mine("answer \(ref) 1", at: 500),
                                  fromAgent("The answer to question \(ref) was not delivered: busy", at: 505)],
                                 accepted: accepted), .sent)
        // Once history shows this answer, a reply to it decides, however soon it came.
        XCTAssertEqual(try state([mine("answer \(ref) 2", at: 1_009),
                                  fromAgent("The answer to question \(ref) was not delivered: busy", at: 1_012)],
                                 accepted: accepted), .notDelivered(reason: "busy"))
        XCTAssertEqual(try state([mine("answer \(ref) 2", at: 1_009),
                                  fromAgent("Answer to question \(ref) delivered to @planner.", at: 1_012)],
                                 accepted: accepted), .delivered)
    }

    func testAReplayedSendIsCaughtUpByTheRowOfItsFirstAttempt() throws {
        // The first tap at 1_010 was applied at once (row m7 at 1_011); its
        // POST answer was lost and a retry minutes later was accepted as a
        // replay. The agent's reply to that answer must reopen the card, not
        // stay "sent" because the replay returned long after the row.
        let history = [mine("answer \(ref) 2", at: 1_011, id: "m7"),
                       fromAgent("The answer to question \(ref) was not delivered: busy", at: 1_020, id: "m8")]
        let reopened = RelayQuestionState.notDelivered(reason: "busy")
        XCTAssertEqual(try state(history, accepted: acceptedAnswer(tappedAt: 1_010, id: "m7")), reopened, "by id")
        XCTAssertEqual(try state(history, accepted: acceptedAnswer(tappedAt: 1_010, earlier: [])), reopened,
                       "by a row that was not there at the send")
        XCTAssertEqual(try state(history, accepted: acceptedAnswer(tappedAt: 1_010)), reopened, "by the first tap's time")
        // Why the view keeps the first tap's time: the moment the replay
        // returned (1_400) is past the row's clock margin and never finds it.
        XCTAssertEqual(try state(history, accepted: acceptedAnswer(tappedAt: 1_400)), .sent)
        // The id Relaycast returned decides when it has one: another row is not it.
        XCTAssertEqual(try state(history, accepted: acceptedAnswer(tappedAt: 1_010, id: "m9")), .sent)
    }

    func testAReAnswerIsNotCaughtUpByTheAnswerBeforeIt() throws {
        // The first answer (m1) was not delivered, and the person answered
        // again at 1_060, within the clock margin of m1. Until history shows
        // the second answer, the card must not reopen for a duplicate send.
        let history = [mine("answer \(ref) 1", at: 1_000, id: "m1"),
                       fromAgent("The answer to question \(ref) was not delivered: busy", at: 1_005, id: "m2")]
        for accepted in [acceptedAnswer(tappedAt: 1_060, id: "m3"),
                         acceptedAnswer(tappedAt: 1_060, earlier: ["m1", "m2"]),
                         acceptedAnswer(tappedAt: 1_060)] {
            let waiting = try state(history, accepted: accepted)
            XCTAssertEqual(waiting, .sent)
            XCTAssertFalse(waiting.answerable)
            // Its row arrives, then the reply to it: the reply decides.
            let shown = history + [mine("answer \(ref) 2", at: 1_061, id: "m3")]
            XCTAssertEqual(try state(shown, accepted: accepted), .sent)
            XCTAssertEqual(try state(shown + [fromAgent("Answer to question \(ref) delivered to @planner.", at: 1_070)],
                                     accepted: accepted), .delivered)
            XCTAssertEqual(try state(shown + [fromAgent("The answer to question \(ref) was not delivered: away", at: 1_070)],
                                     accepted: accepted), .notDelivered(reason: "away"))
        }
        XCTAssertTrue(RelayQuestionFollowUp.notDelivered(reason: "").reopens)
        XCTAssertTrue(RelayQuestionFollowUp.stillOpen(reason: "").reopens)
        XCTAssertFalse(RelayQuestionFollowUp.delivered.reopens)
    }

    func testAnAcceptedPendingAnswerReadsSent() throws {
        XCTAssertEqual(try state([], pending: [.init(text: "answer \(ref) 2", failed: false, accepted: true)]), .sent)
        XCTAssertEqual(try state([], pending: [.init(text: "answer \(ref) 2", failed: false, accepted: false)]), .sending)
    }

    func testAQuestionMayStartWithTheMetadataWords() throws {
        let text = (["[Question \(ref) from @\(agent) on test-mini]", "Working on: Tidying the build", "session s-fixture",
                     "session timeout: raise it to 30s?", "1. Yes", "2. No", "Choose a number.",
                     "Reply: answer \(ref) <n|text>. Expires in 30 min; the agent continues without it."]).joined(separator: "\n")
        let parsed = try XCTUnwrap(parse(text))
        XCTAssertEqual(parsed.items[0].text, "session timeout: raise it to 30s?")
        let working = (["[Question \(ref) from @\(agent) on test-mini]", "Working on: the build",
                        "Working on: which part first?", "free text",
                        "Reply: answer \(ref) <n|text>. Expires in 30 min; the agent continues without it."]).joined(separator: "\n")
        let second = try XCTUnwrap(parse(working))
        XCTAssertEqual(second.workingOn, "the build")
        XCTAssertEqual(second.items[0].text, "Working on: which part first?")
        // Without the metadata lines the first line is the question.
        let bare = (["[Question \(ref) from @\(agent) on test-mini]", "session length: keep it?", "free text",
                     "Reply: answer \(ref) <n|text>. Expires in 30 min; the agent continues without it."]).joined(separator: "\n")
        XCTAssertNil(try XCTUnwrap(parse(bare)).workingOn)
    }

    func testAnsweredRef() {
        XCTAssertEqual(RelayQuestionState.answeredRef("answer Q0A1B2C3D 2"), "q0a1b2c3d")
        XCTAssertNil(RelayQuestionState.answeredRef("answer nope 2"))
        XCTAssertNil(RelayQuestionState.answeredRef("hello there friend"))
        XCTAssertNil(RelayQuestionState.answeredRef("answer q0a1b2c3d"))
    }

    func testStatusText() {
        XCTAssertEqual(QuestionCardView.status(.open, expiresAt: nil, minutes: 30, now: Date()), "Expires in 30 min.")
        let now = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(QuestionCardView.status(.open, expiresAt: now.addingTimeInterval(61), minutes: 30, now: now),
                       "Expires in 2 min.")
        XCTAssertEqual(QuestionCardView.status(.sent, expiresAt: nil, minutes: 30, now: now), "Answer sent.")
    }
}
