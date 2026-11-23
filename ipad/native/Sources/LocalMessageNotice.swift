import Foundation

/// Decides when the app shows its own "New message" banner. It is only a
/// fallback for a device without push: once iOS has issued an APNs token,
/// Relaycast pushes each message and iOS shows that push even while the app
/// is open, so a local banner as well would be a second one for one message.
enum LocalMessageNotice {
    static func shouldPost(apnsToken: String?, initialized: Bool, messageID: String,
                           previousMessageID: String?, fromMe: Bool) -> Bool {
        guard apnsToken?.isEmpty ?? true else { return false }
        return initialized && !messageID.isEmpty && previousMessageID != messageID && !fromMe
    }
}

/// The notification cursor advances only when a peer can be shown. The DM
/// cursor may advance first while a concurrent presence request is pending.
///
/// `serverSays` is what the latest inbox read says about the conversation.
/// `.unknown` (no answer this time) decides as if there were no server
/// state. `.read` holds back a banner that would have shown, remembering it
/// for that message; `.unread` shows such a held-back banner once. A held-back
/// banner is forgotten when the conversation's newest message changes.
struct LocalMessageNotices {
    private var messageIDs: [String: String] = [:]
    private var heldBack: [String: String] = [:]

    mutating func shouldPost(conversationID: String, messageID: String, isCurrentPeer: Bool,
                             apnsToken: String?, initialized: Bool, fromMe: Bool,
                             serverSays: ConversationReadState.Notice = .unknown) -> Bool {
        let previous = messageIDs[conversationID]
        if isCurrentPeer || !initialized { messageIDs[conversationID] = messageID }
        if let held = heldBack[conversationID], held != messageID { heldBack.removeValue(forKey: conversationID) }
        guard isCurrentPeer else { return false }
        let post = LocalMessageNotice.shouldPost(apnsToken: apnsToken, initialized: initialized,
                                                 messageID: messageID, previousMessageID: previous,
                                                 fromMe: fromMe)
        switch serverSays {
        case .unknown:
            return post
        case .read:
            if post { heldBack[conversationID] = messageID }
            return false
        case .unread:
            let wasHeldBack = heldBack.removeValue(forKey: conversationID) == messageID
            return post || (wasHeldBack && (apnsToken?.isEmpty ?? true))
        }
    }
}
