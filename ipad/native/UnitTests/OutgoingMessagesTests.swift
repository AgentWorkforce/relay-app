import XCTest
@testable import AgentRelayiPad

final class OutgoingMessagesTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_800_000_000)

    private func row(_ id: String, _ text: String, mine: Bool = true, at offset: TimeInterval = 5,
                     attachments: [String] = []) -> OutgoingHistoryRow {
        OutgoingHistoryRow(id: id, text: text, attachmentIDs: attachments, mine: mine,
                           createdAt: start.addingTimeInterval(offset))
    }

    // MARK: A failed POST is never sent again on its own

    func testRequestLayerNeverRepeatsAPostOnItsOwn() {
        XCTAssertEqual(RelayRequestRetry.automaticAttempts(method: "POST"), 1)
        XCTAssertEqual(RelayRequestRetry.automaticAttempts(method: "GET"), 3)
    }

    func testPostThatFailedAfterAcceptanceIsNotPostedAgain() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [],
                                  history: [row("m1", "Earlier")], now: start)
        // Relaycast stored the message, then the answer was lost.
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        XCTAssertEqual(outgoing.all.map(\.state), [.failed])

        // The tap reads history first: the message is there, so nothing is POSTed.
        let history = [row("m1", "Earlier", at: -60), row("m2", "Ship it")]
        XCTAssertEqual(outgoing.retry(sent.id, history: history), .delivered)
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testFailedBubbleLeavesWhenHistoryShowsTheAppliedMessage() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)

        outgoing.reconcile(agentID: "reviewer", with: [row("m2", "Ship it")])

        XCTAssertTrue(outgoing.all.isEmpty, "a failed send that history shows must not stay beside it")
    }

    func testSendBeforeHistoryLoadedIsNeverMatchedByContent() {
        var outgoing = OutgoingMessages()
        // The conversation was just opened: its history has not loaded yet.
        let sent = outgoing.begin(agentID: "reviewer", text: "ok", attachments: [], history: nil, now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)

        // History then loads with an earlier "ok", recent enough to pass the clock-skew check.
        let earlier = row("m1", "ok", at: -30)
        outgoing.reconcile(agentID: "reviewer", with: [earlier])
        XCTAssertEqual(outgoing.all.map(\.state), [.failed], "the failed bubble stays")

        guard case .post(let again) = outgoing.retry(sent.id, history: [earlier]) else {
            return XCTFail("a send never applied must be posted again, not taken for an older row")
        }
        XCTAssertEqual(again.id, sent.id)

        // An accepted send without loaded history still leaves by its returned id.
        outgoing.finish(sent.id, attempt: 2, outcome: .accepted(messageID: "m2"))
        outgoing.reconcile(agentID: "reviewer", with: [earlier, row("m2", "ok")])
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testUndatedRowIsNeverClaimedByContent() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "ok", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        let undated = OutgoingHistoryRow(id: "m1", text: "ok", attachmentIDs: [], mine: true, createdAt: nil)

        outgoing.reconcile(agentID: "reviewer", with: [undated])
        XCTAssertEqual(outgoing.all.count, 1, "a row without a date may predate the send")

        guard case .post = outgoing.retry(sent.id, history: [undated]) else {
            return XCTFail("an undated row does not prove the send was applied")
        }
    }

    // MARK: Pending reconciles to the delivered row

    func testAcceptedSendReconcilesToItsHistoryRowByID() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .accepted(messageID: "m2"))
        XCTAssertEqual(outgoing.all.first?.state, .sent, "an accepted send never stays \"Sending…\"")

        // A row with the same text but another id is a different message.
        outgoing.reconcile(agentID: "reviewer", with: [row("m9", "Ship it")])
        XCTAssertEqual(outgoing.all.map(\.id), [sent.id])

        outgoing.reconcile(agentID: "reviewer", with: [row("m9", "Ship it"), row("m2", "Ship it")])
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testSendingBubbleLeavesWhenHistoryShowsItBeforeTheAnswer() {
        var outgoing = OutgoingMessages()
        let file = RelayAttachment(fileID: "f1", filename: "a.png", contentType: "image/png", sizeBytes: 3)
        let sent = outgoing.begin(agentID: "reviewer", text: "Look", attachments: [file], history: [], now: start)

        outgoing.reconcile(agentID: "reviewer", with: [row("m2", "Look")])
        XCTAssertEqual(outgoing.all.count, 1, "the attachments must match too")

        outgoing.reconcile(agentID: "reviewer", with: [row("m2", "Look", attachments: ["f1"])])
        XCTAssertTrue(outgoing.all.isEmpty)
        // The late answer to the POST adds nothing back.
        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testEarlierIdenticalMessageIsNotTakenForTheSend() {
        var outgoing = OutgoingMessages()
        let earlier = row("m1", "ok", at: -30)
        outgoing.begin(agentID: "reviewer", text: "ok", attachments: [], history: [earlier], now: start)

        outgoing.reconcile(agentID: "reviewer", with: [earlier, row("m3", "ok", mine: false)])
        XCTAssertEqual(outgoing.all.count, 1, "an older row or the peer's row is not this send")

        outgoing.reconcile(agentID: "reviewer", with: [row("m4", "ok", at: -3_600)])
        XCTAssertEqual(outgoing.all.count, 1, "a row dated long before the tap is not this send")
    }

    func testIdenticalSendsClaimOneRowEach() {
        var outgoing = OutgoingMessages()
        outgoing.begin(agentID: "reviewer", text: "ok", attachments: [], history: [], now: start)
        outgoing.begin(agentID: "reviewer", text: "ok", attachments: [], history: [], now: start)
        outgoing.begin(agentID: "builder", text: "ok", attachments: [], history: [], now: start)

        outgoing.reconcile(agentID: "reviewer", with: [row("m2", "ok")])

        XCTAssertEqual(outgoing.all.map(\.agentID), ["reviewer", "builder"])
    }

    func testSentBubbleLeavesAfterTheRetentionEvenWithoutItsRow() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .accepted(messageID: "m2"), now: start)

        // Its row is outside the loaded window.
        outgoing.reconcile(agentID: "reviewer", with: [row("m1", "Earlier", at: -30)],
                           now: start.addingTimeInterval(OutgoingMessages.sentRetention - 1))
        XCTAssertEqual(outgoing.all.map(\.state), [.sent])
        outgoing.reconcile(agentID: "reviewer", with: [row("m1", "Earlier", at: -30)],
                           now: start.addingTimeInterval(OutgoingMessages.sentRetention))
        XCTAssertTrue(outgoing.all.isEmpty, "\"Sent\" does not stay for good")
    }

    func testSentBubbleLeavesWhenHistoryIsClearlyNewerThanTheSend() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.finish(sent.id, attempt: 1, outcome: .accepted(messageID: "m2"), now: start)

        outgoing.reconcile(agentID: "reviewer", with: [row("m3", "Reply", mine: false, at: OutgoingMessages.clockSkew)],
                           now: start.addingTimeInterval(1))
        XCTAssertEqual(outgoing.all.count, 1, "a row within the clock skew may still predate it")

        outgoing.reconcile(agentID: "reviewer", with: [row("m4", "Reply", mine: false, at: OutgoingMessages.clockSkew + 1)],
                           now: start.addingTimeInterval(1))
        XCTAssertTrue(outgoing.all.isEmpty)
    }

    func testSettleRemovesOnlyAcceptedBubblesPastTheRetention() {
        var outgoing = OutgoingMessages()
        let accepted = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        let failed = outgoing.begin(agentID: "reviewer", text: "Next", attachments: [], history: [], now: start)
        outgoing.finish(accepted.id, attempt: 1, outcome: .accepted(messageID: nil), now: start)
        outgoing.finish(failed.id, attempt: 1, outcome: .failed, now: start)

        outgoing.settle(accepted.id, now: start.addingTimeInterval(30))
        XCTAssertEqual(outgoing.all.count, 2)

        let later = start.addingTimeInterval(OutgoingMessages.sentRetention)
        outgoing.settle(accepted.id, now: later)
        outgoing.settle(failed.id, now: later)
        XCTAssertEqual(outgoing.all.map(\.id), [failed.id], "a failed bubble waits for a tap")
    }

    // MARK: Failure clears pending and offers one retry per tap

    func testFailureOffersOneRetryPerTapWithTheSameKey() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        XCTAssertEqual(outgoing.retry(sent.id, history: []), .ignored, "a send in flight cannot be retried")

        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        XCTAssertEqual(outgoing.all.first?.state, .failed, "a failure no longer says \"Sending…\"")

        guard case .post(let again) = outgoing.retry(sent.id, history: []) else {
            return XCTFail("a tap on a failed bubble posts it again")
        }
        XCTAssertEqual(again.id, sent.id, "the retry reuses the send's Idempotency-Key")
        XCTAssertEqual(again.attempt, 2)
        XCTAssertEqual(again.state, .sending)
        XCTAssertEqual(outgoing.retry(sent.id, history: []), .ignored, "a second tap posts nothing")
    }

    func testLateFailureOfAnOlderAttemptCannotFailTheRetry() {
        var outgoing = OutgoingMessages()
        let sent = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        outgoing.expire(sent.id, attempt: 1)
        _ = outgoing.retry(sent.id, history: [])

        outgoing.finish(sent.id, attempt: 1, outcome: .failed)
        XCTAssertEqual(outgoing.all.first?.state, .sending)

        // An acceptance is the truth whichever attempt it answers.
        outgoing.finish(sent.id, attempt: 1, outcome: .accepted(messageID: "m2"))
        XCTAssertEqual(outgoing.all.first?.state, .sent)
        outgoing.finish(sent.id, attempt: 2, outcome: .failed)
        XCTAssertEqual(outgoing.all.first?.state, .sent)
        XCTAssertEqual(outgoing.all.first?.messageID, "m2")
    }

    func testUnansweredSendIsShownFailedAfterTheTimeout() {
        var outgoing = OutgoingMessages()
        let stuck = outgoing.begin(agentID: "reviewer", text: "Ship it", attachments: [], history: [], now: start)
        let accepted = outgoing.begin(agentID: "reviewer", text: "Next", attachments: [], history: [], now: start)
        outgoing.finish(accepted.id, attempt: 1, outcome: .accepted(messageID: nil))

        outgoing.expire(stuck.id, attempt: 1)
        outgoing.expire(accepted.id, attempt: 1)

        XCTAssertEqual(outgoing.all.map(\.state), [.failed, .sent])
        XCTAssertFalse(outgoing.all.contains { $0.state == .sending })
    }

    func testConfirmationTimeoutIsSixtySeconds() {
        XCTAssertEqual(OutgoingMessages.confirmationTimeout, .seconds(60))
        XCTAssertEqual(OutgoingMessages.sentRetention, 60)
    }

    // MARK: Idempotency-Key on the wire

    func testSendRequestCarriesTheIdempotencyKey() throws {
        let url = try XCTUnwrap(URL(string: "https://relaycast.example/v1/dm"))
        let spec = RelayRequestSpec(url: url, token: "agent-token", method: "POST",
                                    body: ["to": "reviewer", "text": "Ship it"], idempotencyKey: "send-1")
        let request = try spec.urlRequest()

        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Idempotency-Key"), "send-1")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer agent-token")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/json")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Accept"), "application/json")
        XCTAssertEqual(request.timeoutInterval, 25)
        let body = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(request.httpBody)) as? [String: String])
        XCTAssertEqual(body, ["to": "reviewer", "text": "Ship it"])
    }

    func testCredentialRefreshResendReusesTheIdempotencyKey() throws {
        let url = try XCTUnwrap(URL(string: "https://relaycast.example/v1/dm"))
        let spec = RelayRequestSpec(url: url, token: "refused", method: "POST",
                                    body: ["to": "reviewer", "text": "Ship it"], idempotencyKey: "send-1")
        let first = try spec.urlRequest()
        let resend = try spec.resent(with: "refreshed").urlRequest()

        XCTAssertEqual(resend.value(forHTTPHeaderField: "Authorization"), "Bearer refreshed")
        XCTAssertEqual(resend.value(forHTTPHeaderField: "Idempotency-Key"), "send-1")
        XCTAssertEqual(resend.httpMethod, first.httpMethod)
        XCTAssertEqual(resend.url, first.url)
        XCTAssertEqual(resend.httpBody, first.httpBody)
    }

    func testRequestWithoutAKeySendsNoIdempotencyHeader() throws {
        let url = try XCTUnwrap(URL(string: "https://relaycast.example/v1/dm/conversations"))
        let request = try RelayRequestSpec(url: url, token: "agent-token").urlRequest()

        XCTAssertEqual(request.httpMethod, "GET")
        XCTAssertNil(request.value(forHTTPHeaderField: "Idempotency-Key"))
        XCTAssertNil(request.value(forHTTPHeaderField: "Content-Type"))
        XCTAssertNil(request.httpBody)
    }

    // MARK: Relaycast's answer

    func testMessageIDIsReadFromDMAndChannelAnswers() {
        let dm: [String: Any] = ["conversation_id": "dm_1", "message": ["id": "m2"] as [String: Any],
                                 "created_at": "2026-11-10T00:00:00Z"]
        let channel: [String: Any] = ["id": "m3", "agent_name": "me"]
        let acknowledgement: [String: Any] = ["ok": true]
        XCTAssertEqual(OutgoingMessages.messageID(fromSendResponse: dm), "m2")
        XCTAssertEqual(OutgoingMessages.messageID(fromSendResponse: channel), "m3")
        XCTAssertNil(OutgoingMessages.messageID(fromSendResponse: acknowledgement))
        XCTAssertNil(OutgoingMessages.messageID(fromSendResponse: [Any]()))
    }
}
