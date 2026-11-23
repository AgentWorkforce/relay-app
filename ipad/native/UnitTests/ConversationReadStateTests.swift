import XCTest
@testable import AgentRelayiPad

final class ConversationReadStateTests: XCTestCase {
    private let me = "test-person"

    private func dm(_ id: String, from: String, count: Any? = 1, last: String? = "m_last") -> [String: Any] {
        var entry: [String: Any] = ["conversation_id": id, "from": from]
        if let count { entry["unread_count"] = count }
        entry["last_message"] = last.map { ["id": $0, "text": "Invented text", "created_at": "2026-11-01T10:00:00.000Z"] as [String: Any] }
            ?? NSNull()
        return entry
    }

    private func inbox(_ dms: [[String: Any]]) -> [String: Any] {
        ["unread_channels": [], "mentions": [], "unread_dms": dms, "recent_reactions": []]
    }

    private func answered(_ unread: [String: String]) -> ConversationReadState {
        var state = ConversationReadState()
        let ticket = state.inboxRequested()
        state.apply(inbox: unread, ticket: ticket)
        return state
    }

    // MARK: Inbox parsing

    func testInboxListsUnreadConversationsWithTheirLastMessage() {
        let unread = ConversationReadState.unread(inInbox: inbox([
            dm("dm_a", from: "peer-a", last: "m_a2"),
            dm("dm_b", from: "peer-b", count: nil, last: nil),
        ]), mine: me)
        XCTAssertEqual(unread, ["dm_a": "m_a2", "dm_b": ""])
    }

    func testOwnMessagesNeverCount() {
        let unread = ConversationReadState.unread(inInbox: inbox([dm("dm_a", from: me)]), mine: me)
        XCTAssertTrue(unread.isEmpty)
    }

    func testEmptyIDsAndZeroCountsDoNotCount() {
        let unread = ConversationReadState.unread(inInbox: inbox([
            dm("", from: "peer-a"),
            dm("dm_b", from: "peer-b", count: 0),
            dm("dm_c", from: "peer-c", count: NSNumber(value: 2)),
        ]), mine: me)
        XCTAssertEqual(Set(unread.keys), ["dm_c"])
    }

    func testMalformedInboxHasNothingUnread() {
        XCTAssertTrue(ConversationReadState.unread(inInbox: [:], mine: me).isEmpty)
        XCTAssertTrue(ConversationReadState.unread(inInbox: ["unread_dms": "nope"], mine: me).isEmpty)
    }

    // MARK: Unread set

    func testFirstLoadWithoutAnAnswerHasNothingUnread() {
        let state = ConversationReadState()
        XCTAssertFalse(state.hasAnswer)
        XCTAssertTrue(state.unreadConversationIDs.isEmpty)
    }

    func testAConversationAbsentFromTheInboxIsRead() {
        var state = answered(["dm_a": "m1", "dm_b": "m2"])
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a", "dm_b"])
        state.apply(inbox: ["dm_b": "m2"], ticket: state.inboxRequested())
        XCTAssertEqual(state.unreadConversationIDs, ["dm_b"])
        XCTAssertTrue(state.hasAnswer)
    }

    func testAFailedInboxKeepsThePreviousSet() {
        var state = answered(["dm_a": "m1"])
        _ = state.inboxRequested()
        state.inboxUnanswered()
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
    }

    // MARK: Choosing the message to mark read

    func testNewestIncomingMessageOfThePage() {
        let page: [(id: String, mine: Bool)] = [("m1", false), ("m2", false), ("m3", true)]
        XCTAssertEqual(ConversationReadState.messageToMarkRead(page: page, latestIncomingID: "m2"), "m2")
        XCTAssertEqual(ConversationReadState.messageToMarkRead(page: page, latestIncomingID: nil), "m2")
    }

    func testOwnNewestMessageNeverCounts() {
        XCTAssertNil(ConversationReadState.messageToMarkRead(page: [("m1", true), ("m2", true)], latestIncomingID: nil))
        XCTAssertNil(ConversationReadState.messageToMarkRead(page: [], latestIncomingID: nil))
    }

    func testRowsWithoutServerIDsAreSkipped() {
        XCTAssertEqual(ConversationReadState.messageToMarkRead(page: [("m1", false), ("", false)], latestIncomingID: nil), "m1")
    }

    func testNewestIncomingNotLoadedYetMarksNothing() {
        XCTAssertNil(ConversationReadState.messageToMarkRead(page: [("m1", false), ("m2", true)], latestIncomingID: "m3"))
    }

    func testMarksReadOnlyForAnOpenLoadedDirectConversationInTheForeground() {
        XCTAssertTrue(ConversationReadState.mayMarkRead(isDirectConversation: true, isOpen: true, historyLoaded: true, appActive: true))
        XCTAssertFalse(ConversationReadState.mayMarkRead(isDirectConversation: true, isOpen: true, historyLoaded: true, appActive: false))
        XCTAssertFalse(ConversationReadState.mayMarkRead(isDirectConversation: false, isOpen: true, historyLoaded: true, appActive: true))
        XCTAssertFalse(ConversationReadState.mayMarkRead(isDirectConversation: true, isOpen: false, historyLoaded: true, appActive: true))
        XCTAssertFalse(ConversationReadState.mayMarkRead(isDirectConversation: true, isOpen: true, historyLoaded: false, appActive: true))
    }

    // MARK: Posting reads

    func testBeginReadClearsTheConversationAtOnce() {
        var state = answered(["dm_a": "m1", "dm_b": "m2"])
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        XCTAssertEqual(state.unreadConversationIDs, ["dm_b"])
    }

    func testOnePostPerMessage() {
        var state = ConversationReadState()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        XCTAssertFalse(state.beginRead(conversationID: "dm_a", messageID: "m1"), "Already in flight")
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .success)
        XCTAssertFalse(state.beginRead(conversationID: "dm_a", messageID: "m1"), "Already posted")
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m2"), "A newer message is posted")
        XCTAssertFalse(state.beginRead(conversationID: "dm_a", messageID: ""))
    }

    func testAFailedPostIsAllowedAgain() {
        var state = ConversationReadState()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .failed)
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
    }

    func testAnUnknownMessageIsNotPostedAgain() {
        var state = ConversationReadState()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .refused)
        XCTAssertFalse(state.beginRead(conversationID: "dm_a", messageID: "m1"))
    }

    func testAnAnswerRequestedBeforeThePostFinishedDoesNotReMark() {
        var state = answered(["dm_a": "m1"])
        let before = state.inboxRequested()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        let during = state.inboxRequested()
        state.apply(inbox: ["dm_a": "m1"], ticket: during)
        XCTAssertFalse(state.unreadConversationIDs.contains("dm_a"), "Requested while the post was in flight")
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .success)
        state.apply(inbox: ["dm_a": "m1"], ticket: before)
        XCTAssertFalse(state.unreadConversationIDs.contains("dm_a"), "Requested before the post started")
        state.apply(inbox: ["dm_a": "m1"], ticket: during)
        XCTAssertFalse(state.unreadConversationIDs.contains("dm_a"), "Requested before the post finished")
    }

    func testAnAnswerProtectsNotificationsOfConversationsItListsOrThisDeviceIsReading() {
        var state = answered(["dm_a": "m1", "dm_b": "m4"])
        let before = state.inboxRequested()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))   // in flight
        XCTAssertTrue(state.beginRead(conversationID: "dm_c", messageID: "m7"))
        state.finishRead(conversationID: "dm_c", messageID: "m7", outcome: .success)   // finished after the ticket
        XCTAssertTrue(state.beginRead(conversationID: "dm_d", messageID: "m9"))
        state.finishRead(conversationID: "dm_d", messageID: "m9", outcome: .success)
        let protected = state.apply(inbox: ["dm_a": "m2", "dm_b": "m4"], ticket: before)
        XCTAssertEqual(protected, ["dm_a", "dm_b", "dm_c", "dm_d"],
                       "Listed as unread, or kept read only by this device's own read")
        XCTAssertEqual(state.unreadConversationIDs, ["dm_b"])

        // Once the reads are older than the request, only the answer counts.
        let after = state.inboxRequested()
        XCTAssertEqual(state.apply(inbox: ["dm_b": "m4"], ticket: after), ["dm_a", "dm_b"], "dm_a is still in flight")
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .success)
        XCTAssertEqual(state.apply(inbox: ["dm_b": "m4"], ticket: state.inboxRequested()), ["dm_b"])
    }

    func testAnAnswerRequestedAfterThePostFinishedIsTrusted() {
        var state = answered(["dm_a": "m1"])
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .success)
        let after = state.inboxRequested()
        // A newer message arrived after the read.
        state.apply(inbox: ["dm_a": "m2"], ticket: after)
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
        // The mark is gone: even an answer with an older ticket is now trusted.
        state.apply(inbox: ["dm_a": "m2"], ticket: 0)
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
    }

    func testAFailedPostLetsTheNextAnswerReMark() {
        var state = answered(["dm_a": "m1"])
        let during = state.inboxRequested()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .failed)
        state.apply(inbox: ["dm_a": "m1"], ticket: during)
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
    }

    func testAnUnknownMessageLetsTheNextAnswerReMark() {
        var state = answered(["dm_a": "m1"])
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .refused)
        state.apply(inbox: ["dm_a": "m1"], ticket: state.inboxRequested())
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
    }

    func testAnOlderPostFinishingDoesNotEndANewerOnesMark() {
        var state = answered(["dm_a": "m2"])
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m2"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .failed)
        state.apply(inbox: ["dm_a": "m2"], ticket: state.inboxRequested())
        XCTAssertFalse(state.unreadConversationIDs.contains("dm_a"), "m2 is still in flight")
    }

    // MARK: Carry-over

    func testOldCursorExists() {
        XCTAssertNil(ConversationReadState.oldSeenCursor(nil, hasBaseline: false))
        XCTAssertEqual(ConversationReadState.oldSeenCursor(nil, hasBaseline: true), [:])
        XCTAssertEqual(ConversationReadState.oldSeenCursor([:], hasBaseline: false), [:])
        XCTAssertEqual(ConversationReadState.oldSeenCursor(["dm_a": "m1", "dm_b": 3], hasBaseline: true), ["dm_a": "m1"])
    }

    func testCarryOverPlanCoversUnreadConversationsTheDeviceHadSeen() {
        let plan = ConversationReadState.carryOverPlan(unread: ["dm_c", "dm_a", "dm_b", "dm_d"],
                                                       seen: ["dm_a": "m_a", "dm_c": "m_c", "dm_d": "", "dm_x": "m_x"])
        XCTAssertEqual(plan.map(\.conversationID), ["dm_a", "dm_c"])
        XCTAssertEqual(plan.map(\.messageID), ["m_a", "m_c"])
        XCTAssertTrue(ConversationReadState.carryOverPlan(unread: ["dm_a"], seen: [:]).isEmpty)
    }

    func testRefusedMeansAnyClientErrorButATimeoutOrRateLimit() {
        for status in [400, 404, 410, 422, 499] {
            XCTAssertEqual(ConversationReadState.outcome(forFailureStatus: status), .refused, "\(status)")
        }
        for status in [nil, 408, 429, 500, 503, 302] as [Int?] {
            XCTAssertEqual(ConversationReadState.outcome(forFailureStatus: status), .failed, "\(String(describing: status))")
        }
    }

    func testACredentialRefusalIsRetried() {
        XCTAssertEqual(ConversationReadState.outcome(forFailureStatus: 401), .failed)
        XCTAssertEqual(ConversationReadState.outcome(forFailureStatus: 403), .failed)
        var state = ConversationReadState()
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"))
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: ConversationReadState.outcome(forFailureStatus: 403))
        XCTAssertTrue(state.beginRead(conversationID: "dm_a", messageID: "m1"), "Posted again once the credential works")
    }

    private func step(_ unread: [String: String], cursor: [String: String]?, running: Bool = false,
                      after: Bool = false) -> String {
        switch ConversationReadState.inboxStep(unread: unread, oldCursor: cursor, carryOverRunning: running, afterCarryOver: after) {
        case .apply(let forget): return forget ? "apply+forget" : "apply"
        case .keepLast: return "keepLast"
        case .carryOver(let plan): return "carryOver " + plan.map { "\($0.conversationID)=\($0.messageID)" }.joined(separator: ",")
        }
    }

    func testInboxStep() {
        let unread = ["dm_a": "m_a2", "dm_b": "m_b1"]
        XCTAssertEqual(step(unread, cursor: nil), "apply", "A fresh install applies the answer")
        XCTAssertEqual(step(unread, cursor: [:]), "apply+forget", "Nothing to carry over: forget the cursor")
        XCTAssertEqual(step(unread, cursor: ["dm_x": "m_x"]), "apply+forget")
        XCTAssertEqual(step(unread, cursor: ["dm_b": "m_b1", "dm_a": "m_a1"]), "carryOver dm_a=m_a1,dm_b=m_b1")
        XCTAssertEqual(step(unread, cursor: ["dm_a": "m_a1"], running: true), "keepLast", "Another carry-over is running")
        XCTAssertEqual(step(unread, cursor: nil, running: true), "keepLast")
        XCTAssertEqual(step(unread, cursor: ["dm_a": "m_a1"], after: true), "apply", "The re-read applies as is")
    }

    func testCarryOverDropsEntriesTheServerTookOrRefused() {
        let seen = ["dm_a": "m_a", "dm_b": "m_b", "dm_c": "m_c", "dm_x": "m_x"]
        let plan = [(conversationID: "dm_a", messageID: "m_a"), (conversationID: "dm_b", messageID: "m_b"),
                    (conversationID: "dm_c", messageID: "m_c")]
        let next = ConversationReadState.cursorAfterCarryOver(seen: seen, plan: plan, outcomes: [.success, .refused, .failed],
                                                              failedReloads: 0)
        XCTAssertEqual(next.seen, ["dm_c": "m_c", "dm_x": "m_x"])
        XCTAssertEqual(next.failedReloads, 1)
    }

    func testCarryOverWithoutFailuresDeletesTheCursor() {
        let plan = [(conversationID: "dm_a", messageID: "m_a"), (conversationID: "dm_b", messageID: "m_b")]
        let next = ConversationReadState.cursorAfterCarryOver(seen: ["dm_a": "m_a", "dm_b": "m_b"], plan: plan,
                                                              outcomes: [.success, .refused], failedReloads: 3)
        XCTAssertNil(next.seen)
        XCTAssertEqual(next.failedReloads, 0)
    }

    func testCarryOverGivesUpAfterFiveFailingReloads() {
        let plan = [(conversationID: "dm_a", messageID: "m_a")]
        var failed = 0
        for reload in 1...4 {
            // A rate limit is no verdict on the message: it counts as a failure.
            let next = ConversationReadState.cursorAfterCarryOver(
                seen: ["dm_a": "m_a"], plan: plan,
                outcomes: [ConversationReadState.outcome(forFailureStatus: 429)], failedReloads: failed)
            XCTAssertEqual(next.seen, ["dm_a": "m_a"], "Reload \(reload) keeps the cursor")
            XCTAssertEqual(next.failedReloads, reload)
            failed = next.failedReloads
        }
        let last = ConversationReadState.cursorAfterCarryOver(seen: ["dm_a": "m_a"], plan: plan, outcomes: [.failed],
                                                              failedReloads: failed)
        XCTAssertNil(last.seen, "The fifth failing reload gives up")
        XCTAssertEqual(last.failedReloads, 0)
    }

    func testAnUnfinishedCarryOverCountsAsFailed() {
        let plan = [(conversationID: "dm_a", messageID: "m_a"), (conversationID: "dm_b", messageID: "m_b")]
        let next = ConversationReadState.cursorAfterCarryOver(seen: ["dm_a": "m_a", "dm_b": "m_b"], plan: plan,
                                                              outcomes: [.success], failedReloads: 0)
        XCTAssertEqual(next.seen, ["dm_b": "m_b"])
        XCTAssertEqual(next.failedReloads, 1)
    }

    // MARK: Notifications

    func testDeliveredNotificationsOfReadDirectConversationsAreRemoved() {
        let cutoff = Date(timeIntervalSince1970: 1_000)
        let earlier = cutoff.addingTimeInterval(-60)
        let delivered: [ConversationReadState.Delivered] = [
            ("read", ["conversation_id": "dm_a"], earlier),
            ("unread", ["conversation_id": "dm_b"], earlier),
            ("unknown", ["conversation_id": "dm_z"], earlier),
            ("channel", ["conversation_id": "dm_a", "channel_name": "launch"], earlier),
            ("channel-id", ["conversation_id": "dm_a", "channel_id": "ch_1"], earlier),
            ("thread", ["conversation_id": "dm_a", "thread_id": "m_root"], earlier),
            ("reply", ["conversation_id": "dm_a", "parent_id": "m_root"], earlier),
            ("other", ["kind": "something"], earlier),
            ("at-cutoff", ["conversation_id": "dm_a"], cutoff),
            ("later", ["conversation_id": "dm_a"], cutoff.addingTimeInterval(1)),
        ]
        let removed = ConversationReadState.deliveredNotificationsToRemove(
            delivered, before: cutoff, directConversationIDs: ["dm_a", "dm_b"], unread: ["dm_b"])
        XCTAssertEqual(removed, ["read"], "Only what arrived before the deciding read started")
    }

    func testNoticeFollowsTheLatestInboxRead() {
        XCTAssertEqual(ConversationReadState().notice(conversationID: "dm_a"), .unknown, "No answer yet")
        let state = answered(["dm_a": "m1"])
        XCTAssertEqual(state.notice(conversationID: "dm_a"), .unread)
        XCTAssertEqual(state.notice(conversationID: "dm_b"), .read)
    }

    func testAFailedInboxKeepsTheSetButSaysNothingForTheNotice() {
        var state = answered(["dm_a": "m1"])
        XCTAssertEqual(state.notice(conversationID: "dm_b"), .read)
        _ = state.inboxRequested()
        state.inboxUnanswered()
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
        XCTAssertTrue(state.hasAnswer)
        XCTAssertEqual(state.notice(conversationID: "dm_b"), .unknown, "The inbox said nothing this time")
        XCTAssertEqual(state.notice(conversationID: "dm_a"), .unknown)
        state.apply(inbox: ["dm_a": "m1"], ticket: state.inboxRequested())
        XCTAssertEqual(state.notice(conversationID: "dm_b"), .read, "A new answer holds it back again")
    }
}
