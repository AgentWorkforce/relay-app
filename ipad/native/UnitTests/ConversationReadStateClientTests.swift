import XCTest
import UserNotifications
@testable import AgentRelayiPad

/// Drives RelayClient itself against an in-memory Relaycast (the Debug
/// fixture hook in `api()`) and a fake notification centre: no account,
/// token or network.
private let me = "unit-read-person"
private let workspace = "unit-read-workspace"
private let peer = "unit-read-peer"
private let conversationID = "dm_unit_read"

@MainActor
final class ConversationReadStateClientTests: XCTestCase {

    /// The server: one conversation with a person, newest messages last.
    private final class FakeRelay {
        var incoming = ["m1"]
        var unread = false
        var inboxFails = false
        var reads: [String] = []

        func respond(_ path: String, _ method: String, _ body: JSONObject?) -> Any? {
            if path == "/inbox" {
                // Not an object: the app treats it as a failed read.
                if inboxFails { return "unavailable" }
                let dms: [JSONObject] = unread ? [["conversation_id": conversationID, "from": peer, "unread_count": 1,
                                                    "last_message": ["id": incoming.last!, "text": "Invented"]]] : []
                return ["unread_channels": [JSONObject](), "mentions": [JSONObject](), "unread_dms": dms,
                        "recent_reactions": [JSONObject]()] as JSONObject
            }
            if method == "POST", path.hasPrefix("/messages/"), path.hasSuffix("/read") {
                let id = String(path.dropFirst("/messages/".count).dropLast("/read".count))
                reads.append(id)
                if id == incoming.last { unread = false }
                return JSONObject()
            }
            if path == "/dm/conversations" {
                return [["id": conversationID,
                         "participants": [["agent_name": me, "agent_id": "a_me"], ["agent_name": peer, "agent_id": "a_peer"]],
                         "last_message": ["id": incoming.last!, "agent_id": "a_peer", "agent_name": peer,
                                          "text": "Invented"]] as JSONObject]
            }
            if path.hasPrefix("/dm/\(conversationID)/messages") {
                return incoming.reversed().map { ["id": $0, "agent_name": peer, "agent_id": "a_peer", "text": "Invented text",
                                                  "created_at": "2026-11-01T10:00:00.000Z"] as JSONObject }
            }
            if path.hasPrefix("/agents?") || path == "/agents" {
                return [["name": peer, "type": "human", "status": "online"] as JSONObject]
            }
            if path == "/nodes" || path == "/channels" || path.hasSuffix("/readers") { return [JSONObject]() }
            return JSONObject()
        }
    }

    private final class FakeCenter {
        var delivered: [ConversationReadState.Delivered] = []
        var removed: [String] = []
        var posted: [String] = []
    }

    private var server = FakeRelay()
    private var center = FakeCenter()
    private var savedName: String?
    private var savedPushToken: String?
    private var client: RelayClient!

    private var storageKeys: [String] {
        ["seen", "baseline", "carryOverFailures", "hidden"].map { "relay.mobileUnread.\(workspace).\(me).\($0)" }
    }

    override func setUp() async throws {
        savedName = UserDefaults.standard.string(forKey: "relay.mobileName")
        UserDefaults.standard.set(me, forKey: "relay.mobileName")
        storageKeys.forEach(UserDefaults.standard.removeObject(forKey:))
        // The app's own banner is for a device without push.
        savedPushToken = RelayPushState.shared.token
        RelayPushState.shared.token = nil
        server = FakeRelay()
        center = FakeCenter()
        let server = server, center = center
        RelayUITestFixture.unitTestResponder = { server.respond($0, $1, $2) }
        client = RelayClient()
        client.paired = true
        client.cloudWorkspaceID = workspace
        client.isAppActive = { true }
        client.notificationCenter = RelayNotificationCenter(
            delivered: { center.delivered },
            remove: { ids in
                center.removed += ids
                center.delivered.removeAll { ids.contains($0.identifier) }
            },
            post: { center.posted.append($0.identifier) })
    }

    override func tearDown() async throws {
        RelayUITestFixture.unitTestResponder = nil
        client = nil
        RelayPushState.shared.token = savedPushToken
        storageKeys.forEach(UserDefaults.standard.removeObject(forKey:))
        if let savedName { UserDefaults.standard.set(savedName, forKey: "relay.mobileName") }
        else { UserDefaults.standard.removeObject(forKey: "relay.mobileName") }
    }

    private func notification(_ id: String) -> ConversationReadState.Delivered {
        (identifier: id, userInfo: ["conversation_id": conversationID], date: .distantPast)
    }

    private var peerRow: String { "conversation:\(peer)" }

    func testAnAppliedInboxAnswerClearsNotificationsOfReadConversations() async {
        center.delivered = [notification("n1")]
        server.inboxFails = true
        await client.refresh()
        XCTAssertTrue(center.removed.isEmpty, "A failed inbox read clears nothing")

        server.inboxFails = false
        server.unread = true
        await client.pollMessages()
        XCTAssertTrue(center.removed.isEmpty, "An unread conversation keeps its notification")
        XCTAssertTrue(client.unreadConversationIDs.contains(peerRow))

        server.unread = false
        await client.pollMessages()
        XCTAssertEqual(center.removed, ["n1"], "Read elsewhere: the notification goes")
        XCTAssertFalse(client.unreadConversationIDs.contains(peerRow))
        XCTAssertTrue(server.reads.isEmpty)
    }

    func testAReadPostClearsTheConversationsNotifications() async throws {
        server.incoming = ["m1", "m2"]
        server.unread = true
        center.delivered = [notification("n1")]
        await client.refresh()
        XCTAssertTrue(client.unreadConversationIDs.contains(peerRow))
        XCTAssertTrue(center.removed.isEmpty)

        let agent = try XCTUnwrap(client.conversations.first { $0.name == peer })
        client.selectConversation(agent)
        await client.loadHistory(for: agent)
        XCTAssertEqual(server.reads, ["m2"])
        XCTAssertFalse(client.unreadConversationIDs.contains(peerRow))
        XCTAssertEqual(center.removed, ["n1"], "The read POST clears the notification without another inbox read")

        await client.loadHistory(for: agent)
        XCTAssertEqual(server.reads, ["m2"], "One read POST per message")
    }

    func testNoReadPostWhileTheAppIsInTheBackground() async throws {
        server.incoming = ["m1", "m2"]
        server.unread = true
        client.isAppActive = { false }
        await client.refresh()
        let agent = try XCTUnwrap(client.conversations.first { $0.name == peer })
        client.selectConversation(agent)
        await client.loadHistory(for: agent)
        XCTAssertTrue(server.reads.isEmpty)
        XCTAssertTrue(client.unreadConversationIDs.contains(peerRow))
    }

    func testTheBannerFollowsTheServersReadState() async {
        await client.refresh()
        XCTAssertTrue(center.posted.isEmpty, "The first load stays quiet")

        // A new message the desktop has already read: no banner.
        server.incoming = ["m1", "m2"]
        server.unread = false
        await client.pollMessages()
        XCTAssertTrue(center.posted.isEmpty, "The server says it is read")

        // An inbox outage says nothing new about it.
        server.inboxFails = true
        await client.pollMessages()
        XCTAssertTrue(center.posted.isEmpty)

        // The server now says it is unread: the held-back banner shows once.
        server.inboxFails = false
        server.unread = true
        await client.pollMessages()
        XCTAssertEqual(center.posted, ["m2"])
        await client.pollMessages()
        XCTAssertEqual(center.posted, ["m2"])

        // During an outage a newer message still gets its banner.
        server.inboxFails = true
        server.incoming = ["m1", "m2", "m3"]
        await client.pollMessages()
        XCTAssertEqual(center.posted, ["m2", "m3"])
    }
}
