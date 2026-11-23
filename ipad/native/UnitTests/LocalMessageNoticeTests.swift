import XCTest
@testable import AgentRelayiPad

final class LocalMessageNoticeTests: XCTestCase {
    func testNoBannerWhenPushIsAvailable() {
        XCTAssertFalse(LocalMessageNotice.shouldPost(apnsToken: "abc123", initialized: true, messageID: "m2",
                                                     previousMessageID: "m1", fromMe: false))
    }

    func testBannerForNewMessageWithoutPush() {
        XCTAssertTrue(LocalMessageNotice.shouldPost(apnsToken: nil, initialized: true, messageID: "m2",
                                                    previousMessageID: "m1", fromMe: false))
        XCTAssertTrue(LocalMessageNotice.shouldPost(apnsToken: "", initialized: true, messageID: "m2",
                                                    previousMessageID: nil, fromMe: false))
    }

    func testNoBannerForSeenOwnOrFirstLoadMessages() {
        XCTAssertFalse(LocalMessageNotice.shouldPost(apnsToken: nil, initialized: true, messageID: "m1",
                                                     previousMessageID: "m1", fromMe: false))
        XCTAssertFalse(LocalMessageNotice.shouldPost(apnsToken: nil, initialized: true, messageID: "m2",
                                                     previousMessageID: "m1", fromMe: true))
        XCTAssertFalse(LocalMessageNotice.shouldPost(apnsToken: nil, initialized: false, messageID: "m2",
                                                     previousMessageID: nil, fromMe: false))
        XCTAssertFalse(LocalMessageNotice.shouldPost(apnsToken: nil, initialized: true, messageID: "",
                                                     previousMessageID: "m1", fromMe: false))
    }
    func testNewMessageBeforeRosterResponseStillGetsOneBanner() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m1", isCurrentPeer: true,
                                         apnsToken: nil, initialized: false, fromMe: false))
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m2", isCurrentPeer: false,
                                         apnsToken: nil, initialized: true, fromMe: false))
        XCTAssertTrue(notices.shouldPost(conversationID: "dm_1", messageID: "m2", isCurrentPeer: true,
                                        apnsToken: nil, initialized: true, fromMe: false))
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m2", isCurrentPeer: true,
                                         apnsToken: nil, initialized: true, fromMe: false))
    }

    func testInitialHistoryStaysQuietWhenRosterArrivesAfterThreads() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m1", isCurrentPeer: false,
                                         apnsToken: nil, initialized: false, fromMe: false))
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m1", isCurrentPeer: true,
                                         apnsToken: nil, initialized: true, fromMe: false))
    }

    func testPushAndOwnMessagesAdvanceTheVisibleCursorWithoutLocalBanners() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m1", isCurrentPeer: true,
                                         apnsToken: "apns", initialized: true, fromMe: false))
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m1", isCurrentPeer: true,
                                         apnsToken: nil, initialized: true, fromMe: false))
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m2", isCurrentPeer: true,
                                         apnsToken: nil, initialized: true, fromMe: true))
    }

    // MARK: Server read state

    private func post(_ notices: inout LocalMessageNotices, _ messageID: String, _ serverSays: ConversationReadState.Notice,
                      initialized: Bool = true, apnsToken: String? = nil) -> Bool {
        notices.shouldPost(conversationID: "dm_1", messageID: messageID, isCurrentPeer: true, apnsToken: apnsToken,
                           initialized: initialized, fromMe: false, serverSays: serverSays)
    }

    func testUnknownServerStateDecidesAsBefore() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .unknown, initialized: false))
        XCTAssertTrue(post(&notices, "m2", .unknown))
        XCTAssertFalse(post(&notices, "m2", .unknown))
    }

    func testUnreadPostsANewMessageOnce() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .unread, initialized: false), "First load stays quiet")
        XCTAssertTrue(post(&notices, "m2", .unread))
        XCTAssertFalse(post(&notices, "m2", .unread))
    }

    func testReadHoldsTheBannerBackUntilTheServerSaysUnread() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .read, initialized: false))
        XCTAssertFalse(post(&notices, "m2", .read), "Held back")
        XCTAssertFalse(post(&notices, "m2", .read))
        XCTAssertTrue(post(&notices, "m2", .unread), "Shows once the server says unread")
        XCTAssertFalse(post(&notices, "m2", .unread), "Only once")
    }

    func testAnInboxOutageDoesNotPostAHeldBackBanner() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .read, initialized: false))
        XCTAssertFalse(post(&notices, "m2", .read))
        XCTAssertFalse(post(&notices, "m2", .unknown), "An outage says nothing new")
        XCTAssertTrue(post(&notices, "m2", .unread), "Still held back for the next answer")
    }

    func testAHeldBackBannerIsForgottenWhenANewerMessageArrives() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .read, initialized: false))
        XCTAssertFalse(post(&notices, "m2", .read), "m2 held back")
        // The person answers from another device: m3 is their own message.
        XCTAssertFalse(notices.shouldPost(conversationID: "dm_1", messageID: "m3", isCurrentPeer: true, apnsToken: nil,
                                          initialized: true, fromMe: true, serverSays: .read))
        XCTAssertFalse(post(&notices, "m3", .unread), "m2's banner was forgotten")
        XCTAssertTrue(post(&notices, "m4", .unread))
    }

    func testFirstLoadStaysQuietWhateverTheServerSays() {
        for serverSays in [ConversationReadState.Notice.read, .unread, .unknown] {
            var notices = LocalMessageNotices()
            XCTAssertFalse(post(&notices, "m1", serverSays, initialized: false))
            XCTAssertFalse(post(&notices, "m1", .unread), "An old message never posts later")
        }
    }

    func testAHeldBackBannerDoesNotPostOncePushIsAvailable() {
        var notices = LocalMessageNotices()
        XCTAssertFalse(post(&notices, "m1", .read, initialized: false))
        XCTAssertFalse(post(&notices, "m2", .read))
        XCTAssertFalse(post(&notices, "m2", .unread, apnsToken: "apns"))
    }
}
