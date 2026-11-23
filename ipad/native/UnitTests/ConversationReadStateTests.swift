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
        // A failed read never calls apply; its ticket is simply dropped.
        _ = state.inboxRequested()
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
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .notFound)
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
        state.finishRead(conversationID: "dm_a", messageID: "m1", outcome: .notFound)
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

    func testOldCursorIsForgottenOnlyAfterEveryPostReachedTheServer() {
        XCTAssertTrue(ConversationReadState.carryOverComplete([]))
        XCTAssertTrue(ConversationReadState.carryOverComplete([.success, .notFound, .success]))
        XCTAssertFalse(ConversationReadState.carryOverComplete([.success, .failed]))
        XCTAssertFalse(ConversationReadState.carryOverComplete([.failed]))
    }

    // MARK: Notifications

    func testDeliveredNotificationsOfReadDirectConversationsAreRemoved() {
        let delivered: [(identifier: String, userInfo: [AnyHashable: Any])] = [
            ("read", ["conversation_id": "dm_a"]),
            ("unread", ["conversation_id": "dm_b"]),
            ("unknown", ["conversation_id": "dm_z"]),
            ("channel", ["conversation_id": "dm_a", "channel_name": "launch"]),
            ("channel-id", ["conversation_id": "dm_a", "channel_id": "ch_1"]),
            ("thread", ["conversation_id": "dm_a", "thread_id": "m_root"]),
            ("reply", ["conversation_id": "dm_a", "parent_id": "m_root"]),
            ("other", ["kind": "something"]),
        ]
        let removed = ConversationReadState.deliveredNotificationsToRemove(
            delivered, directConversationIDs: ["dm_a", "dm_b"], unread: ["dm_b"])
        XCTAssertEqual(removed, ["read"])
    }

    func testLocalNoticeHeldBackOnlyForAConversationTheServerSaysIsRead() {
        XCTAssertTrue(ConversationReadState().allowsLocalNotice(conversationID: "dm_a"), "No answer yet: as before")
        let state = answered(["dm_a": "m1"])
        XCTAssertTrue(state.allowsLocalNotice(conversationID: "dm_a"))
        XCTAssertFalse(state.allowsLocalNotice(conversationID: "dm_b"))
    }

    func testAFailedInboxKeepsTheSetButDoesNotHoldBackTheLocalNotice() {
        var state = answered(["dm_a": "m1"])
        XCTAssertFalse(state.allowsLocalNotice(conversationID: "dm_b"))
        _ = state.inboxRequested()
        state.inboxUnanswered()
        XCTAssertEqual(state.unreadConversationIDs, ["dm_a"])
        XCTAssertTrue(state.hasAnswer)
        XCTAssertTrue(state.allowsLocalNotice(conversationID: "dm_b"), "The inbox said nothing this time")
        state.apply(inbox: ["dm_a": "m1"], ticket: state.inboxRequested())
        XCTAssertFalse(state.allowsLocalNotice(conversationID: "dm_b"), "A new answer holds it back again")
    }
}
