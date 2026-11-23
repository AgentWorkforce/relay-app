import Foundation

/// Which direct conversations are unread, from the server's read state.
///
/// Relaycast keeps one read cursor per agent and conversation, shared with
/// the desktop (which reads and writes as the same agent). `GET /inbox`
/// lists the conversations with someone else's messages after that cursor;
/// `POST /messages/<id>/read` moves it forward. The conversation list's
/// `unread_count` counts every message, the agent's own included, so it is
/// never used.
///
/// Until the first inbox answer nothing is unread. A failed inbox read keeps
/// the last answer. A read this device posts takes the conversation off the
/// unread set at once, and an inbox answer requested before that post
/// finished cannot put it back: it was read from before the post.
struct ConversationReadState {
    enum Outcome: Equatable {
        case success
        /// The server does not know the message (404): never retried.
        case notFound
        /// Anything else: the next history load may try again.
        case failed
    }

    /// A read this device posted for a conversation: in flight while
    /// `finishedAt` is nil, else the counter value when it succeeded.
    private struct Mark: Equatable {
        let messageID: String
        var finishedAt: Int?
    }

    /// Server conversation IDs (like `dm_x`) with unread messages.
    private(set) var unreadConversationIDs: Set<String> = []
    /// Whether any inbox answer has been applied.
    private(set) var hasAnswer = false
    /// Whether the latest inbox read answered and was applied. When it did
    /// not, the unread set is only the last known one.
    private(set) var answerIsCurrent = false
    /// Advances with every read that starts or succeeds, so an inbox answer
    /// can tell whether it was requested before or after a read.
    private var counter = 0
    /// Message IDs the server accepted (or does not know): one POST each.
    private var posted: Set<String> = []
    private var inFlight: Set<String> = []
    private var marks: [String: Mark] = [:]

    // MARK: Inbox

    /// The unread direct conversations in a `GET /inbox` answer, mapped to
    /// their last message ID ("" when the answer has none). The agent's own
    /// messages never make a conversation unread.
    static func unread(inInbox inbox: [String: Any], mine: String) -> [String: String] {
        var result: [String: String] = [:]
        for entry in inbox["unread_dms"] as? [[String: Any]] ?? [] {
            let conversationID = text(entry["conversation_id"])
            guard !conversationID.isEmpty, text(entry["from"]) != mine else { continue }
            if let count = entry["unread_count"] as? NSNumber, count.intValue <= 0 { continue }
            result[conversationID] = text((entry["last_message"] as? [String: Any])?["id"])
        }
        return result
    }

    /// Takes a ticket for an inbox request about to be sent.
    mutating func inboxRequested() -> Int { counter }

    /// Applies an inbox answer requested with `ticket`. A conversation this
    /// device is reading, or read after the request was sent, stays read.
    mutating func apply(inbox unread: [String: String], ticket: Int) {
        var next = Set(unread.keys)
        for (conversationID, mark) in marks {
            if let finished = mark.finishedAt, finished <= ticket {
                marks.removeValue(forKey: conversationID)
            } else {
                next.remove(conversationID)
            }
        }
        unreadConversationIDs = next
        hasAnswer = true
        answerIsCurrent = true
    }

    /// The latest inbox read failed or was not applied: the last known set
    /// stays, but it no longer speaks for new messages.
    mutating func inboxUnanswered() {
        answerIsCurrent = false
    }

    // MARK: Reading

    /// Starts marking `messageID` read. False when it was already posted or
    /// is being posted: each message is posted at most once.
    mutating func beginRead(conversationID: String, messageID: String) -> Bool {
        guard !conversationID.isEmpty, !messageID.isEmpty,
              !posted.contains(messageID), !inFlight.contains(messageID) else { return false }
        inFlight.insert(messageID)
        counter += 1
        marks[conversationID] = Mark(messageID: messageID, finishedAt: nil)
        unreadConversationIDs.remove(conversationID)
        return true
    }

    mutating func finishRead(conversationID: String, messageID: String, outcome: Outcome) {
        inFlight.remove(messageID)
        let ownsMark = marks[conversationID]?.messageID == messageID
        switch outcome {
        case .success:
            posted.insert(messageID)
            counter += 1
            if ownsMark { marks[conversationID]?.finishedAt = counter }
        case .notFound:
            posted.insert(messageID)
            if ownsMark { marks.removeValue(forKey: conversationID) }
        case .failed:
            if ownsMark { marks.removeValue(forKey: conversationID) }
        }
    }

    /// The message to mark read in a loaded page (oldest first): its newest
    /// message from someone else. Nil when the newest incoming message the
    /// conversation list reports is not in the page yet, or when there is
    /// no incoming message. Pass "" as the ID of a row without a server ID.
    static func messageToMarkRead(page: [(id: String, mine: Bool)], latestIncomingID: String?) -> String? {
        if let latest = latestIncomingID, !page.contains(where: { $0.id == latest }) { return nil }
        return page.last { !$0.mine && !$0.id.isEmpty }?.id
    }

    /// A read is posted only for an open direct conversation whose history
    /// has loaded, while the app is in the foreground. Never for a channel.
    static func mayMarkRead(isDirectConversation: Bool, isOpen: Bool, historyLoaded: Bool, appActive: Bool) -> Bool {
        isDirectConversation && isOpen && historyLoaded && appActive
    }

    // MARK: Carrying over the device's old cursor

    /// The device-local "seen" cursor older builds kept, if there is one:
    /// the saved map, or an empty map when only its baseline flag was saved.
    static func oldSeenCursor(_ stored: [String: Any]?, hasBaseline: Bool) -> [String: String]? {
        if let stored { return stored.compactMapValues { $0 as? String } }
        return hasBaseline ? [:] : nil
    }

    /// The reads to post so the server knows what this device had already
    /// seen: for each unread conversation, the message the old cursor holds.
    static func carryOverPlan(unread: some Sequence<String>, seen: [String: String]) -> [(conversationID: String, messageID: String)] {
        Set(unread).sorted().compactMap { conversationID in
            guard let messageID = seen[conversationID], !messageID.isEmpty else { return nil }
            return (conversationID, messageID)
        }
    }

    /// The old cursor may be forgotten once every carried-over read reached
    /// the server (or the server does not know the message). An empty plan
    /// is complete.
    static func carryOverComplete(_ outcomes: [Outcome]) -> Bool {
        outcomes.allSatisfy { $0 != .failed }
    }

    // MARK: Notifications

    /// Delivered notifications for direct conversations that are now read,
    /// local and remote alike. Channel and thread notifications stay.
    static func deliveredNotificationsToRemove(_ delivered: [(identifier: String, userInfo: [AnyHashable: Any])],
                                               directConversationIDs: Set<String>,
                                               unread: Set<String>) -> [String] {
        delivered.compactMap { item in
            let info = item.userInfo
            guard ["channel_name", "channel_id", "thread_id", "parent_id"].allSatisfy({ info[$0] == nil }),
                  let conversationID = info["conversation_id"] as? String,
                  directConversationIDs.contains(conversationID),
                  !unread.contains(conversationID) else { return nil }
            return item.identifier
        }
    }

    /// The app's own banner is held back only for a conversation the latest
    /// inbox read says has nothing unread. Before the first answer, or when
    /// the latest read failed or was not applied, it is not held back.
    func allowsLocalNotice(conversationID: String) -> Bool {
        !answerIsCurrent || unreadConversationIDs.contains(conversationID)
    }

    private static func text(_ value: Any?) -> String {
        if let value = value as? String { return value }
        if let value = value as? NSNumber { return value.stringValue }
        return ""
    }
}
