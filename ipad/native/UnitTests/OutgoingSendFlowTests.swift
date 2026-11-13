import XCTest
@testable import AgentRelayiPad

/// A history reload that does not finish until the test releases it.
@MainActor private final class SlowHistoryLoad {
    private(set) var finished = false
    private var gate: CheckedContinuation<Void, Never>?
    var waiting: Bool { gate != nil }

    func load() async {
        await withCheckedContinuation { continuation in gate = continuation }
        finished = true
    }

    func release() {
        gate?.resume()
        gate = nil
    }
}

private let start = Date(timeIntervalSince1970: 1_800_000_000)

private func row(_ id: String, _ text: String, at offset: TimeInterval = 5) -> OutgoingHistoryRow {
    OutgoingHistoryRow(id: id, text: text, attachmentIDs: [], mine: true, createdAt: start.addingTimeInterval(offset))
}

@MainActor final class OutgoingSendFlowTests: XCTestCase {
    private func spin(until done: () -> Bool) async {
        var spins = 0
        while !done(), spins < 1_000 {
            await Task.yield()
            spins += 1
        }
    }

    // MARK: send() answers before the background reload

    func testSendAnswersBeforeTheSlowHistoryReloadFinishes() async {
        let history = SlowHistoryLoad()
        var failures = 0
        let outcome = await RelaySendFlow.run(post: { ["message": ["id": "m2"]] as [String: Any] },
                                              onFailure: { _ in failures += 1 },
                                              afterAcceptance: { await history.load() })

        XCTAssertEqual(outcome, .accepted(messageID: "m2"))
        XCTAssertFalse(history.finished, "the send answered before the history reload finished")
        await spin { history.waiting }
        XCTAssertTrue(history.waiting, "the reload runs after the answer")
        XCTAssertFalse(history.finished)

        history.release()
        await spin { history.finished }
        XCTAssertTrue(history.finished)
        XCTAssertEqual(failures, 0)
    }

    func testFailedOrCancelledPostSkipsTheReloadAfterAcceptance() async {
        var failures: [String] = []
        var reloads = 0
        let failed = await RelaySendFlow.run(post: { throw RelayAPIError(message: "lost") },
                                             onFailure: { failures.append($0.localizedDescription) },
                                             afterAcceptance: { reloads += 1 })
        let cancelled = await RelaySendFlow.run(post: { throw CancellationError() },
                                                onFailure: { failures.append($0.localizedDescription) },
                                                afterAcceptance: { reloads += 1 })
        await Task.yield()

        XCTAssertEqual(failed, .failed)
        XCTAssertEqual(cancelled, .cancelled)
        XCTAssertEqual(failures, ["lost"])
        XCTAssertEqual(reloads, 0)
    }

    // MARK: post()'s timers

    func testUnansweredAttemptExpiresAfterTheConfirmationTimeout() async {
        var outgoing = OutgoingMessages()
        let stuck = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        let accepted = outgoing.begin(agentID: "reviewer", text: "Next", attachments: [], history: [], now: start)
        outgoing.finish(accepted.id, attempt: 1, outcome: .accepted(messageID: "m2"), now: start)
        var slept: [Duration] = []
        let timers = OutgoingTimers(sleep: { slept.append($0) }, now: { start })

        await timers.expireWhenUnanswered(stuck.id, attempt: 1) { change in change(&outgoing) }
        await timers.expireWhenUnanswered(accepted.id, attempt: 1) { change in change(&outgoing) }

        XCTAssertEqual(slept, [.seconds(60), .seconds(60)])
        XCTAssertEqual(outgoing.all.map(\.state), [.failed, .sent])
    }

    func testAcceptedBubbleIsSettledAfterTheRetention() async {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .accepted(messageID: "m2"), now: start)
        var slept: [Duration] = []
        let timers = OutgoingTimers(sleep: { slept.append($0) },
                                    now: { start.addingTimeInterval(OutgoingMessages.sentRetention) })

        await timers.settleWhenAccepted(sent.id) { change in change(&outgoing) }

        XCTAssertEqual(slept, [.seconds(60)])
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    // MARK: A tap reloads history before deciding

    func testRetryTapReloadsHistoryBeforeDecidingAndSkipsADeliveredMessage() async {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        var steps: [String] = []

        let decision = await OutgoingRetryTap.run(loadHistory: {
            steps.append("load")
            return [row("m2", "Ship it")]
        }, decide: { history in
            steps.append("decide")
            return outgoing.retry(sent.id, history: history)
        }, post: { _ in
            steps.append("post")
        })

        XCTAssertEqual(decision, .delivered)
        XCTAssertEqual(steps, ["load", "decide"], "history is read first and nothing is posted")
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testRetryTapPostsOnceWhenHistoryLacksTheMessage() async {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        var steps: [String] = []
        var posted: [OutgoingMessage] = []

        for _ in 0..<2 {
            await OutgoingRetryTap.run(loadHistory: {
                steps.append("load")
                return []
            }, decide: { history in
                outgoing.retry(sent.id, history: history)
            }, post: { message in
                steps.append("post")
                posted.append(message)
            })
        }

        XCTAssertEqual(steps, ["load", "post", "load"], "a second tap posts nothing")
        XCTAssertEqual(posted.map(\.id), [sent.id])
        XCTAssertEqual(posted.map(\.attempt), [2])
    }

    func testRetryTapDoesNothingOnceTheConversationIsClosed() async {
        var decided = false
        let decision = await OutgoingRetryTap.run(loadHistory: { nil },
                                                  decide: { _ in decided = true; return .ignored },
                                                  post: { _ in XCTFail("nothing is posted") })
        XCTAssertEqual(decision, .ignored)
        XCTAssertFalse(decided)
    }
}
