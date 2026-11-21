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
                       sentAt: Date? = Date(timeIntervalSince1970: 1_000), now: Date = Date(timeIntervalSince1970: 1_060)) throws -> RelayQuestionState {
        let text = question(["Which one?", "1. A", "2. B", "Choose a number."])
        let parsed = try XCTUnwrap(parse(text))
        let messages = [RelayQuestionState.Message(text: text, mine: false, sender: agent)] + after
        return RelayQuestionState.state(for: parsed, at: 0, in: messages, chatAgent: agent, pending: pending,
                                        sentAt: sentAt, now: now)
    }

    private func fromAgent(_ text: String) -> RelayQuestionState.Message { .init(text: text, mine: false, sender: agent) }
    private func mine(_ text: String) -> RelayQuestionState.Message { .init(text: text, mine: true, sender: "me") }

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
        // 30 minutes after it was sent the card stops sending.
        XCTAssertEqual(try state([], now: Date(timeIntervalSince1970: 1_000 + 30 * 60)), .expired)
        XCTAssertEqual(try state([], now: Date(timeIntervalSince1970: 1_000 + 30 * 60 - 1)), .open)
        // Without a send time the minutes are shown as written and nothing expires by clock.
        XCTAssertEqual(try state([], sentAt: nil, now: .distantFuture), .open)
        // An answered card stays answered after the expiry time.
        XCTAssertEqual(try state([mine("answer \(ref) 2")], now: .distantFuture), .sent)
    }

    func testStatusText() {
        XCTAssertEqual(QuestionCardView.status(.open, expiresAt: nil, minutes: 30, now: Date()), "Expires in 30 min.")
        let now = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(QuestionCardView.status(.open, expiresAt: now.addingTimeInterval(61), minutes: 30, now: now),
                       "Expires in 2 min.")
        XCTAssertEqual(QuestionCardView.status(.sent, expiresAt: nil, minutes: 30, now: now), "Answer sent.")
    }
}
